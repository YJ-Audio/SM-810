use audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Resampler};
use std::{fs::File, path::Path};
use symphonia::core::{
	codecs::audio::AudioDecoderOptions,
	formats::{FormatOptions, TrackType, probe::Hint},
	io::MediaSourceStream,
	meta::MetadataOptions,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
	#[error("I/O: {0}")]
	Io(#[from] std::io::Error),
	#[error("decode: {0}")]
	Decode(#[from] symphonia::core::errors::Error),
	#[error("unsupported or invalid audio: {0}")]
	Invalid(String),
	#[error("resampling: {0}")]
	Resample(String),
}

#[derive(Debug, Clone)]
pub struct Audio {
	pub samples: Vec<f32>,
	pub sample_rate: u32,
	pub channels: usize,
	pub bits_per_sample: Option<u32>,
	pub container_frames: Option<u64>,
}
impl Audio {
	pub fn frames(&self) -> usize {
		self.samples.len() / self.channels.max(1)
	}
	pub fn duration(&self) -> f64 {
		self.frames() as f64 / self.sample_rate.max(1) as f64
	}
	pub fn mono(&self) -> Vec<f32> {
		self.samples
			.chunks_exact(self.channels.max(1))
			.map(|frame| frame.iter().sum::<f32>() / frame.len() as f32)
			.collect()
	}
	pub fn resample(&self, rate: u32) -> Result<Self, Error> {
		if rate == self.sample_rate {
			return Ok(self.clone());
		}
		if rate == 0 || self.sample_rate == 0 || self.channels == 0 {
			return Err(Error::Invalid("zero sample rate or channels".into()));
		}
		let mut resampler = Fft::<f32>::new(
			self.sample_rate as usize,
			rate as usize,
			1024,
			self.channels,
			FixedSync::Both,
		)
		.map_err(|e| Error::Resample(e.to_string()))?;
		let input = InterleavedSlice::new(&self.samples, self.channels, self.frames())
			.map_err(|e| Error::Resample(e.to_string()))?;
		let count = resampler.process_all_needed_output_len(self.frames());
		let mut samples = vec![0f32; count * self.channels];
		let mut output = InterleavedSlice::new_mut(&mut samples, self.channels, count)
			.map_err(|e| Error::Resample(e.to_string()))?;
		let (_, frames) = resampler
			.process_all_into_buffer(&input, &mut output, self.frames(), None)
			.map_err(|e| Error::Resample(e.to_string()))?;
		samples.truncate(frames * self.channels);
		Ok(Self {
			samples,
			sample_rate: rate,
			channels: self.channels,
			bits_per_sample: self.bits_per_sample,
			container_frames: Some(frames as u64),
		})
	}
	pub fn mono_at(&self, rate: u32) -> Result<Vec<f32>, Error> {
		let mono = Self {
			samples: self.mono(),
			sample_rate: self.sample_rate,
			channels: 1,
			bits_per_sample: None,
			container_frames: self.container_frames,
		};
		Ok(mono.resample(rate)?.samples)
	}
}

pub fn decode(path: &Path) -> Result<Audio, Error> {
	decode_head(path, None)
}
pub fn decode_head(path: &Path, seconds: Option<f64>) -> Result<Audio, Error> {
	if seconds.is_some_and(|s| !s.is_finite() || s <= 0.0) {
		return Err(Error::Invalid("preview duration must be positive".into()));
	}
	let mut result: Option<Audio> = None;
	packets(path, |mut packet| {
		let audio = result.get_or_insert_with(|| Audio {
			samples: Vec::new(),
			..packet.clone()
		});
		if let Some(seconds) = seconds {
			let limit = (seconds * packet.sample_rate as f64) as usize * packet.channels;
			packet.samples.truncate(limit.saturating_sub(audio.samples.len()));
		}
		if audio.samples.len().saturating_add(packet.samples.len()) > 128 * 1024 * 1024 {
			return Err(Error::Invalid("decoded audio exceeds 512 MiB limit".into()));
		}
		audio.samples.extend_from_slice(&packet.samples);
		Ok(seconds.is_none_or(|seconds| audio.duration() < seconds))
	})?;
	result.ok_or_else(|| Error::Invalid("empty signal".into()))
}

fn packets(path: &Path, mut visit: impl FnMut(Audio) -> Result<bool, Error>) -> Result<(), Error> {
	let file = File::open(path)?;
	let mut hint = Hint::new();
	if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
		hint.with_extension(ext);
	}
	let mut format = symphonia::default::get_probe().probe(
		&hint,
		MediaSourceStream::new(Box::new(file), Default::default()),
		FormatOptions::default(),
		MetadataOptions::default(),
	)?;
	let track = format
		.default_track(TrackType::Audio)
		.ok_or_else(|| Error::Invalid("no audio track".into()))?;
	let params = track
		.codec_params
		.as_ref()
		.and_then(|p| p.audio())
		.ok_or_else(|| Error::Invalid("missing audio parameters".into()))?;
	let mut metadata = Audio {
		samples: Vec::new(),
		sample_rate: params.sample_rate.unwrap_or(0),
		channels: params.channels.as_ref().map(|c| c.count()).unwrap_or(0),
		bits_per_sample: params.bits_per_sample,
		container_frames: track.num_frames,
	};
	let id = track.id;
	let mut options = AudioDecoderOptions::default();
	options.gapless = true;
	let mut decoder = symphonia::default::get_codecs().make_audio_decoder(params, &options)?;

	let mut seen = false;
	while let Some(packet) = format.next_packet()? {
		if packet.track_id != id {
			continue;
		}
		let buffer = decoder.decode(&packet)?;
		let channels = buffer.spec().channels().count();
		let rate = buffer.spec().rate();
		if seen && (channels != metadata.channels || rate != metadata.sample_rate) {
			return Err(Error::Invalid("mid-stream format change".into()));
		}
		if channels == 0 || rate == 0 {
			return Err(Error::Invalid("zero sample rate or channels".into()));
		}
		metadata.channels = channels;
		metadata.sample_rate = rate;
		let mut samples = vec![0.0f32; buffer.samples_interleaved()];
		buffer.copy_to_slice_interleaved(&mut samples);
		if samples.iter().any(|s| !s.is_finite()) {
			return Err(Error::Invalid("non-finite signal".into()));
		}
		if samples.is_empty() {
			continue;
		}
		seen = true;
		if !visit(Audio {
			samples,
			..metadata.clone()
		})? {
			return Ok(());
		}
	}
	if seen {
		Ok(())
	} else {
		Err(Error::Invalid("empty signal".into()))
	}
}

/// Visits native-rate or resampled blocks without retaining the full file. Returning false cancels decoding.
pub fn stream(path: &Path, rate: u32, semitones: i8, mut emit: impl FnMut(Audio) -> bool) -> Result<(), Error> {
	if rate == 0 || !(-24..=24).contains(&semitones) {
		return Err(Error::Invalid("invalid playback rate".into()));
	}
	let mut converter: Option<BlockResampler> = None;
	let mut cancelled = false;
	packets(path, |mut packet| {
		packet.sample_rate = (packet.sample_rate as f64 * 2.0f64.powf(semitones as f64 / 12.0)).round() as u32;
		if packet.sample_rate == rate {
			cancelled = !emit(packet);
		} else {
			if converter.is_none() {
				converter = Some(BlockResampler::new(&packet, rate)?);
			}
			cancelled = !converter.as_mut().unwrap().push(&packet.samples, &mut emit)?;
		}
		Ok(!cancelled)
	})?;
	if !cancelled && let Some(mut converter) = converter {
		converter.finish(&mut emit)?;
	}
	Ok(())
}

/// Converts cached native PCM using the streaming filter. An incomplete head is not flushed:
/// only frames whose filter context exists are emitted, so the original stream can resume at the same index.
pub fn convert_head(mut audio: Audio, rate: u32, semitones: i8, complete: bool) -> Result<Audio, Error> {
	if rate == 0 || !(-24..=24).contains(&semitones) {
		return Err(Error::Invalid("Invalid playback rate".into()));
	}
	audio.sample_rate = (audio.sample_rate as f64 * 2.0f64.powf(semitones as f64 / 12.0)).round() as u32;
	if audio.sample_rate == rate {
		return Ok(audio);
	}
	let mut converter = BlockResampler::new(&audio, rate)?;
	let mut samples = Vec::new();
	let mut emit = |block: Audio| {
		samples.extend(block.samples);
		true
	};
	converter.push(&audio.samples, &mut emit)?;
	if complete {
		converter.finish(&mut emit)?;
	}
	Ok(Audio {
		samples,
		sample_rate: rate,
		..audio
	})
}

struct BlockResampler {
	filter: Fft<f32>,
	metadata: Audio,
	pending: Vec<f32>,
	input_frames: usize,
	output_frames: usize,
	trim: usize,
	ratio: f64,
}
impl BlockResampler {
	fn new(audio: &Audio, rate: u32) -> Result<Self, Error> {
		let filter = Fft::<f32>::new(
			audio.sample_rate as usize,
			rate as usize,
			1024,
			audio.channels,
			FixedSync::Both,
		)
		.map_err(|e| Error::Resample(e.to_string()))?;
		Ok(Self {
			trim: filter.output_delay(),
			filter,
			metadata: Audio {
				samples: Vec::new(),
				sample_rate: rate,
				container_frames: None,
				..audio.clone()
			},
			pending: Vec::new(),
			input_frames: 0,
			output_frames: 0,
			ratio: rate as f64 / audio.sample_rate as f64,
		})
	}
	fn push(&mut self, samples: &[f32], emit: &mut impl FnMut(Audio) -> bool) -> Result<bool, Error> {
		self.input_frames += samples.len() / self.metadata.channels;
		self.pending.extend_from_slice(samples);
		while self.pending.len() / self.metadata.channels >= self.filter.input_frames_next() {
			if !self.block(false, emit)? {
				return Ok(false);
			}
		}
		Ok(true)
	}
	fn block(&mut self, last: bool, emit: &mut impl FnMut(Audio) -> bool) -> Result<bool, Error> {
		let channels = self.metadata.channels;
		let available = (self.pending.len() / channels).min(self.filter.input_frames_next());
		let input =
			InterleavedSlice::new(&self.pending, channels, available).map_err(|e| Error::Resample(e.to_string()))?;
		let capacity = self.filter.output_frames_max();
		let mut samples = vec![0.0; capacity * channels];
		let mut output =
			InterleavedSlice::new_mut(&mut samples, channels, capacity).map_err(|e| Error::Resample(e.to_string()))?;
		let indexing = rubato::Indexing {
			partial_len: Some(available),
			..Default::default()
		};
		let (_, produced) = self
			.filter
			.process_into_buffer(&input, &mut output, Some(&indexing))
			.map_err(|e| Error::Resample(e.to_string()))?;
		self.pending.drain(..available * channels);
		let skip = self.trim.min(produced);
		self.trim -= skip;
		let expected = (self.input_frames as f64 * self.ratio).ceil() as usize;
		let frames = if last {
			(produced - skip).min(expected.saturating_sub(self.output_frames))
		} else {
			produced - skip
		};
		if frames == 0 {
			return Ok(true);
		}
		self.output_frames += frames;
		samples.copy_within(skip * channels..(skip + frames) * channels, 0);
		samples.truncate(frames * channels);
		Ok(emit(Audio {
			samples,
			..self.metadata.clone()
		}))
	}
	fn finish(&mut self, emit: &mut impl FnMut(Audio) -> bool) -> Result<(), Error> {
		let expected = (self.input_frames as f64 * self.ratio).ceil() as usize;
		while self.output_frames < expected {
			if !self.block(true, emit)? {
				break;
			}
		}
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn wav_decode_head_downmix_and_resample_preserve_duration() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("sine.wav");
		let mut writer = hound::WavWriter::create(
			&path,
			hound::WavSpec {
				channels: 2,
				sample_rate: 48000,
				bits_per_sample: 16,
				sample_format: hound::SampleFormat::Int,
			},
		)
		.unwrap();
		for i in 0..48000 {
			let s = ((i as f32 * 440.0 * std::f32::consts::TAU / 48000.0).sin() * 16000.0) as i16;
			writer.write_sample(s).unwrap();
			writer.write_sample(s).unwrap();
		}
		writer.finalize().unwrap();
		let audio = decode(&path).unwrap();
		assert_eq!(audio.frames(), 48000);
		assert_eq!(audio.channels, 2);
		assert_eq!(decode_head(&path, Some(0.1)).unwrap().frames(), 4800);
		let mono = audio.mono_at(16000).unwrap();
		assert_eq!(mono.len(), 16000);
		assert!(mono.iter().map(|s| s * s).sum::<f32>() / 16000.0 > 0.1);
	}
	#[test]
	fn streamed_resampling_matches_whole_file_and_can_cancel_after_first_block() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("stream.wav");
		let mut file = hound::WavWriter::create(
			&path,
			hound::WavSpec {
				channels: 2,
				sample_rate: 44100,
				bits_per_sample: 32,
				sample_format: hound::SampleFormat::Float,
			},
		)
		.unwrap();
		for i in 0..103237 {
			file.write_sample((i as f32 * 0.07).sin() * 0.4).unwrap();
			file.write_sample((i as f32 * 0.09).cos() * 0.2).unwrap();
		}
		file.finalize().unwrap();
		for (rate, pitch) in [(44100, 0), (48000, 0), (16000, 0), (48000, 7), (48000, -12)] {
			let mut original = decode(&path).unwrap();
			original.sample_rate = (original.sample_rate as f64 * 2.0f64.powf(pitch as f64 / 12.0)).round() as u32;
			let expected = original.resample(rate).unwrap();
			let mut output = Vec::new();
			let mut blocks = 0;
			stream(&path, rate, pitch, |audio| {
				blocks += 1;
				assert_eq!(audio.sample_rate, rate);
				output.extend(audio.samples);
				true
			})
			.unwrap();
			assert!(blocks > 1);
			assert_eq!(output.len(), expected.samples.len());
			assert!(output.iter().zip(&expected.samples).all(|(a, b)| (a - b).abs() < 1e-5));
			let head = decode_head(&path, Some(1.0)).unwrap();
			let converted = convert_head(head, rate, pitch, false).unwrap();
			assert!(!converted.samples.is_empty());
			assert!(converted.samples.len() < output.len());
			assert!(
				converted.samples.iter().zip(&output).all(|(a, b)| (a - b).abs() < 1e-5),
				"Cached prefix differs at rate {rate}, pitch {pitch}"
			);
			let complete = convert_head(decode(&path).unwrap(), rate, pitch, true).unwrap();
			assert_eq!(complete.samples.len(), output.len());
			assert!(complete.samples.iter().zip(&output).all(|(a, b)| (a - b).abs() < 1e-5));
		}
		let mut calls = 0;
		stream(&path, 48000, 0, |block| {
			calls += 1;
			assert!(block.frames() < 10000);
			false
		})
		.unwrap();
		assert_eq!(calls, 1);
	}
}
