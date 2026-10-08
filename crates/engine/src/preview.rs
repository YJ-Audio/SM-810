use crate::{Engine, Error, Result};
use sampler_db::{self as db, OptionalExtension};
use std::{
	fs::{File, OpenOptions},
	io::{Seek, SeekFrom, Write},
	sync::{
		atomic::{AtomicUsize, Ordering},
		mpsc,
	},
	thread,
};

pub struct Preview {
	pub audio: sampler_decode::Audio,
	pub complete: bool,
}
impl Engine {
	pub fn cache_previews(&self, limit: usize, cancelled: impl Fn() -> bool + Sync) -> Result<usize> {
		let _guard = self.preview_lock.try_lock().map_err(|_| Error::Busy)?;
		let _scan = self.scan_lock.try_lock().map_err(|_| Error::Busy)?;
		let claimed = AtomicUsize::new(0);
		let finished = AtomicUsize::new(0);
		let workers = thread::available_parallelism()
			.map(|n| n.get().saturating_sub(1).max(1))
			.unwrap_or(1);
		thread::scope(|scope| -> Result<()> {
			let mut threads = Vec::new();
			for _ in 0..workers {
				threads.push(scope.spawn(|| -> Result<()> {
                while !cancelled() && claimed.fetch_add(1, Ordering::Relaxed) < limit {
                    let (send, recv) = mpsc::sync_channel(1);
                    self.write(move |tx| {
                        let id = tx.query_row("SELECT sample_id FROM jobs WHERE kind='preview_cache' AND state='pending' ORDER BY priority DESC,sample_id LIMIT 1", [], |r|r.get::<_, i64>(0)).optional()?;
                        if let Some(id) = id { tx.execute("UPDATE jobs SET state='running',attempts=attempts+1 WHERE sample_id=?1 AND kind='preview_cache'", [id])?; }
                        let _ = send.send(id); Ok(())
                    })?;
                    let Some(id) = recv.recv().map_err(|_|Error::WriterStopped)? else { break };
                    let result = self.build_preview(id);
                    let error = result.err().map(|e|e.to_string());
                    self.write(move |tx| db::finish_job(tx, &db::Job { sample_id: id, kind: "preview_cache".into() }, error.as_deref()))?;
                    finished.fetch_add(1, Ordering::Relaxed);
                }
                Ok(())
            }));
			}
			for worker in threads {
				worker
					.join()
					.map_err(|_| Error::Invalid("Preview worker panicked".into()))??;
			}
			Ok(())
		})?;
		Ok(finished.load(Ordering::Relaxed))
	}
	pub fn retry_preview_failures(&self) -> Result<()> {
		self.write(|tx| {
			tx.execute(
				"UPDATE jobs SET state='pending',error=NULL WHERE kind='preview_cache' AND state='failed'",
				[],
			)?;
			Ok(())
		})
	}
	fn build_preview(&self, id: i64) -> Result<()> {
		let file = db::hash_files(&self.reader()?, id)?
			.into_iter()
			.find(|f| f.path.is_file())
			.ok_or_else(|| Error::Invalid("No available file for preview cache".into()))?;
		let parent = file
			.path
			.parent()
			.ok_or_else(|| Error::Invalid("Missing parent path".into()))?;
		let before = sampler_scan::fingerprint(parent, &file.path)?;
		if before.mtime != file.mtime || before.size != file.size || before.quick_hash.as_slice() != file.quick_hash {
			return Err(Error::Invalid("Source changed; rescan required".into()));
		}
		// Read a little beyond the saved head to reliably detect EOF, including exact one-second files.
		let mut audio = sampler_decode::decode_head(&file.path, Some(1.1))?;
		if audio.channels > 32 || audio.sample_rate > 768000 {
			return Err(Error::Invalid("Unsupported preview format".into()));
		}
		let complete = audio.frames() <= audio.sample_rate as usize;
		audio.samples.truncate(audio.sample_rate as usize * audio.channels);
		let after = sampler_scan::fingerprint(parent, &file.path)?;
		if before.mtime != after.mtime || before.quick_hash != after.quick_hash || before.size != after.size {
			return Err(Error::Invalid("Source changed during preview caching".into()));
		}
		let frames = audio.frames();
		let rate = audio.sample_rate;
		let channels = audio.channels;
		let bytes: Vec<u8> = audio
			.samples
			.iter()
			.flat_map(|s| ((*s * 32768.0).round().clamp(-32768.0, 32767.0) as i16).to_le_bytes())
			.collect();
		let pack = self.path.with_file_name("preview.pack");
		self.write(move |tx| {
			// Windows cannot lock an append-only handle; read access enables LockFileEx.
			let mut pack = OpenOptions::new().create(true).read(true).append(true).open(pack)?;
			pack.lock()?;
			let offset = pack.seek(SeekFrom::End(0))?;
			pack.write_all(&bytes)?; pack.sync_data()?;
			tx.execute("INSERT INTO preview_cache(sample_id,pack_offset,frames,sample_rate,channels,complete) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(sample_id) DO UPDATE SET pack_offset=excluded.pack_offset,frames=excluded.frames,sample_rate=excluded.sample_rate,channels=excluded.channels,complete=excluded.complete", (id, offset as i64, frames as i64, rate, channels as i64, complete))?;
			Ok(())
		})
	}
	/// Reads only the local immutable pack; it never stats or opens a library source.
	pub fn preview(&self, id: i64) -> Result<Option<Preview>> {
		let location = self
			.reader()?
			.query_row(
				"SELECT pack_offset,frames,sample_rate,channels,complete FROM preview_cache WHERE sample_id=?1",
				[id],
				|r| {
					Ok((
						r.get::<_, i64>(0)?,
						r.get::<_, u32>(1)?,
						r.get::<_, u32>(2)?,
						r.get::<_, u32>(3)?,
						r.get::<_, bool>(4)?,
					))
				},
			)
			.optional()
			.map_err(db::Error::from)?;
		let Some((offset, frames, rate, channels, complete)) = location else {
			return Ok(None);
		};
		let offset = usize::try_from(offset).map_err(|_| Error::Invalid("Invalid preview offset".into()))?;
		let frames = frames as usize;
		let channels = channels as usize;
		if !(1..=32).contains(&channels) || rate == 0 || rate > 768000 || frames == 0 || frames > rate as usize {
			return Err(Error::Invalid("Invalid preview cache metadata".into()));
		}
		let end = offset
			.checked_add(frames * channels * 2)
			.ok_or_else(|| Error::Invalid("Preview offset overflow".into()))?;
		let mut mapping = self.preview_map.lock().map_err(|_| Error::Busy)?;
		if mapping.as_ref().is_none_or(|map| map.len() < end) {
			let file = File::open(self.path.with_file_name("preview.pack"))?;
			if file.metadata()?.len() < end as u64 {
				return Err(Error::Invalid("Preview pack is truncated".into()));
			}
			// SAFETY: engine writers serialize appends with an OS file lock. No engine path truncates
			// or modifies mapped regions; the single DB writer fsyncs before publishing references.
			*mapping = Some(unsafe { memmap2::MmapOptions::new().map(&file)? });
		}
		let bytes = &mapping.as_ref().unwrap()[offset..end];
		let samples = bytes
			.as_chunks::<2>()
			.0
			.iter()
			.map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0)
			.collect();
		Ok(Some(Preview {
			audio: sampler_decode::Audio {
				samples,
				sample_rate: rate,
				channels,
				bits_per_sample: Some(16),
				container_frames: complete.then_some(frames as u64),
			},
			complete,
		}))
	}
}
