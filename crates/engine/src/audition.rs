use crate::{Engine, Error, Result};
use sampler_audio::{Buffer, Output, Position, StreamBuffer, StreamWriter, matched_gain};
use serde::{Deserialize, Serialize};
use std::{
	collections::VecDeque,
	sync::{
		Arc, Mutex,
		atomic::{AtomicU64, Ordering},
		mpsc::{self, SyncSender},
	},
	thread::{self, JoinHandle},
	time::Duration,
};

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(default)]
pub struct Settings {
	pub semitones: i8,
	pub match_lufs: bool,
	pub target_lufs: f64,
}
impl Default for Settings {
	fn default() -> Self {
		Self {
			semitones: 0,
			match_lufs: true,
			target_lufs: -16.0,
		}
	}
}
struct Request {
	id: i64,
	prefetch: bool,
	settings: Settings,
	revision: u64,
}
#[derive(Serialize)]
pub struct Status {
	pub sample_id: u64,
	pub seconds: f64,
	pub playing: bool,
	pub error: Option<String>,
}
pub struct Auditioner {
	prefetch_sender: Option<SyncSender<Request>>,
	sender: Option<SyncSender<Request>>,
	thread: Option<JoinHandle<()>>,
	revision: Arc<AtomicU64>,
	position: Arc<Position>,
	rate: u32,
	error: Arc<Mutex<Option<String>>>,
	output: Arc<Mutex<Output>>,
}
impl Auditioner {
	pub fn open(engine: Arc<Engine>) -> Result<Self> {
		let output = Output::open()?;
		let rate = output.sample_rate;
		let position = output.position.clone();
		let output = Arc::new(Mutex::new(output));
		let device = output.clone();
		let (sender, receiver) = mpsc::sync_channel::<Request>(32);
		let (prefetch_sender, prefetch_receiver) = mpsc::sync_channel::<Request>(8);
		let revision = Arc::new(AtomicU64::new(0));
		let current = revision.clone();
		let error = Arc::new(Mutex::new(None));
		let errors = error.clone();
		let thread = thread::Builder::new()
			.name("sampler-audition-loader".into())
			.spawn(move || {
				let mut cache = PreviewCache::default();
				loop {
					let mut request = match receiver.recv_timeout(Duration::from_millis(10)) {
						Ok(request) => request,
						Err(mpsc::RecvTimeoutError::Disconnected) => break,
						Err(mpsc::RecvTimeoutError::Timeout) => match prefetch_receiver.try_recv() {
							Ok(request) => request,
							Err(_) => continue,
						},
					};
					for newer in receiver.try_iter() {
						if !newer.prefetch || request.prefetch {
							request = newer;
						}
					}
					let result = (|| -> Result<()> {
						if current.load(Ordering::Acquire) != request.revision {
							return Ok(());
						}
						let id = request.id;
						let key = (id, request.settings.semitones);
						let cached = cache.get(key);
						if request.prefetch && cached.is_some() {
							return Ok(());
						}
						let analysis = engine.analysis(id)?;
						let gain = if request.settings.match_lufs {
							matched_gain(
								analysis.as_ref().and_then(|a| a.lufs),
								analysis.as_ref().and_then(|a| a.peak_dbfs),
								request.settings.target_lufs,
							)
						} else {
							1.0
						};
						if let Some((buffer, true)) = &cached {
							let mut output = device
								.lock()
								.map_err(|_| Error::Invalid("Audio control lock poisoned".into()))?;
							if current.load(Ordering::Acquire) == request.revision {
								output.play(id as u64, buffer.clone(), gain)?;
							}
							return Ok(());
						}
						let mut writer: Option<StreamWriter> = None;
						let mut head = Vec::new();
						let mut channels = 0;
						let mut skip = 0;
						let mut complete = true;
						let mut output_error = None;
						if let Some((buffer, _)) = &cached {
							let (mut producer, stream) = StreamBuffer::bounded(buffer.channels, rate as usize * 2)?;
							skip = producer.write(&buffer.samples);
							let mut output = device
								.lock()
								.map_err(|_| Error::Invalid("Audio control lock poisoned".into()))?;
							if current.load(Ordering::Acquire) != request.revision {
								return Ok(());
							}
							output.stream(id as u64, stream, gain)?;
							writer = Some(producer);
						}
						sampler_decode::stream(&engine.file_path(id)?, rate, request.settings.semitones, |packet| {
							if current.load(Ordering::Acquire) != request.revision {
								complete = false;
								return false;
							}
							channels = packet.channels;
							let head_limit = rate as usize * channels;
							let count = packet.samples.len().min(head_limit.saturating_sub(head.len()));
							head.extend_from_slice(&packet.samples[..count]);
							if request.prefetch {
								if head.len() == head_limit {
									complete = false;
									return false;
								}
								return true;
							}
							let skipped = skip.min(packet.samples.len());
							skip -= skipped;
							let samples = &packet.samples[skipped..];
							if samples.is_empty() {
								return true;
							}
							let result = (|| -> Result<bool> {
								if writer.is_none() {
									let (mut producer, stream) = StreamBuffer::bounded(channels, rate as usize * 2)?;
									let seeded = producer.write(samples);
									let mut output = device
										.lock()
										.map_err(|_| Error::Invalid("Audio control lock poisoned".into()))?;
									if current.load(Ordering::Acquire) != request.revision {
										return Ok(false);
									}
									output.stream(id as u64, stream, gain)?;
									drop(output);
									writer = Some(producer);
									return Ok(write_stream(
										writer.as_mut().unwrap(),
										&samples[seeded..],
										&current,
										request.revision,
									));
								}
								Ok(write_stream(
									writer.as_mut().unwrap(),
									samples,
									&current,
									request.revision,
								))
							})();
							match result {
								Ok(true) => true,
								Ok(false) => {
									complete = false;
									false
								}
								Err(error) => {
									output_error = Some(error);
									complete = false;
									false
								}
							}
						})?;
						drop(writer);
						if !head.is_empty() {
							// A full one-second head may have truncated a longer sound.
							let complete = complete && head.len() < rate as usize * channels;
							cache.insert(
								key,
								Arc::new(Buffer {
									samples: head.into_boxed_slice(),
									channels,
								}),
								complete,
							);
						}
						if let Some(error) = output_error {
							return Err(error);
						}
						Ok(())
					})();
					if !request.prefetch
						&& current.load(Ordering::Acquire) == request.revision
						&& let Ok(mut error) = errors.lock()
					{
						*error = result.err().map(|error| error.to_string());
					}
				}
			})?;
		Ok(Self {
			prefetch_sender: Some(prefetch_sender),
			sender: Some(sender),
			thread: Some(thread),
			revision,
			position,
			rate,
			error,
			output,
		})
	}
	pub fn play(&self, id: Option<i64>, settings: Settings) -> Result<()> {
		if !(-24..=24).contains(&settings.semitones)
			|| !settings.target_lufs.is_finite()
			|| !(-36.0..=-6.0).contains(&settings.target_lufs)
		{
			return Err(Error::Invalid("Invalid audition settings".into()));
		}
		let revision = self.revision.fetch_add(1, Ordering::AcqRel) + 1;
		if id.is_none() {
			self.output
				.lock()
				.map_err(|_| Error::Invalid("Audio control lock poisoned".into()))?
				.stop()?;
			return Ok(());
		}
		self.sender
			.as_ref()
			.ok_or(Error::WriterStopped)?
			.try_send(Request {
				id: id.unwrap(),
				settings,
				revision,
				prefetch: false,
			})
			.map_err(|_| Error::Busy)
	}
	pub fn prefetch(&self, ids: Vec<i64>, settings: Settings) {
		if !(-24..=24).contains(&settings.semitones) {
			return;
		}
		let revision = self.revision.load(Ordering::Acquire);
		if let Some(sender) = &self.prefetch_sender {
			for id in ids.into_iter().take(8) {
				let _ = sender.try_send(Request {
					id,
					settings,
					revision,
					prefetch: true,
				});
			}
		}
	}
	pub fn status(&self) -> Status {
		let error = if self.position.device_error.load(Ordering::Acquire) {
			Some("Audio output device disconnected. Restart the application to reconnect.".into())
		} else {
			self.error.lock().ok().and_then(|error| error.clone())
		};
		Status {
			sample_id: self.position.sample_id.load(Ordering::Relaxed),
			seconds: self.position.frame.load(Ordering::Relaxed) as f64 / self.rate as f64,
			playing: self.position.playing.load(Ordering::Acquire),
			error,
		}
	}
}
impl Drop for Auditioner {
	fn drop(&mut self) {
		self.revision.fetch_add(1, Ordering::Release);
		self.sender.take();
		self.prefetch_sender.take();
		if let Some(thread) = self.thread.take() {
			let _ = thread.join();
		}
	}
}

