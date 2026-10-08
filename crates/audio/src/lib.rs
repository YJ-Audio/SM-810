use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use rtrb::{Consumer, Producer, RingBuffer};
use std::{
	sync::{
		Arc,
		atomic::{AtomicBool, AtomicU64, Ordering},
		mpsc,
	},
	thread::{self, JoinHandle},
	time::{Duration, Instant},
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
	#[error("audio device: {0}")]
	Device(String),
	#[error("audio command queue is full")]
	QueueFull,
	#[error("invalid audio buffer")]
	InvalidBuffer,
}

#[derive(Debug)]
pub struct Buffer {
	pub samples: Box<[f32]>,
	pub channels: usize,
}
impl Buffer {
	pub fn frames(&self) -> usize {
		self.samples.len() / self.channels.max(1)
	}
}
#[derive(Default)]
pub struct Position {
	pub sample_id: AtomicU64,
	pub frame: AtomicU64,
	pub playing: AtomicBool,
	pub device_error: AtomicBool,
	pub onset_token: AtomicU64,
	pub onset_micros: AtomicU64,
}
#[derive(Clone, Copy)]
pub struct Onset {
	pub token: u64,
	pub requested: Instant,
	pub prior_micros: u64,
}
pub struct StreamWriter {
	samples: Producer<f32>,
	done: Arc<AtomicBool>,
}
impl StreamWriter {
	/// Writes only available space; the non-realtime loader handles backpressure.
	pub fn write(&mut self, samples: &[f32]) -> usize {
		let count = samples.len().min(self.samples.slots());
		for &sample in &samples[..count] {
			let _ = self.samples.push(sample);
		}
		count
	}
}
impl Drop for StreamWriter {
	fn drop(&mut self) {
		self.done.store(true, Ordering::Release);
	}
}
pub struct StreamBuffer {
	samples: Consumer<f32>,
	frame: Vec<f32>,
	done: Arc<AtomicBool>,
}
impl StreamBuffer {
	pub fn bounded(channels: usize, capacity_frames: usize) -> Result<(StreamWriter, Box<Self>), Error> {
		if channels == 0 || channels > 32 || capacity_frames == 0 {
			return Err(Error::InvalidBuffer);
		}
		let (writer, reader) = RingBuffer::new(channels * capacity_frames);
		let done = Arc::new(AtomicBool::new(false));
		Ok((
			StreamWriter {
				samples: writer,
				done: done.clone(),
			},
			Box::new(Self {
				samples: reader,
				frame: vec![0.0; channels],
				done,
			}),
		))
	}
}
pub enum Source {
	Buffered(Arc<Buffer>),
	Streaming(Box<StreamBuffer>),
}
pub enum Command {
	Play {
		id: u64,
		buffer: Arc<Buffer>,
		gain: f32,
		onset: Option<Onset>,
	},
	Stream {
		id: u64,
		stream: Box<StreamBuffer>,
		gain: f32,
		onset: Option<Onset>,
	},
	Stop,
}
struct Voice {
	id: u64,
	source: Source,
	frame: usize,
	gain: f32,
	onset: Option<Onset>,
}
impl Voice {
	fn prepare_frame(&mut self) -> bool {
		if let Source::Streaming(stream) = &mut self.source {
			if stream.samples.slots() < stream.frame.len() {
				stream.frame.fill(0.0);
				return false;
			}
			for value in &mut stream.frame {
				*value = stream.samples.pop().unwrap_or(0.0);
			}
		}
		true
	}
	fn finished(&self) -> bool {
		match &self.source {
			Source::Buffered(buffer) => self.frame >= buffer.frames(),
			Source::Streaming(stream) => {
				stream.done.load(Ordering::Acquire) && stream.samples.slots() < stream.frame.len()
			}
		}
	}
	fn sample(&self, channel: usize, output_channels: usize) -> f32 {
		let (samples, channels) = match &self.source {
			Source::Buffered(buffer) => {
				let offset = self.frame.saturating_mul(buffer.channels);
				(
					buffer
						.samples
						.get(offset..offset.saturating_add(buffer.channels))
						.unwrap_or(&[]),
					buffer.channels,
				)
			}
			Source::Streaming(stream) => (stream.frame.as_slice(), stream.frame.len()),
		};
		if channels == 0 || samples.is_empty() {
			return 0.0;
		}
		if output_channels == 1 && channels > 1 {
			samples.iter().sum::<f32>() / channels as f32 * self.gain
		} else {
			samples.get(channel % channels).copied().unwrap_or(0.0) * self.gain
		}
	}
}

