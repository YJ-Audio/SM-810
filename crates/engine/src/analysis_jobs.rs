use crate::{Engine, Error, Result};
use sampler_db as db;
use std::{
	fs::OpenOptions,
	io::{Read, Seek, SeekFrom, Write},
	sync::{
		atomic::{AtomicUsize, Ordering},
		mpsc,
	},
	thread,
};

impl Engine {
	pub fn analyze_pending(&self, limit: usize) -> Result<usize> {
		let _guard = self.scan_lock.try_lock().map_err(|_| Error::Busy)?;
		self.write(|tx| db::requeue_old_analysis(tx, sampler_analysis::ANALYZER_VERSION))?;
		let claimed = AtomicUsize::new(0);
		let finished = AtomicUsize::new(0);
		let workers = thread::available_parallelism()
			.map(|n| n.get().saturating_sub(1).max(1))
			.unwrap_or(1);
		thread::scope(|scope| -> Result<()> {
			let mut threads = Vec::new();
			for _ in 0..workers {
				threads.push(scope.spawn(|| -> Result<()> {
					loop {
						if claimed.fetch_add(1, Ordering::Relaxed) >= limit {
							break;
						}
						let (send, recv) = mpsc::sync_channel(1);
						self.write(move |tx| {
							let job = db::claim_analysis_job(tx)?;
							let _ = send.send(job);
							Ok(())
						})?;
						let Some(job) = recv.recv().map_err(|_| Error::WriterStopped)? else {
							break;
						};
						let outcome = self.analyze_job(&job);
						let error = outcome.err().map(|e| e.to_string());
						self.write(move |tx| db::finish_job(tx, &job, error.as_deref()))?;
						finished.fetch_add(1, Ordering::Relaxed);
					}
					Ok(())
				}));
			}
			for worker in threads {
				worker
					.join()
					.map_err(|_| Error::Invalid("analysis worker panicked".into()))??;
			}
			Ok(())
		})?;
		Ok(finished.load(Ordering::Relaxed))
	}
	fn analyze_job(&self, job: &db::Job) -> Result<()> {
		let file = db::hash_files(&self.reader()?, job.sample_id)?
			.into_iter()
			.find(|file| file.path.is_file())
			.ok_or_else(|| Error::Invalid("no available file for analysis".into()))?;
		let parent = file
			.path
			.parent()
			.ok_or_else(|| Error::Invalid("missing parent path".into()))?;
		let before = sampler_scan::fingerprint(parent, &file.path)?;
		if before.mtime != file.mtime || before.size != file.size || before.quick_hash.as_slice() != file.quick_hash {
			return Err(Error::Invalid("source changed; rescan required".into()));
		}
		let audio = sampler_decode::decode(&file.path)?;
		let analysis = if job.kind == "analyze" {
			Some(sampler_analysis::analyze(&file.path, &audio)?)
		} else {
			None
		};
		let peaks = sampler_analysis::peaks(&audio, 256);
		let after = sampler_scan::fingerprint(parent, &file.path)?;
		if before.mtime != after.mtime || before.quick_hash != after.quick_hash || before.size != after.size {
			return Err(Error::Invalid("source changed during analysis".into()));
		}
		let analysis: Option<db::AnalysisRecord> = analysis
			.map(|a| serde_json::from_value(serde_json::to_value(a)?))
			.transpose()?;
		let pack = self.path.with_file_name("peaks.pack");
		let id = job.sample_id;
		self.write(move |tx|{
            let mut file=OpenOptions::new().create(true).append(true).open(pack)?;
            let offset=file.seek(SeekFrom::End(0))?;
            file.write_all(&peaks)?;file.sync_data()?;
            tx.execute("INSERT INTO waveform_peaks(sample_id,pack_offset,bins) VALUES(?1,?2,256) ON CONFLICT(sample_id) DO UPDATE SET pack_offset=excluded.pack_offset,bins=excluded.bins",(id,offset as i64))?;
            if let Some(analysis)=analysis {db::store_analysis(tx,id,&analysis)?;}
            tx.execute("UPDATE jobs SET state='done',error=NULL WHERE sample_id=?1 AND kind='peaks'",[id])?;
            Ok(())
        })
	}
	pub fn peaks(&self, id: i64) -> Result<Vec<i8>> {
		let db = self.reader()?;
		use sampler_db::OptionalExtension;
		let location: Option<(u64, usize)> = db
			.query_row(
				"SELECT pack_offset,bins FROM waveform_peaks WHERE sample_id=?1",
				[id],
				|r| Ok((r.get::<_, i64>(0)? as u64, r.get::<_, u32>(1)? as usize)),
			)
			.optional()
			.map_err(db::Error::from)?;
		let Some((offset, bins)) = location else {
			return Ok(Vec::new());
		};
		let mut file = std::fs::File::open(self.path.with_file_name("peaks.pack"))?;
		file.seek(SeekFrom::Start(offset))?;
		let mut bytes = vec![0u8; bins * 2];
		file.read_exact(&mut bytes)?;
		Ok(bytes.into_iter().map(|byte| byte as i8).collect())
	}
	pub fn set_manual(&self, id: i64, bpm: Option<f64>, key: Option<u8>, mode: Option<String>) -> Result<()> {
		self.write(move |tx| db::set_manual(tx, id, bpm, key, mode.as_deref()))
	}
	pub fn analysis(&self, id: i64) -> Result<Option<db::AnalysisRecord>> {
		Ok(db::analysis(&self.reader()?, id)?)
	}
	pub fn retry_failed(&self) -> Result<()> {
		self.write(|tx| {
			tx.execute("UPDATE jobs SET state='pending',error=NULL WHERE state='failed'", [])?;
			Ok(())
		})
	}
}
