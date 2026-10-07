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
	let mut result = Audio {
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
	while let Some(packet) = format.next_packet()? {
		if packet.track_id != id {
			continue;
		}
		let buffer = decoder.decode(&packet)?;
		let channels = buffer.spec().channels().count();
		let rate = buffer.spec().rate();
		if !result.samples.is_empty() && (channels != result.channels || rate != result.sample_rate) {
			return Err(Error::Invalid("mid-stream format change".into()));
		}
		result.channels = channels;
		result.sample_rate = rate;
		if channels == 0 || rate == 0 {
			return Err(Error::Invalid("zero sample rate or channels".into()));
		}
		let old = result.samples.len();
		let count = buffer.samples_interleaved();
		// A malformed container must not exhaust memory in every analysis worker.
		if old.saturating_add(count) > 128 * 1024 * 1024 {
			return Err(Error::Invalid("decoded audio exceeds 512 MiB limit".into()));
		}
		result.samples.resize(old + count, 0.0);
		buffer.copy_to_slice_interleaved(&mut result.samples[old..]);
		if let Some(seconds) = seconds {
			let limit = (seconds * rate as f64) as usize * channels;
			if result.samples.len() >= limit {
				result.samples.truncate(limit);
				break;
			}
		}
	}
	if result.samples.is_empty() || result.samples.iter().any(|s| !s.is_finite()) {
		return Err(Error::Invalid("empty or non-finite signal".into()));
	}
	Ok(result)
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
}