pub struct Mixer {
	commands: Consumer<Command>,
	retired: Producer<Source>,
	current: Option<Voice>,
	previous: Option<Voice>,
	fade: usize,
	fade_frames: usize,
	channels: usize,
	position: Arc<Position>,
}
impl Mixer {
	pub fn new(sample_rate: u32, channels: usize) -> (Self, Producer<Command>, Consumer<Source>, Arc<Position>) {
		let (commands, receiver) = RingBuffer::new(32);
		let (retired, garbage) = RingBuffer::new(128);
		let position = Arc::new(Position::default());
		let mixer = Self {
			commands: receiver,
			retired,
			current: None,
			previous: None,
			fade: 0,
			fade_frames: (sample_rate as usize / 200).max(1),
			channels: channels.max(1),
			position: position.clone(),
		};
		(mixer, commands, garbage, position)
	}
	fn accept_commands(&mut self) {
		// Reserving retirement slots before popping ensures Arc destructors never run here.
		if self.retired.slots() >= 2
			&& (self.current.is_none() && self.previous.is_none() || self.fade >= self.fade_frames)
		{
			let Ok(command) = self.commands.pop() else { return };
			if let Some(previous) = self.previous.take() {
				let _ = self.retired.push(previous.source);
			}
			self.previous = self.current.take();
			self.fade = 0;
			self.current = match command {
				Command::Play {
					id,
					buffer,
					gain,
					onset,
				} => Some(Voice {
					id,
					source: Source::Buffered(buffer),
					frame: 0,
					gain,
					onset,
				}),
				Command::Stream {
					id,
					stream,
					gain,
					onset,
				} => Some(Voice {
					id,
					source: Source::Streaming(stream),
					frame: 0,
					gain,
					onset,
				}),
				Command::Stop => None,
			};
		}
	}
	pub fn render<T: cpal::Sample + cpal::FromSample<f32>>(&mut self, output: &mut [T]) {
		self.render_with_delay(output, Duration::ZERO);
	}
	fn render_with_delay<T: cpal::Sample + cpal::FromSample<f32>>(&mut self, output: &mut [T], delivery: Duration) {
		self.accept_commands();
		for frame in output.chunks_mut(self.channels) {
			let current_ready = self.current.as_mut().is_some_and(Voice::prepare_frame);
			if current_ready
				&& let Some(voice) = &mut self.current
				&& let Some(onset) = voice.onset.take()
			{
				// A single monotonic clock read per measured voice; no logs or allocation on the callback.
				let micros = onset
					.prior_micros
					.saturating_add(onset.requested.elapsed().as_micros() as u64)
					.saturating_add(delivery.as_micros() as u64);
				self.position.onset_micros.store(micros, Ordering::Relaxed);
				self.position.onset_token.store(onset.token, Ordering::Release);
			}
			let previous_ready = self.previous.as_mut().is_some_and(Voice::prepare_frame);
			let angle = (self.fade as f32 / self.fade_frames as f32).min(1.0) * std::f32::consts::FRAC_PI_2;
			let (incoming, outgoing) = angle.sin_cos();
			for (channel, value) in frame.iter_mut().enumerate() {
				let a = self
					.current
					.as_ref()
					.map_or(0.0, |voice| voice.sample(channel, self.channels) * incoming);
				let b = self
					.previous
					.as_ref()
					.map_or(0.0, |voice| voice.sample(channel, self.channels) * outgoing);
				*value = T::from_sample((a + b).clamp(-1.0, 1.0));
			}
			if current_ready && let Some(voice) = &mut self.current {
				voice.frame = voice.frame.saturating_add(1);
			}
			if previous_ready && let Some(voice) = &mut self.previous {
				voice.frame = voice.frame.saturating_add(1);
			}
			self.fade = self.fade.saturating_add(1);
		}
		if self.fade >= self.fade_frames
			&& self.retired.slots() > 0
			&& let Some(previous) = self.previous.take()
		{
			let _ = self.retired.push(previous.source);
		}
		if let Some(voice) = &self.current {
			self.position.sample_id.store(voice.id, Ordering::Relaxed);
			self.position.frame.store(
				match &voice.source {
					Source::Buffered(buffer) => voice.frame.min(buffer.frames()),
					Source::Streaming(_) => voice.frame,
				} as u64,
				Ordering::Relaxed,
			);
			self.position.playing.store(!voice.finished(), Ordering::Release);
		} else {
			self.position.playing.store(false, Ordering::Release);
		}
	}
}

