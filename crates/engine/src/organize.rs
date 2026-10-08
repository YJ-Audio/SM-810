use crate::{Engine, Error, Result, query::Query};
use sampler_db::{self as db, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, sync::mpsc};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
	pub id: Option<i64>,
	pub tag: String,
	pub target: RuleTarget,
	pub pattern: String,
	pub enabled: bool,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleTarget {
	Filename,
	RelPath,
}
impl RuleTarget {
	fn as_str(self) -> &'static str {
		match self {
			Self::Filename => "filename",
			Self::RelPath => "rel_path",
		}
	}
}
fn compile(pattern: &str) -> Result<regex::Regex> {
	if pattern.is_empty() || pattern.len() > 4096 {
		return Err(Error::Invalid("Pattern must contain 1–4096 bytes".into()));
	}
	regex::RegexBuilder::new(pattern)
		.size_limit(1024 * 1024)
		.build()
		.map_err(|e| Error::Invalid(format!("Invalid pattern: {e}")))
}
#[derive(Debug, Serialize)]
pub struct Collection {
	pub id: i64,
	pub name: String,
	pub kind: String,
	pub query: Option<Query>,
	pub count: usize,
	pub error: Option<String>,
}
impl Engine {
	pub fn rules(&self) -> Result<Vec<Rule>> {
		let reader = self.reader()?;
		let mut statement = reader.prepare("WITH RECURSIVE paths(id,path) AS (SELECT id,name FROM tags WHERE parent_id IS NULL UNION ALL SELECT t.id,p.path || '/' || t.name FROM tags t JOIN paths p ON t.parent_id=p.id) SELECT tr.id,p.path,tr.target,tr.pattern,tr.enabled FROM tag_rules tr JOIN paths p ON p.id=tr.tag_id ORDER BY tr.id").map_err(db::Error::from)?;
		statement
			.query_map([], |r| {
				Ok(Rule {
					id: Some(r.get(0)?),
					tag: r.get(1)?,
					target: if r.get::<_, String>(2)? == "filename" {
						RuleTarget::Filename
					} else {
						RuleTarget::RelPath
					},
					pattern: r.get(3)?,
					enabled: r.get(4)?,
				})
			})
			.map_err(db::Error::from)?
			.map(|r| r.map_err(db::Error::from).map_err(Error::from))
			.collect()
	}
	pub fn save_rule(&self, rule: Rule) -> Result<()> {
		let _guard = self.organize_lock.lock().map_err(|_| Error::Busy)?;
		compile(&rule.pattern)?;
		if rule.tag.len() > 512 {
			return Err(Error::Invalid("Tag path is too long".into()));
		}
		self.write(move |tx| {
			let tag = db::ensure_tag(tx, &rule.tag)?;
			if let Some(id) = rule.id {
				if tx.execute(
					"UPDATE tag_rules SET tag_id=?1,target=?2,pattern=?3,enabled=?4 WHERE id=?5",
					(tag, rule.target.as_str(), rule.pattern, rule.enabled, id),
				)? == 0
				{
					return Err(db::Error::Invalid("Rule was not found".into()));
				}
			} else {
				tx.execute(
					"INSERT INTO tag_rules(tag_id,target,pattern,enabled) VALUES(?1,?2,?3,?4)",
					(tag, rule.target.as_str(), rule.pattern, rule.enabled),
				)?;
			}
			Ok(())
		})
	}
	pub fn apply_rules(&self) -> Result<usize> {
		let _guard = self.organize_lock.lock().map_err(|_| Error::Busy)?;
		let rules = self.rules()?;
		let reader = self.reader()?;
		let mut compiled = Vec::new();
		for rule in rules.into_iter().filter(|r| r.enabled) {
			let id = rule.id.expect("Persisted rule has an ID");
			let tag: i64 = reader
				.query_row("SELECT tag_id FROM tag_rules WHERE id=?1", [id], |r| r.get(0))
				.map_err(db::Error::from)?;
			compiled.push((id, tag, rule.target, compile(&rule.pattern)?));
		}
		let mut assignments = HashSet::new();
		if !compiled.is_empty() {
			let mut statement = reader
				.prepare("SELECT sample_id,rel_path FROM files")
				.map_err(db::Error::from)?;
			let files = statement
				.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
				.map_err(db::Error::from)?;
			for file in files {
				let (sample, path) = file.map_err(db::Error::from)?;
				for (rule, tag, target, expression) in &compiled {
					let text = match target {
						RuleTarget::Filename => path.rsplit('/').next().unwrap_or(&path),
						RuleTarget::RelPath => &path,
					};
					if expression.is_match(text) {
						assignments.insert((sample, *tag, *rule));
					}
				}
			}
		}
		let mut assignments: Vec<_> = assignments.into_iter().collect();
		assignments.sort_unstable();
		let (sender, receiver) = mpsc::sync_channel(1);
		self.write(move |tx| {
			let mut affected: HashSet<i64> = tx
				.prepare("SELECT DISTINCT sample_id FROM sample_tags WHERE rule_id IS NOT NULL")?
				.query_map([], |r| r.get(0))?
				.collect::<std::result::Result<_, _>>()?;
			tx.execute("DELETE FROM sample_tags WHERE rule_id IS NOT NULL", [])?;
			let mut count = 0;
			for (sample, tag, rule) in assignments {
				// A manual row wins; deleting only derived rows also lets overlapping enabled rules take over.
				count += tx.execute(
					"INSERT OR IGNORE INTO sample_tags(sample_id,tag_id,rule_id) VALUES(?1,?2,?3)",
					(sample, tag, rule),
				)?;
				affected.insert(sample);
			}
			for sample in affected {
				db::rebuild_search(tx, sample)?;
			}
			let _ = sender.send(count);
			Ok(())
		})?;
		receiver.recv().map_err(|_| Error::WriterStopped)
	}
	pub fn delete_rule(&self, id: i64) -> Result<()> {
		{
			let _guard = self.organize_lock.lock().map_err(|_| Error::Busy)?;
			self.write(move |tx| {
				let affected: Vec<i64> = tx
					.prepare("SELECT sample_id FROM sample_tags WHERE rule_id=?1")?
					.query_map([id], |r| r.get(0))?
					.collect::<std::result::Result<_, _>>()?;
				tx.execute("DELETE FROM tag_rules WHERE id=?1", [id])?;
				for sample in affected {
					db::rebuild_search(tx, sample)?;
				}
				Ok(())
			})?;
		}
		self.apply_rules()?;
		Ok(())
	}
	pub fn collection_query(&self, id: i64) -> Result<Option<Query>> {
		let value: Option<Option<String>> = self
			.reader()?
			.query_row("SELECT query FROM collections WHERE id=?1", [id], |r| r.get(0))
			.optional()
			.map_err(db::Error::from)?;
		value
			.ok_or_else(|| Error::Invalid("Collection was not found".into()))?
			.map(|q| serde_json::from_str(&q).map_err(Error::from))
			.transpose()
	}
	pub fn collection_ids(&self, id: i64) -> Result<HashSet<i64>> {
		if let Some(query) = self.collection_query(id)? {
			return self.query_ids(&query);
		}
		let reader = self.reader()?;
		reader
			.prepare("SELECT sample_id FROM collection_items WHERE collection_id=?1")
			.map_err(db::Error::from)?
			.query_map([id], |r| r.get(0))
			.map_err(db::Error::from)?
			.map(|r| r.map_err(db::Error::from).map_err(Error::from))
			.collect()
	}
	pub fn scope_collection(&self, request: &mut db::BrowseQuery) -> Result<()> {
		if let Some(id) = request.collection_id {
			let mut ids = self.collection_ids(id)?;
			if let Some(existing) = &request.ids {
				let allowed: HashSet<_> = existing.iter().copied().collect();
				ids.retain(|id| allowed.contains(id));
			}
			request.ids = Some(ids.into_iter().collect());
		}
		Ok(())
	}
	pub fn collections(&self) -> Result<Vec<Collection>> {
		let reader = self.reader()?;
		let mut statement = reader
			.prepare("SELECT id,name,kind,query FROM collections ORDER BY id")
			.map_err(db::Error::from)?;
		let rows = statement
			.query_map([], |r| {
				Ok((
					r.get::<_, i64>(0)?,
					r.get::<_, String>(1)?,
					r.get::<_, String>(2)?,
					r.get::<_, Option<String>>(3)?,
				))
			})
			.map_err(db::Error::from)?;
		rows.map(|row| {
			let (id, name, kind, query) = row.map_err(db::Error::from)?;
			let query = query.map(|q| serde_json::from_str(&q)).transpose()?;
			let (count, error) = match self.collection_ids(id) {
				Ok(ids) => (ids.len(), None),
				Err(error) => (0, Some(error.to_string())),
			};
			Ok(Collection {
				id,
				name,
				kind,
				query,
				count,
				error,
			})
		})
		.collect()
	}
	fn validate_collection_query(&self, query: &Query, edited: Option<i64>, depth: usize) -> Result<()> {
		if depth > 32 {
			return Err(Error::Invalid("Collection query nesting exceeds 32 levels".into()));
		}
		match query {
			Query::Collection { id } => {
				if Some(*id) == edited {
					return Err(Error::Invalid("Collections cannot refer to themselves".into()));
				}
				if let Some(query) = self.collection_query(*id)? {
					self.validate_collection_query(&query, edited, depth + 1)?;
				}
			}
			Query::All { conditions } | Query::Any { conditions } => {
				for query in conditions {
					self.validate_collection_query(query, edited, depth + 1)?;
				}
			}
			Query::Not { condition } => self.validate_collection_query(condition, edited, depth + 1)?,
			_ => {}
		}
		Ok(())
	}
	pub fn save_collection(&self, id: Option<i64>, name: String, query: Option<Query>) -> Result<i64> {
		let _guard = self.organize_lock.lock().map_err(|_| Error::Busy)?;
		if name.trim().is_empty() || name.chars().count() > 100 {
			return Err(Error::Invalid("Collection name must contain 1–100 characters".into()));
		}
		if let Some(query) = &query {
			self.validate_collection_query(query, id, 0)?;
			self.query_ids(query)?;
		}
		let kind = if query.is_some() { "smart" } else { "static" };
		let query = query.map(|q| serde_json::to_string(&q)).transpose()?;
		let (sender, receiver) = mpsc::sync_channel(1);
		self.write(move |tx| {
			let id = if let Some(id) = id {
				if tx.execute(
					"UPDATE collections SET name=?1,kind=?2,query=?3 WHERE id=?4",
					(name, kind, query, id),
				)? == 0
				{
					return Err(db::Error::Invalid("Collection was not found".into()));
				}
				id
			} else {
				tx.execute(
					"INSERT INTO collections(name,kind,query) VALUES(?1,?2,?3)",
					(name, kind, query),
				)?;
				tx.last_insert_rowid()
			};
			let _ = sender.send(id);
			Ok(())
		})?;
		receiver.recv().map_err(|_| Error::WriterStopped)
	}
	pub fn edit_collection_items(&self, id: i64, ids: Vec<i64>, remove: bool) -> Result<()> {
		let _guard = self.organize_lock.lock().map_err(|_| Error::Busy)?;
		self.write(move |tx| {
			let kind: String = tx.query_row("SELECT kind FROM collections WHERE id=?1", [id], |r| r.get(0))?;
			if kind != "static" {
				return Err(db::Error::Invalid(
					"Smart collections are determined by their conditions".into(),
				));
			}
			let mut position: f64 = tx.query_row(
				"SELECT COALESCE(MAX(position),0) FROM collection_items WHERE collection_id=?1",
				[id],
				|r| r.get(0),
			)?;
			for sample in ids {
				if remove {
					tx.execute(
						"DELETE FROM collection_items WHERE collection_id=?1 AND sample_id=?2",
						(id, sample),
					)?;
				} else {
					position += 1.0;
					tx.execute(
						"INSERT OR IGNORE INTO collection_items(collection_id,sample_id,position) VALUES(?1,?2,?3)",
						(id, sample, position),
					)?;
				}
			}
			Ok(())
		})
	}
	pub fn move_collection_item(&self, id: i64, sample: i64, earlier: bool) -> Result<()> {
		let _guard = self.organize_lock.lock().map_err(|_| Error::Busy)?;
		self.write(move |tx| {
			let rows: Vec<(i64, f64)> = tx
				.prepare(
					"SELECT sample_id,position FROM collection_items WHERE collection_id=?1 ORDER BY position,sample_id",
				)?
				.query_map([id], |r| Ok((r.get(0)?, r.get(1)?)))?
				.collect::<std::result::Result<_, _>>()?;
			let at = rows
				.iter()
				.position(|r| r.0 == sample)
				.ok_or_else(|| db::Error::Invalid("Sample is not in collection".into()))?;
			let neighbor = if earlier {
				at.checked_sub(1)
			} else {
				(at + 1 < rows.len()).then_some(at + 1)
			};
			if let Some(other) = neighbor {
				for (sample, position) in [(rows[at].0, rows[other].1), (rows[other].0, rows[at].1)] {
					tx.execute(
						"UPDATE collection_items SET position=?1 WHERE collection_id=?2 AND sample_id=?3",
						(position, id, sample),
					)?;
				}
			}
			Ok(())
		})
	}
	pub fn delete_collection(&self, id: i64) -> Result<()> {
		let _guard = self.organize_lock.lock().map_err(|_| Error::Busy)?;
		self.write(move |tx| {
			tx.execute("DELETE FROM collections WHERE id=?1", [id])?;
			Ok(())
		})
	}
}