fn write_stream(writer: &mut StreamWriter, mut samples: &[f32], current: &AtomicU64, revision: u64) -> bool {
	while !samples.is_empty() {
		if current.load(Ordering::Acquire) != revision {
			return false;
		}
		let written = writer.write(samples);
		samples = &samples[written..];
		if written == 0 {
			thread::sleep(Duration::from_millis(1));
		}
	}
	true
}
#[derive(Default)]
struct PreviewCache {
	entries: VecDeque<((i64, i8), Arc<Buffer>, bool)>,
	bytes: usize,
}
impl PreviewCache {
	fn get(&mut self, key: (i64, i8)) -> Option<(Arc<Buffer>, bool)> {
		let index = self.entries.iter().position(|entry| entry.0 == key)?;
		let entry = self.entries.remove(index)?;
		let result = (entry.1.clone(), entry.2);
		self.entries.push_back(entry);
		Some(result)
	}
	fn insert(&mut self, key: (i64, i8), buffer: Arc<Buffer>, complete: bool) {
		if let Some(index) = self.entries.iter().position(|entry| entry.0 == key) {
			self.bytes -= self.entries.remove(index).unwrap().1.samples.len() * 4;
		}
		let size = buffer.samples.len() * 4;
		const LIMIT: usize = 128 * 1024 * 1024;
		if size > LIMIT {
			return;
		}
		while self.bytes + size > LIMIT {
			if let Some(entry) = self.entries.pop_front() {
				self.bytes -= entry.1.samples.len() * 4;
			} else {
				break;
			}
		}
		self.bytes += size;
		self.entries.push_back((key, buffer, complete));
	}
}