pub struct Output {
	pub sample_rate: u32,
	pub position: Arc<Position>,
	commands: Producer<Command>,
	shutdown: Option<mpsc::Sender<()>>,
	thread: Option<JoinHandle<()>>,
}
impl Output {
	pub fn open() -> Result<Self, Error> {
		let (ready, wait) = mpsc::sync_channel(1);
		let (shutdown, exit) = mpsc::channel();
		let thread = thread::Builder::new()
			.name("sampler-audio-device".into())
			.spawn(move || {
				let setup = (|| -> Result<_, Error> {
					let device = cpal::default_host()
						.default_output_device()
						.ok_or_else(|| Error::Device("No output device".into()))?;
					let supported = device
						.default_output_config()
						.map_err(|e| Error::Device(e.to_string()))?;
					let rate = supported.sample_rate();
					let channels = supported.channels() as usize;
					let (mixer, commands, garbage, position) = Mixer::new(rate, channels);
					let config: cpal::StreamConfig = supported.into();
					let stream = match supported.sample_format() {
						cpal::SampleFormat::F32 => stream::<f32>(&device, config, mixer, position.clone()),
						cpal::SampleFormat::F64 => stream::<f64>(&device, config, mixer, position.clone()),
						cpal::SampleFormat::I16 => stream::<i16>(&device, config, mixer, position.clone()),
						cpal::SampleFormat::I32 => stream::<i32>(&device, config, mixer, position.clone()),
						cpal::SampleFormat::U16 => stream::<u16>(&device, config, mixer, position.clone()),
						format => Err(Error::Device(format!("Unsupported output format: {format}"))),
					}?;
					stream.play().map_err(|e| Error::Device(e.to_string()))?;
					Ok((stream, rate, commands, garbage, position))
				})();
				match setup {
					Ok((stream, rate, commands, mut garbage, position)) => {
						if ready.send(Ok((rate, commands, position))).is_err() {
							return;
						}
						loop {
							while let Ok(buffer) = garbage.pop() {
								drop(buffer);
							}
							match exit.recv_timeout(Duration::from_millis(2)) {
								Err(mpsc::RecvTimeoutError::Timeout) => {}
								_ => break,
							}
						}
						drop(stream);
					}
					Err(error) => {
						let _ = ready.send(Err(error));
					}
				}
			})
			.map_err(|e| Error::Device(e.to_string()))?;
		let (sample_rate, commands, position) = wait.recv().map_err(|e| Error::Device(e.to_string()))??;
		Ok(Self {
			sample_rate,
			position,
			commands,
			shutdown: Some(shutdown),
			thread: Some(thread),
		})
	}
	pub fn play(&mut self, id: u64, buffer: Arc<Buffer>, gain: f32) -> Result<(), Error> {
		self.play_measured(id, buffer, gain, None)
	}
	pub fn play_measured(
		&mut self,
		id: u64,
		buffer: Arc<Buffer>,
		gain: f32,
		onset: Option<Onset>,
	) -> Result<(), Error> {
		if buffer.channels == 0 || buffer.frames() == 0 || !gain.is_finite() || gain < 0.0 {
			return Err(Error::InvalidBuffer);
		}
		self.commands
			.push(Command::Play {
				id,
				buffer,
				gain,
				onset,
			})
			.map_err(|_| Error::QueueFull)
	}
	pub fn stream(&mut self, id: u64, stream: Box<StreamBuffer>, gain: f32) -> Result<(), Error> {
		self.stream_measured(id, stream, gain, None)
	}
	pub fn stream_measured(
		&mut self,
		id: u64,
		stream: Box<StreamBuffer>,
		gain: f32,
		onset: Option<Onset>,
	) -> Result<(), Error> {
		if !gain.is_finite() || gain < 0.0 {
			return Err(Error::InvalidBuffer);
		}
		self.commands
			.push(Command::Stream {
				id,
				stream,
				gain,
				onset,
			})
			.map_err(|_| Error::QueueFull)
	}
	pub fn stop(&mut self) -> Result<(), Error> {
		self.commands.push(Command::Stop).map_err(|_| Error::QueueFull)
	}
}
impl Drop for Output {
	fn drop(&mut self) {
		if let Some(tx) = self.shutdown.take() {
			let _ = tx.send(());
		}
		if let Some(thread) = self.thread.take() {
			let _ = thread.join();
		}
	}
}
fn stream<T: cpal::SizedSample + cpal::FromSample<f32>>(
	device: &cpal::Device,
	config: cpal::StreamConfig,
	mut mixer: Mixer,
	position: Arc<Position>,
) -> Result<cpal::Stream, Error> {
	device
		.build_output_stream(
			config,
			move |data: &mut [T], info| {
				let timestamp = info.timestamp();
				let delay = timestamp.playback.duration_since(timestamp.callback);
				mixer.render_with_delay(data, delay);
			},
			move |_| {
				position.device_error.store(true, Ordering::Release);
			},
			None,
		)
		.map_err(|e| Error::Device(e.to_string()))
}

