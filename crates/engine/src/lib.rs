use sampler_db::{self as db, Root, Sample, Storage};
use sampler_scan as scan;
use serde::Serialize;
use std::{
	fs::{File, OpenOptions},
	path::{Path, PathBuf},
	sync::{
		Mutex, RwLock,
		mpsc::{self, SyncSender},
	},
	thread::{self, JoinHandle},
	time::Instant,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
	#[error(transparent)]
	Layout(#[from] sampler_layout::Error),
	#[error(transparent)]
	Embed(#[from] sampler_embed::Error),
	#[error(transparent)]
	Similarity(#[from] sampler_similarity::Error),
	#[error(transparent)]
	Audio(#[from] sampler_audio::Error),
	#[error("WAV export: {0}")]
	Wav(#[from] hound::Error),
	#[error(transparent)]
	Decode(#[from] sampler_decode::Error),
	#[error(transparent)]
	Analysis(#[from] sampler_analysis::Error),
	#[error("serialization: {0}")]
	Json(#[from] serde_json::Error),
	#[error(transparent)]
	Database(#[from] db::Error),
	#[error(transparent)]
	Scan(#[from] scan::Error),
	#[error("I/O: {0}")]
	Io(#[from] std::io::Error),
	#[error("writer thread stopped")]
	WriterStopped,
	#[error("another library operation is already running")]
	Busy,
	#[error("invalid source: {0}")]
	Invalid(String),
}
pub type Result<T> = std::result::Result<T, Error>;
type Write = Box<dyn FnOnce(&db::Transaction<'_>) -> db::Result<()> + Send>;
struct Request {
	write: Write,
	reply: SyncSender<db::Result<()>>,
}

pub struct Engine {
	vectors: RwLock<sampler_similarity::Store>,
	model: Mutex<Option<sampler_embed::Model>>,
	embedding_lock: Mutex<()>,
	map_lock: Mutex<()>,
	organize_lock: Mutex<()>,
	preview_lock: Mutex<()>,
	preview_map: Mutex<Option<memmap2::Mmap>>,
	_process_lock: File,
	path: PathBuf,
	sender: Option<SyncSender<Request>>,
	writer: Option<JoinHandle<()>>,
	scan_lock: Mutex<()>,
}

impl Engine {
	pub fn open(path: impl AsRef<Path>) -> Result<Self> {
		let path = path.as_ref().to_path_buf();
		if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
			std::fs::create_dir_all(parent)?;
		}
		let mut lock_path = path.as_os_str().to_owned();
		lock_path.push(".lock");
		let process_lock = OpenOptions::new()
			.create(true)
			.truncate(false)
			.read(true)
			.write(true)
			.open(lock_path)?;
		process_lock.try_lock().map_err(|error| {
			Error::Invalid(format!(
				"Cannot exclusively open library {}; close other Sampler or CLI instances: {error}",
				path.display()
			))
		})?;
		let (sender, receiver) = mpsc::sync_channel::<Request>(32);
		let (ready_tx, ready_rx) = mpsc::sync_channel(1);
		let writer_path = path.clone();
		let writer = thread::Builder::new().name("sampler-db-writer".into()).spawn(move || {
			let mut connection = match db::open_writer(&writer_path) {
				Ok(connection) => {
					let _ = ready_tx.send(Ok(()));
					connection
				}
				Err(error) => {
					let _ = ready_tx.send(Err(error));
					return;
				}
			};
			while let Ok(request) = receiver.recv() {
				let mut batch = vec![request];
				batch.extend(receiver.try_iter().take(31));
				let outcome = (|| -> db::Result<()> {
					let tx = connection.transaction()?;
					for request in &mut batch {
						let write = std::mem::replace(&mut request.write, Box::new(|_| Ok(())));
						write(&tx)?;
					}
					tx.commit()?;
					Ok(())
				})();
				let failure = outcome.err().map(|error| error.to_string());
				for request in batch {
					let result = failure
						.as_ref()
						.map_or(Ok(()), |message| Err(db::Error::Invalid(message.clone())));
					let _ = request.reply.send(result);
				}
			}
		})?;
		ready_rx.recv().map_err(|_| Error::WriterStopped)??;
		let vectors = embedding::load_store(&path)?;
		Ok(Self {
			vectors: RwLock::new(vectors),
			model: Mutex::new(None),
			embedding_lock: Mutex::new(()),
			map_lock: Mutex::new(()),
			organize_lock: Mutex::new(()),
			preview_lock: Mutex::new(()),
			preview_map: Mutex::new(None),
			_process_lock: process_lock,
			path,
			sender: Some(sender),
			writer: Some(writer),
			scan_lock: Mutex::new(()),
		})
	}

	pub fn write(&self, write: impl FnOnce(&db::Transaction<'_>) -> db::Result<()> + Send + 'static) -> Result<()> {
		let (reply, rx) = mpsc::sync_channel(1);
		self.sender
			.as_ref()
			.ok_or(Error::WriterStopped)?
			.send(Request {
				write: Box::new(write),
				reply,
			})
			.map_err(|_| Error::WriterStopped)?;
		rx.recv().map_err(|_| Error::WriterStopped)??;
		Ok(())
	}

	pub fn reader(&self) -> Result<db::Connection> {
		Ok(db::open_reader(&self.path)?)
	}
	pub fn roots(&self) -> Result<Vec<Root>> {
		Ok(db::roots(&self.reader()?)?)
	}
	pub fn search(&self, text: &str, limit: usize, offset: usize) -> Result<Vec<Sample>> {
		Ok(db::search(&self.reader()?, text, limit, offset)?)
	}
	pub fn add_root(&self, path: &Path, label: &str, storage: Storage) -> Result<i64> {
		let path = std::fs::canonicalize(path)?;
		if !path.is_dir() {
			return Err(Error::Invalid("source must be a directory".into()));
		}
		let text = path
			.to_str()
			.ok_or_else(|| Error::Invalid("source path must be UTF-8".into()))?
			.to_owned();
		let label = label.to_owned();
		self.write(move |tx| {
			db::add_root(tx, &text, &label, storage)?;
			Ok(())
		})?;
		self.roots()?
			.into_iter()
			.find(|root| root.path == path)
			.map(|root| root.id)
			.ok_or_else(|| Error::Invalid("source was not registered".into()))
	}
	pub fn relocate_root(&self, id: i64, path: &Path) -> Result<()> {
		let path = std::fs::canonicalize(path)?;
		if !path.is_dir() {
			return Err(Error::Invalid("source must be a directory".into()));
		}
		let text = path
			.to_str()
			.ok_or_else(|| Error::Invalid("source path must be UTF-8".into()))?
			.to_owned();
		self.write(move |tx| {
			db::relocate_root(tx, id, &text)?;
			let mut stmt = tx.prepare("SELECT DISTINCT sample_id FROM files WHERE root_id=?1")?;
			let ids = stmt
				.query_map([id], |r| r.get::<_, i64>(0))?
				.collect::<std::result::Result<Vec<_>, _>>()?;
			for id in ids {
				db::rebuild_search(tx, id)?;
			}
			Ok(())
		})
	}
	pub fn tag(&self, sample: i64, name: &str) -> Result<()> {
		let name = name.to_owned();
		self.write(move |tx| db::tag(tx, sample, &name))
	}
	pub fn jobs(&self) -> Result<Vec<db::JobCount>> {
		Ok(db::jobs(&self.reader()?)?)
	}

	pub fn scan(&self, id: i64) -> Result<ScanReport> {
		let _guard = self.scan_lock.try_lock().map_err(|_| Error::Busy)?;
		let start = Instant::now();
		let root = self
			.roots()?
			.into_iter()
			.find(|root| root.id == id && root.enabled)
			.ok_or_else(|| Error::Invalid("source is missing or disabled".into()))?;
		self.write(move |tx| {
			db::begin_scan(tx, id)?;
			Ok(())
		})?;
		let generation = self
			.reader()?
			.query_row("SELECT next_generation FROM root_state WHERE root_id=?1", [id], |r| {
				r.get::<_, i64>(0)
			})
			.map_err(db::Error::from)?;
		if let Err(error) = std::fs::read_dir(&root.path) {
			let status = if error.kind() == std::io::ErrorKind::NotFound {
				"offline"
			} else {
				"partial"
			};
			let message = error.to_string();
			self.write(move |tx| db::finish_scan(tx, id, generation, status, Some(&message)))?;
			return Ok(ScanReport {
				root_id: id,
				files: 0,
				seconds: start.elapsed().as_secs_f64(),
				status: status.into(),
				errors: vec![error.to_string()],
			});
		}
		let needs_cache = root.storage != "local";
		let mut batch = Vec::with_capacity(256);
		let mut failed = None;
		let mut report = scan::walk(&root.path, |entry| {
			batch.push(entry);
			if batch.len() >= 256
				&& let Err(error) = self.store_batch(id, generation, needs_cache, std::mem::take(&mut batch))
			{
				failed = Some(error);
				return false;
			}
			true
		});
		if let Some(error) = failed {
			report.errors.push(error.to_string());
		}
		if !batch.is_empty() {
			self.store_batch(id, generation, needs_cache, batch)?;
		}
		let status = if report.errors.is_empty() { "online" } else { "partial" };
		let message = (!report.errors.is_empty()).then(|| report.errors.join("\n"));
		self.write(move |tx| db::finish_scan(tx, id, generation, status, message.as_deref()))?;
		self.apply_rules()?;
		Ok(ScanReport {
			root_id: id,
			files: report.files,
			seconds: start.elapsed().as_secs_f64(),
			status: status.into(),
			errors: report.errors,
		})
	}

	fn store_batch(&self, id: i64, generation: i64, cache: bool, batch: Vec<scan::Entry>) -> Result<()> {
		self.write(move |tx| {
			for file in batch {
				db::upsert_file(
					tx,
					id,
					generation,
					db::FileInput {
						rel_path: &file.rel_path,
						size: file.size,
						mtime: file.mtime,
						quick_hash: &file.quick_hash,
						preview_cache: cache || file.compressed,
					},
				)?;
			}
			Ok(())
		})
	}

	pub fn verify(&self, limit: usize) -> Result<usize> {
		let _guard = self.scan_lock.try_lock().map_err(|_| Error::Busy)?;
		let mut processed = 0;
		for _ in 0..limit {
			let (sender, receiver) = mpsc::sync_channel(1);
			self.write(move |tx| {
				let id = db::claim_hash_job(tx)?;
				let _ = sender.send(id);
				Ok(())
			})?;
			let Some(id) = receiver.recv().map_err(|_| Error::WriterStopped)? else {
				break;
			};
			let files = db::hash_files(&self.reader()?, id)?;
			let mut errors = Vec::new();
			if files.is_empty() {
				errors.push("No available file; rescan when source is online".into());
			}
			for file in files {
				let checked = (|| -> Result<[u8; 16]> {
					let parent = file
						.path
						.parent()
						.ok_or_else(|| Error::Invalid("file has no parent".into()))?;
					let before = scan::fingerprint(parent, &file.path)?;
					if before.mtime != file.mtime
						|| before.size != file.size
						|| before.quick_hash.as_slice() != file.quick_hash
					{
						return Err(Error::Invalid("file changed since scan; rescan required".into()));
					}
					let hash = scan::full_hash(&file.path)?;
					let after = scan::fingerprint(parent, &file.path)?;
					if before.mtime != after.mtime || before.quick_hash != after.quick_hash || before.size != after.size
					{
						return Err(Error::Invalid(
							"file changed during verification; rescan required".into(),
						));
					}
					Ok(hash)
				})();
				match checked {
					Ok(hash) => self.write(move |tx| {
						db::store_hash(tx, file.id, id, &hash)?;
						Ok(())
					})?,
					Err(error) => errors.push(error.to_string()),
				}
			}
			let error = (!errors.is_empty()).then(|| errors.join("\n"));
			self.write(move |tx| db::finish_hash_job(tx, id, error.as_deref()))?;
			processed += 1;
		}
		*self
			.vectors
			.write()
			.map_err(|_| Error::Invalid("Vector store lock poisoned".into()))? = embedding::load_store(&self.path)?;
		Ok(processed)
	}

	pub fn watch(&self, id: i64) -> Result<scan::Watch> {
		let root = self
			.roots()?
			.into_iter()
			.find(|root| root.id == id)
			.ok_or_else(|| Error::Invalid("unknown source".into()))?;
		Ok(scan::Watch::new(&root.path)?)
	}
}
pub mod organize;

impl Drop for Engine {
	fn drop(&mut self) {
		self.sender.take();
		if let Some(writer) = self.writer.take() {
			let _ = writer.join();
		}
	}
}

#[derive(Debug, Serialize)]
pub struct ScanReport {
	pub root_id: i64,
	pub files: usize,
	pub seconds: f64,
	pub status: String,
	pub errors: Vec<String>,
}

mod analysis_jobs;

pub mod audition;
mod desktop;

mod embedding;

pub mod query;

pub mod maps;

pub mod preview;
