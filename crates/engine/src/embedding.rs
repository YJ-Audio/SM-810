use crate::{
	Engine, Error, Result,
	desktop::{Page, Row},
};
use sampler_db as db;
use sampler_embed::{DIMENSIONS, DIRECTORY, MODEL, Model};
use sampler_similarity::{Hit, Store};
use std::{
	collections::{HashMap, HashSet},
	path::{Path, PathBuf},
	sync::mpsc,
};

pub(crate) fn load_store(path: &Path) -> Result<Store> {
	let reader = db::open_reader(path)?;
	let mut store = Store::new(DIMENSIONS);
	let mut statement = reader
		.prepare("SELECT sample_id,dim,vec FROM embeddings WHERE model=?1 ORDER BY sample_id")
		.map_err(db::Error::from)?;
	let rows = statement
		.query_map([MODEL], |row| {
			Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get::<_, Vec<u8>>(2)?))
		})
		.map_err(db::Error::from)?;
	for row in rows {
		let (id, dimensions, bytes) = row.map_err(db::Error::from)?;
		if dimensions != DIMENSIONS as i64 || bytes.len() != DIMENSIONS * 4 {
			return Err(Error::Invalid(format!("Invalid embedding for sample {id}")));
		}
		store.insert(
			id,
			bytes
				.as_chunks::<4>()
				.0
				.iter()
				.map(|v| f32::from_le_bytes(*v))
				.collect(),
		)?;
	}
	Ok(store)
}
impl Engine {
	pub fn model_directory(&self) -> PathBuf {
		std::env::var_os("SAMPLER_MODEL_DIR")
			.map(PathBuf::from)
			.unwrap_or_else(|| self.path.with_file_name("models").join(DIRECTORY))
	}
	pub fn model_ready(&self) -> bool {
		sampler_embed::assets::is_installed(&self.model_directory())
	}
	pub fn embedding_count(&self) -> usize {
		self.vectors.read().map(|v| v.len()).unwrap_or(0)
	}
	pub fn download_model(&self, progress: impl FnMut(u64, u64)) -> Result<()> {
		let _guard = self.embedding_lock.try_lock().map_err(|_| Error::Busy)?;
		sampler_embed::assets::download(&self.model_directory(), progress)?;
		Ok(())
	}
	pub(crate) fn with_model<T>(&self, f: impl FnOnce(&mut Model) -> Result<T>) -> Result<T> {
		let mut model = self
			.model
			.lock()
			.map_err(|_| Error::Invalid("CLAP model lock poisoned".into()))?;
		if model.is_none() {
			*model = Some(Model::open(&self.model_directory())?);
		}
		f(model.as_mut().expect("model initialized above"))
	}
	pub(crate) fn embed_one(&self, id: i64) -> Result<Vec<f32>> {
		if let Some(vector) = self
			.vectors
			.read()
			.map_err(|_| Error::Invalid("Vector store lock poisoned".into()))?
			.vector(id)
		{
			return Ok(vector.to_vec());
		}
		let _scan = self.scan_lock.lock().map_err(|_| Error::Busy)?;
		if let Some(vector) = self
			.vectors
			.read()
			.map_err(|_| Error::Invalid("Vector store lock poisoned".into()))?
			.vector(id)
		{
			return Ok(vector.to_vec());
		}
		let path = self.file_path(id)?;
		let parent = path
			.parent()
			.ok_or_else(|| Error::Invalid("No source directory".into()))?;
		let before = sampler_scan::fingerprint(parent, &path)?;
		let stored = db::hash_files(&self.reader()?, id)?
			.into_iter()
			.find(|f| f.path == path)
			.ok_or_else(|| Error::Invalid("Source not found".into()))?;
		if before.size != stored.size
			|| before.mtime != stored.mtime
			|| before.quick_hash.as_slice() != stored.quick_hash
		{
			return Err(Error::Invalid("Source changed; rescan required".into()));
		}
		let vector = self.with_model(|model| Ok(model.audio_file(&path)?))?;
		let after = sampler_scan::fingerprint(parent, &path)?;
		if before.size != after.size || before.mtime != after.mtime || before.quick_hash != after.quick_hash {
			return Err(Error::Invalid("Source changed during embedding".into()));
		}
		let bytes: Vec<u8> = vector.iter().flat_map(|v| v.to_le_bytes()).collect();
		self.write(move |tx| {
            tx.execute("INSERT INTO embeddings(model,sample_id,dim,vec) VALUES(?1,?2,?3,?4) ON CONFLICT(model,sample_id) DO UPDATE SET dim=excluded.dim,vec=excluded.vec", (MODEL,id,DIMENSIONS as i64,bytes))?;
            tx.execute("UPDATE jobs SET state='done',error=NULL WHERE kind='embed' AND sample_id=?1",[id])?;
            Ok(())
        })?;
		self.vectors
			.write()
			.map_err(|_| Error::Invalid("Vector store lock poisoned".into()))?
			.insert(id, vector.clone())?;
		Ok(vector)
	}
	pub fn embed_pending(&self, limit: usize) -> Result<usize> {
		self.embed_until(limit, || false)
	}
	pub fn embed_until(&self, limit: usize, cancelled: impl Fn() -> bool) -> Result<usize> {
		let _embedding = self.embedding_lock.try_lock().map_err(|_| Error::Busy)?;
		self.with_model(|_| Ok(()))?;
		self.write(|tx| {
            tx.execute("UPDATE jobs SET state='pending',error=NULL WHERE kind='embed' AND state='done' AND sample_id NOT IN (SELECT sample_id FROM embeddings WHERE model=?1)",[MODEL])?;
            Ok(())
        })?;
		let mut processed = 0;
		for _ in 0..limit {
			if cancelled() {
				break;
			}
			let (sender, receiver) = mpsc::sync_channel(1);
			self.write(move |tx| {
				let id = db::claim_embedding_job(tx)?;
				let _ = sender.send(id);
				Ok(())
			})?;
			let Some(id) = receiver.recv().map_err(|_| Error::WriterStopped)? else {
				break;
			};
			let error = self.embed_one(id).err().map(|e| e.to_string());
			self.write(move |tx| {
				db::finish_job(
					tx,
					&db::Job {
						sample_id: id,
						kind: "embed".into(),
					},
					error.as_deref(),
				)
			})?;
			processed += 1;
		}
		Ok(processed)
	}
	pub fn prioritize_embeddings(&self, ids: Vec<i64>) -> Result<()> {
		self.write(move |tx| {
			for id in ids.into_iter().take(180) {
				tx.execute(
					"UPDATE jobs SET priority=10 WHERE sample_id=?1 AND kind IN ('embed','preview_cache') AND state='pending'",
					[id],
				)?;
			}
			Ok(())
		})
	}
	pub fn similar(&self, id: i64, count: usize) -> Result<Vec<Row>> {
		let query = self.embed_one(id)?;
		let candidates: HashSet<_> = db::matching_ids(&self.reader()?, &db::BrowseQuery::default())?
			.into_iter()
			.collect();
		let hits = self
			.vectors
			.read()
			.map_err(|_| Error::Invalid("Vector store lock poisoned".into()))?
			.search(&query, count.min(100), Some(&candidates), Some(id))?;
		self.rows_for_hits(hits)
	}
	pub(crate) fn semantic_browse(&self, mut request: db::BrowseQuery) -> Result<Page> {
		let query = if let Some(text) = request.text.trim_start().strip_prefix('~') {
			let vector = self.with_model(|model| Ok(model.text(text.trim())?))?;
			request.text.clear();
			vector
		} else if let Some(id) = request.similar_to {
			self.embed_one(id)?
		} else {
			return Err(Error::Invalid("No similarity query".into()));
		};
		let candidates: HashSet<_> = db::matching_ids(&self.reader()?, &request)?.into_iter().collect();
		let store = self
			.vectors
			.read()
			.map_err(|_| Error::Invalid("Vector store lock poisoned".into()))?;
		let total = candidates.iter().filter(|id| store.vector(**id).is_some()).count();
		let hits = store
			.search(
				&query,
				request.offset.saturating_add(request.limit.unwrap_or(180)).min(100000),
				Some(&candidates),
				None,
			)?
			.into_iter()
			.skip(request.offset)
			.collect();
		drop(store);
		Ok(Page {
			items: self.rows_for_hits(hits)?,
			total,
		})
	}
	fn rows_for_hits(&self, hits: Vec<Hit>) -> Result<Vec<Row>> {
		if hits.is_empty() {
			return Ok(Vec::new());
		}
		let request = db::BrowseQuery {
			ids: Some(hits.iter().map(|h| h.id).collect()),
			limit: Some(hits.len()),
			..Default::default()
		};
		let reader = self.reader()?;
		let samples = db::browse(&reader, &request)?.items;
		let mut rows: HashMap<_, _> = samples.into_iter().map(|sample| (sample.id, sample)).collect();
		hits.into_iter()
			.filter_map(|hit| rows.remove(&hit.id).map(|sample| (hit, sample)))
			.map(|(hit, sample)| {
				Ok(Row {
					analysis: db::analysis(&reader, sample.id)?,
					peaks: self.peaks_with_reader(&reader, sample.id)?,
					sample,
					similarity: Some(hit.score),
				})
			})
			.collect()
	}
}