pub fn matched_gain(lufs: Option<f64>, peak_dbfs: Option<f64>, target: f64) -> f32 {
	let desired = lufs.filter(|v| v.is_finite()).map_or(0.0, |lufs| target - lufs);
	let headroom = peak_dbfs.filter(|v| v.is_finite()).map_or(0.0, |peak| -1.0 - peak);
	10.0f64.powf(desired.min(headroom) / 20.0) as f32
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn crossfade_is_continuous_and_retirement_is_deferred() {
		let (mut mixer, mut commands, mut garbage, position) = Mixer::new(48000, 1);
		let first = Arc::new(Buffer {
			samples: vec![0.5; 10000].into_boxed_slice(),
			channels: 1,
		});
		commands
			.push(Command::Play {
				id: 1,
				buffer: first.clone(),
				gain: 1.0,
				onset: None,
			})
			.ok()
			.unwrap();
		let mut block = [0.0f32; 480];
		mixer.render(&mut block);
		assert_eq!(block[0], 0.0);
		assert!((block[479] - 0.5).abs() < 0.001);
		commands
			.push(Command::Play {
				id: 2,
				buffer: Arc::new(Buffer {
					samples: vec![-0.5; 10000].into_boxed_slice(),
					channels: 1,
				}),
				gain: 1.0,
				onset: None,
			})
			.ok()
			.unwrap();
		mixer.render(&mut block);
		assert!((block[0] - 0.5).abs() < 0.001);
		assert!(block.windows(2).all(|v| (v[1] - v[0]).abs() < 0.01));
		assert_eq!(Arc::strong_count(&first), 2);
		drop(garbage.pop().unwrap());
		assert_eq!(Arc::strong_count(&first), 1);
		assert_eq!(position.sample_id.load(Ordering::Relaxed), 2);
		commands.push(Command::Stop).ok().unwrap();
		mixer.render(&mut block);
		assert!(!position.playing.load(Ordering::Acquire));
		assert!(block[479].abs() < 0.001);
	}
	#[test]
	fn loudness_gain_respects_peak_headroom() {
		let gain = matched_gain(Some(-30.0), Some(-3.0), -16.0);
		assert!((20.0 * gain.log10() - 2.0).abs() < 0.001);
	}
	#[test]
	fn rapid_switches_finish_each_fade_without_amplitude_jumps() {
		let (mut mixer, mut commands, mut garbage, _) = Mixer::new(48000, 1);
		let mut block = [0.0f32; 64];
		let mut last = 0.0;
		for id in 1..32 {
			let buffer = Arc::new(Buffer {
				samples: vec![if id % 2 == 0 { -0.5 } else { 0.5 }; 4096].into_boxed_slice(),
				channels: 1,
			});
			commands
				.push(Command::Play {
					id,
					buffer,
					gain: 1.0,
					onset: None,
				})
				.ok()
				.unwrap();
			mixer.render(&mut block);
			for sample in block {
				assert!((sample - last).abs() < 0.01);
				last = sample;
			}
			while let Ok(source) = garbage.pop() {
				drop(source);
			}
		}
	}
}
