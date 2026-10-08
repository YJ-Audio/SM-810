use crate::{Engine, Error, Result, query::Query};
use sampler_db::{self as db, OptionalExtension};
use sampler_embed::{DIMENSIONS, MODEL};
use sampler_similarity::layout::{Point, align, place};
use serde::Serialize;
use std::{
	collections::{HashMap, HashSet},
	fs::File,
	path::Path,
	process::{Command, Stdio},
	thread,
	time::Duration,
};

#[derive(Debug, Serialize)]
pub struct MapSummary {
	pub id: i64,
	pub name: String,
	pub query: Query,
	pub layout_rev: i64,
	pub points: usize,
	pub provisional: usize,
	pub labels: Vec<Label>,
}
#[derive(Debug, Serialize)]
pub struct Label {
	pub x: f32,
	pub y: f32,
	pub text: String,
}
struct MapRow {
	point: Point,
	kind: u8,
	key: u8,
	available: bool,
	provisional: bool,
	size: f32,
	duration: f32,
	tags: Vec<String>,
}
impl Engine {
	pub fn ensure_maps(&self) -> Result<()> {
		self.write(|tx| {
			let count: i64 = tx.query_row("SELECT count(*) FROM maps", [], |r| r.get(0))?;
			if count == 0 {
				let text = |words: &[&str]| Query::Any {
					conditions: words.iter().map(|s| Query::Text { text: (*s).into() }).collect(),
				};
				let defaults = [
					("All sounds", Query::default()),
					(
						"Drums · one-shots",
						Query::All {
							conditions: vec![
								text(&["kick", "snare", "clap", "hat", "perc", "tom"]),
								Query::Not {
									condition: Box::new(Query::Field {
										field: crate::query::Field::IsLoop,
										op: crate::query::Comparison::Eq,
										value: 1.0,
									}),
								},
							],
						},
					),
					("Bass", text(&["bass", "sub"])),
					("Textures", text(&["texture", "pad", "ambient", "noise"])),
					(
						"Loops",
						Query::Field {
							field: crate::query::Field::IsLoop,
							op: crate::query::Comparison::Eq,
							value: 1.0,
						},
					),
				];
				for (name, query) in defaults {
					let query = serde_json::to_string(&query).map_err(|e| db::Error::Invalid(e.to_string()))?;
					tx.execute(
						"INSERT INTO maps(name,model,query) VALUES(?1,?2,?3)",
						(name, MODEL, query),
					)?;
				}
			}
			Ok(())
		})
	}
	pub fn create_map(&self, name: String, query: Query) -> Result<()> {
		if name.trim().is_empty() || name.chars().count() > 100 {
			return Err(Error::Invalid("Map name must contain 1–100 characters".into()));
		}
		let _ = self.query_ids(&query)?;
		let query = serde_json::to_string(&query)?;
		self.write(move |tx| {
			tx.execute(
				"INSERT INTO maps(name,model,query) VALUES(?1,?2,?3)",
				(name, MODEL, query),
			)?;
			Ok(())
		})
	}
	pub fn maps(&self) -> Result<Vec<MapSummary>> {
		let reader = self.reader()?;
		let mut statement=reader.prepare("SELECT id,name,query,layout_rev,(SELECT count(*) FROM map_points WHERE map_id=maps.id),(SELECT count(*) FROM map_points WHERE map_id=maps.id AND placed_by='neighbors') FROM maps WHERE model=?1 ORDER BY id").map_err(db::Error::from)?;
		let rows = statement
			.query_map([MODEL], |r| {
				Ok((
					r.get::<_, i64>(0)?,
					r.get::<_, String>(1)?,
					r.get::<_, String>(2)?,
					r.get::<_, i64>(3)?,
					r.get::<_, i64>(4)?,
					r.get::<_, i64>(5)?,
				))
			})
			.map_err(db::Error::from)?;
		rows.map(|row| {
			let (id, name, query, layout_rev, points, provisional) = row.map_err(db::Error::from)?;
			Ok(MapSummary {
				id,
				name,
				query: serde_json::from_str(&query)?,
				layout_rev,
				points: points as usize,
				provisional: provisional as usize,
				labels: Vec::new(),
			})
		})
		.collect()
	}
	pub fn map_query(&self, id: i64) -> Result<Query> {
		let value: Option<String> = self
			.reader()?
			.query_row("SELECT query FROM maps WHERE id=?1 AND model=?2", (id, MODEL), |r| {
				r.get(0)
			})
			.optional()
			.map_err(db::Error::from)?;
		Ok(serde_json::from_str(&value.ok_or_else(|| {
			Error::Invalid("Map was not found for the current model".into())
		})?)?)
	}
	pub fn scope_map(&self, request: &mut db::BrowseQuery) -> Result<()> {
		if let Some(id) = request.map_id {
			let mut ids = self.query_ids(&self.map_query(id)?)?;
			if let Some(existing) = &request.ids {
				let allowed: HashSet<_> = existing.iter().copied().collect();
				ids.retain(|id| allowed.contains(id));
			}
			request.ids = Some(ids.into_iter().collect());
		}
		Ok(())
	}
	pub fn sync_map(&self, id: i64) -> Result<usize> {
		let Ok(_guard) = self.map_lock.try_lock() else {
			return Ok(0);
		};
		let candidates = self.query_ids(&self.map_query(id)?)?;
		let mut existing: Vec<Point> = self.map_rows(id)?.into_iter().map(|r| r.point).collect();
		let placed: HashSet<_> = existing.iter().map(|p| p.id).collect();
		let mut missing: Vec<_> = candidates.difference(&placed).copied().collect();
		missing.sort_unstable();
		self.prioritize_embeddings(missing.iter().copied().take(180).collect())?;
		let vectors = self
			.vectors
			.read()
			.map_err(|_| Error::Invalid("Vector store lock poisoned".into()))?;
		let mut additions = Vec::new();
		for id in missing.into_iter().filter(|id| vectors.vector(*id).is_some()).take(512) {
			let point = place(&vectors, id, &existing)?;
			existing.push(point);
			additions.push(point);
		}
		drop(vectors);
		let count = additions.len();
		if count > 0 {
			self.write(move |tx| {
				for point in additions {
					tx.execute(
						"INSERT OR IGNORE INTO map_points(map_id,sample_id,x,y,placed_by) VALUES(?1,?2,?3,?4,'neighbors')",
						(id, point.id, point.x, point.y),
					)?;
				}
				Ok(())
			})?;
		}
		Ok(count)
	}
	fn map_rows(&self, id: i64) -> Result<Vec<MapRow>> {
		let reader = self.reader()?;
		let mut statement=reader.prepare("SELECT mp.sample_id,mp.x,mp.y,mp.placed_by,COALESCE(a.key_root,255),COALESCE(a.lufs,-36),COALESCE(a.duration_ms,0),MIN(f.rel_path),MAX(r.enabled AND rs.status!='offline' AND f.last_seen_scan>=rs.complete_generation),COALESCE((SELECT group_concat(t.name,char(31)) FROM sample_tags st JOIN tags t ON t.id=st.tag_id WHERE st.sample_id=mp.sample_id),'') FROM map_points mp JOIN files f ON f.sample_id=mp.sample_id JOIN roots r ON r.id=f.root_id JOIN root_state rs ON rs.root_id=r.id LEFT JOIN analysis a ON a.sample_id=mp.sample_id WHERE mp.map_id=?1 GROUP BY mp.sample_id ORDER BY mp.sample_id").map_err(db::Error::from)?;
		let rows = statement
			.query_map([id], |r| {
				let tags: String = r.get(9)?;
				let name: String = r.get(7)?;
				let kind = kind(&format!("{name} {tags}"));
				let lufs: f32 = r.get(5)?;
				Ok(MapRow {
					point: Point {
						id: r.get(0)?,
						x: r.get(1)?,
						y: r.get(2)?,
					},
					kind,
					key: r.get::<_, i64>(4)? as u8,
					available: r.get(8)?,
					provisional: r.get::<_, String>(3)? == "neighbors",
					size: 2.5 + ((lufs + 60.0) / 57.0).clamp(0.0, 1.0) * 3.0,
					duration: r.get::<_, f32>(6)? / 1000.0,
					tags: tags
						.split('\u{1f}')
						.filter(|s| !s.is_empty())
						.map(String::from)
						.collect(),
				})
			})
			.map_err(db::Error::from)?;
		rows.map(|row| row.map_err(db::Error::from).map_err(Error::from))
			.collect()
	}
	pub fn map_summary(&self, id: i64) -> Result<MapSummary> {
		let mut summary = self
			.maps()?
			.into_iter()
			.find(|m| m.id == id)
			.ok_or_else(|| Error::Invalid("Map was not found".into()))?;
		summary.labels = labels(&self.map_rows(id)?);
		Ok(summary)
	}
	pub fn map_binary(&self, id: i64, mut request: db::BrowseQuery) -> Result<Vec<u8>> {
		self.sync_map(id)?;
		request.map_id = Some(id);
		self.scope_map(&mut request)?;
		let matched: HashSet<_> = if request.text.trim_start().starts_with('~') {
			request.limit = Some(200);
			request.offset = 0;
			self.browse(request)?.items.into_iter().map(|r| r.sample.id).collect()
		} else {
			db::matching_ids(&self.reader()?, &request)?.into_iter().collect()
		};
		let rows = self.map_rows(id)?;
		let revision: i64 = self
			.reader()?
			.query_row("SELECT layout_rev FROM maps WHERE id=?1", [id], |r| r.get(0))
			.map_err(db::Error::from)?;
		let mut bytes = Vec::with_capacity(32 + rows.len() * 32);
		bytes.extend_from_slice(b"MAP1");
		bytes.extend_from_slice(&1u32.to_le_bytes());
		bytes.extend_from_slice(&id.to_le_bytes());
		bytes.extend_from_slice(&revision.to_le_bytes());
		bytes.extend_from_slice(&(rows.len() as u32).to_le_bytes());
		bytes.extend_from_slice(&(rows.iter().filter(|r| r.provisional).count() as u32).to_le_bytes());
		for row in rows {
			bytes.extend_from_slice(&row.point.id.to_le_bytes());
			bytes.extend_from_slice(&row.point.x.to_le_bytes());
			bytes.extend_from_slice(&row.point.y.to_le_bytes());
			bytes.extend_from_slice(&[
				row.kind,
				row.key,
				u8::from(matched.contains(&row.point.id))
					| (u8::from(row.available) * 2)
					| (u8::from(row.provisional) * 4),
				0,
			]);
			bytes.extend_from_slice(&row.size.to_le_bytes());
			bytes.extend_from_slice(&row.duration.to_le_bytes());
			bytes.extend_from_slice(&0f32.to_le_bytes());
		}
		Ok(bytes)
	}
	pub fn recompute_map(&self, id: i64, program: &Path, cancelled: impl Fn() -> bool) -> Result<()> {
		let _guard = self.map_lock.try_lock().map_err(|_| Error::Busy)?;
		let candidates = self.query_ids(&self.map_query(id)?)?;
		let previous: Vec<_> = self.map_rows(id)?.into_iter().map(|r| r.point).collect();
		let vectors = self
			.vectors
			.read()
			.map_err(|_| Error::Invalid("Vector store lock poisoned".into()))?;
		let ids: Vec<_> = vectors
			.ids()
			.iter()
			.filter(|id| candidates.contains(id))
			.copied()
			.collect();
		if ids.is_empty() {
			return Err(Error::Invalid("Index sounds before recomputing this map".into()));
		}
		let matrix: Vec<_> = ids
			.iter()
			.flat_map(|id| vectors.vector(*id).expect("store row exists").iter().copied())
			.collect();
		drop(vectors);
		let dir = tempfile::tempdir()?;
		let input = dir.path().join("input.bin");
		let output = dir.path().join("output.bin");
		let stderr = dir.path().join("stderr.txt");
		sampler_layout::write_input(&input, &ids, DIMENSIONS, &matrix)?;
		drop(matrix);
		let mut child = Command::new(program)
			.arg(&input)
			.arg(&output)
			.stdout(Stdio::null())
			.stderr(File::create(&stderr)?)
			.spawn()
			.map_err(|e| Error::Invalid(format!("Cannot start layout sidecar {}: {e}", program.display())))?;
		let status = loop {
			if cancelled() {
				let _ = child.kill();
				let _ = child.wait();
				return Err(Error::Invalid("Layout recomputation cancelled".into()));
			}
			match child.try_wait() {
				Ok(Some(status)) => break status,
				Ok(None) => {}
				Err(error) => {
					let _ = child.kill();
					let _ = child.wait();
					return Err(error.into());
				}
			}
			thread::sleep(Duration::from_millis(50));
		};
		if !status.success() {
			let detail = std::fs::read_to_string(&stderr).unwrap_or_default();
			return Err(Error::Invalid(format!(
				"Layout sidecar failed ({status}): {}",
				detail.chars().take(2000).collect::<String>()
			)));
		}
		let mut points = sampler_layout::read_output(&output)?;
		let expected: HashSet<_> = ids.iter().copied().collect();
		let actual: HashSet<_> = points.iter().map(|p| p.id).collect();
		if points.len() != ids.len() || actual != expected {
			return Err(Error::Invalid("Layout sidecar returned different sample IDs".into()));
		}
		align(&mut points, &previous)?;
		if cancelled() {
			return Err(Error::Invalid("Layout recomputation cancelled".into()));
		}
		self.write(move |tx|{
            for point in points {tx.execute("INSERT INTO map_points(map_id,sample_id,x,y,placed_by) VALUES(?1,?2,?3,?4,'projection') ON CONFLICT(map_id,sample_id) DO UPDATE SET x=excluded.x,y=excluded.y,placed_by='projection'",(id,point.id,point.x,point.y))?;}
            tx.execute("UPDATE maps SET layout_rev=layout_rev+1 WHERE id=?1",[id])?;Ok(())
        })
	}
}
fn kind(text: &str) -> u8 {
	let text = text.to_lowercase();
	if ["kick", "kck"].iter().any(|s| text.contains(s)) {
		0
	} else if ["snare", "clap", "snr"].iter().any(|s| text.contains(s)) {
		1
	} else if ["hat", "cymbal", "ride"].iter().any(|s| text.contains(s)) {
		2
	} else if ["perc", "shaker", "tom", "conga"].iter().any(|s| text.contains(s)) {
		3
	} else if ["bass", "sub"].iter().any(|s| text.contains(s)) {
		4
	} else {
		5
	}
}
fn labels(rows: &[MapRow]) -> Vec<Label> {
	if rows.len() < 8 {
		return Vec::new();
	}
	let k = ((rows.len() as f32 / 40.0).sqrt() as usize).clamp(1, 10);
	let mut centers = vec![(rows[0].point.x, rows[0].point.y)];
	while centers.len() < k {
		let furthest = rows
			.iter()
			.max_by(|a, b| {
				let distance = |row: &MapRow| {
					centers
						.iter()
						.map(|&(x, y)| (row.point.x - x).powi(2) + (row.point.y - y).powi(2))
						.fold(f32::INFINITY, f32::min)
				};
				distance(a).total_cmp(&distance(b))
			})
			.expect("nonempty map");
		centers.push((furthest.point.x, furthest.point.y));
	}
	let nearest = |row: &MapRow, centers: &[(f32, f32)]| {
		centers
			.iter()
			.enumerate()
			.min_by(|(_, a), (_, b)| {
				let distance = |(x, y): &&(f32, f32)| (row.point.x - x).powi(2) + (row.point.y - y).powi(2);
				distance(a).total_cmp(&distance(b))
			})
			.map(|(i, _)| i)
			.expect("nonempty centers")
	};
	for _ in 0..12 {
		let mut sums = vec![(0.0, 0.0, 0usize); k];
		for row in rows {
			let group = nearest(row, &centers);
			sums[group].0 += row.point.x;
			sums[group].1 += row.point.y;
			sums[group].2 += 1;
		}
		for (center, (x, y, n)) in centers.iter_mut().zip(sums) {
			if n > 0 {
				*center = (x / n as f32, y / n as f32);
			}
		}
	}
	let names = ["Kick", "Snare / Clap", "Hat", "Percussion", "Bass", "Samples"];
	let mut counts: Vec<HashMap<String, usize>> = vec![HashMap::new(); k];
	for row in rows {
		let group = nearest(row, &centers);
		if row.tags.is_empty() {
			*counts[group].entry(names[row.kind as usize].into()).or_default() += 1;
		} else {
			for tag in &row.tags {
				*counts[group].entry(tag.clone()).or_default() += 1;
			}
		}
	}
	centers
		.into_iter()
		.zip(counts)
		.filter_map(|((x, y), counts)| {
			counts
				.into_iter()
				.max_by(|(a, n), (b, m)| n.cmp(m).then_with(|| b.cmp(a)))
				.map(|(text, _)| Label { x, y, text })
		})
		.collect()
}
