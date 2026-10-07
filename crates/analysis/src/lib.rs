use ebur128::{EbuR128, Mode};
use regex::Regex;
use sampler_decode::Audio;
use serde::{Deserialize, Serialize};
use std::{
	fs::File,
	io::{Read, Seek, SeekFrom},
	path::Path,
	sync::LazyLock,
};
use thiserror::Error;

pub const ANALYZER_VERSION: i64 = 1;
#[derive(Debug, Error)]
pub enum Error {
	#[error("I/O: {0}")]
	Io(#[from] std::io::Error),
	#[error("loudness: {0}")]
	Loudness(#[from] ebur128::Error),
	#[error("invalid RIFF chunk")]
	Riff,
	#[error(transparent)]
	Decode(#[from] sampler_decode::Error),
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Metadata {
	pub bpm: Option<f64>,
	pub bpm_source: Option<String>,
	pub key_root: Option<u8>,
	pub key_mode: Option<String>,
	pub key_source: Option<String>,
	pub is_loop: Option<bool>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Analysis {
	pub analyzer_ver: i64,
	pub duration_ms: i64,
	pub sample_rate: u32,
	pub channels: usize,
	pub lufs: Option<f64>,
	pub peak_dbfs: Option<f64>,
	#[serde(flatten)]
	pub metadata: Metadata,
}

static BPM: LazyLock<Regex> = LazyLock::new(|| {
	Regex::new(r"(?i)(?:^|[^a-z0-9])([0-9]{2,3}(?:\.[0-9]+)?)\s*bpm(?:$|[^a-z0-9])")
		.expect("constant BPM regex is valid")
});
static NUMBER: LazyLock<Regex> = LazyLock::new(|| {
	Regex::new(r"(?:^|_)([0-9]{2,3}(?:\.[0-9]+)?)(?:_|$)").expect("constant numeric tempo regex is valid")
});
static KEY: LazyLock<Regex> = LazyLock::new(|| {
	Regex::new(r"(?:^|[ _\-])([A-G])([#b]?)(min|maj|minor|major|m)?(?:$|[ _\-])").expect("constant key regex is valid")
});

pub fn filename_metadata(path: &Path) -> Metadata {
	let stem = path.file_stem().unwrap_or_default().to_string_lossy();
	let bpm = BPM
		.captures(&stem)
		.or_else(|| NUMBER.captures(&stem))
		.and_then(|c| c[1].parse::<f64>().ok())
		.filter(|v| (40.0..=300.0).contains(v));
	let mut metadata = Metadata {
		bpm,
		bpm_source: bpm.map(|_| "filename".into()),
		..Default::default()
	};
	if let Some(c) = KEY.captures(&stem) {
		let base = match &c[1] {
			"C" => 0,
			"D" => 2,
			"E" => 4,
			"F" => 5,
			"G" => 7,
			"A" => 9,
			_ => 11,
		};
		let offset = match &c[2] {
			"#" => 1,
			"b" => -1,
			_ => 0,
		};
		metadata.key_root = Some((base + offset + 12) as u8 % 12);
		metadata.key_mode = c.get(3).map(|m| {
			if ["min", "minor", "m"].contains(&m.as_str()) {
				"minor"
			} else {
				"major"
			}
			.into()
		});
		metadata.key_source = Some("filename".into());
	}
	metadata
}
fn u32le(data: &[u8], offset: usize) -> Result<u32, Error> {
	Ok(u32::from_le_bytes(
		data.get(offset..offset + 4)
			.ok_or(Error::Riff)?
			.try_into()
			.map_err(|_| Error::Riff)?,
	))
}

pub fn riff_metadata(path: &Path) -> Result<Metadata, Error> {
	let mut file = File::open(path)?;
	let size = file.metadata()?.len();
	let mut header = [0u8; 12];
	if file.read(&mut header)? != 12 || &header[..4] != b"RIFF" || &header[8..] != b"WAVE" {
		return Ok(Metadata::default());
	}
	let end = (u32le(&header, 4)? as u64 + 8).min(size);
	let mut offset = 12;
	let mut result = Metadata::default();
	let mut smpl_key = None;
	let mut smpl_loop = None;
	while offset + 8 <= end {
		file.seek(SeekFrom::Start(offset))?;
		let mut header = [0u8; 8];
		file.read_exact(&mut header)?;
		let length = u32le(&header, 4)? as u64;
		if offset + 8 + length > end {
			return Err(Error::Riff);
		}
		if &header[..4] == b"acid" || &header[..4] == b"smpl" {
			let mut data = vec![0u8; length.min(64) as usize];
			file.read_exact(&mut data)?;
			if &header[..4] == b"acid" {
				if length < 24 {
					return Err(Error::Riff);
				}
				let flags = u32le(&data, 0)?;
				result.is_loop = Some(flags & 1 == 0);
				if flags & 2 != 0 {
					result.key_root = Some(u16::from_le_bytes([data[4], data[5]]) as u8 % 12);
					result.key_source = Some("chunk".into());
				}
				let bpm = f32::from_bits(u32le(&data, 20)?) as f64;
				if bpm.is_finite() && (20.0..=400.0).contains(&bpm) {
					result.bpm = Some(bpm);
					result.bpm_source = Some("chunk".into());
				}
			} else {
				if length < 36 {
					return Err(Error::Riff);
				}
				let key = u32le(&data, 12)?;
				if key <= 127 {
					smpl_key = Some((key % 12) as u8);
				}
				smpl_loop = Some(u32le(&data, 28)? > 0);
			}
		}
		offset += 8 + length + (length % 2);
	}
	if result.key_root.is_none() {
		result.key_root = smpl_key;
		result.key_source = smpl_key.map(|_| "chunk".into());
	}
	if result.is_loop.is_none() {
		result.is_loop = smpl_loop.filter(|v| *v);
	}
	Ok(result)
}

fn estimate_bpm(mono: &[f32], rate: u32) -> Option<f64> {
	let hop = (rate as usize / 200).max(1);
	let energy: Vec<f32> = mono
		.chunks(hop)
		.map(|block| block.iter().map(|s| s * s).sum::<f32>() / block.len() as f32)
		.collect();
	let onset: Vec<f32> = energy.windows(2).map(|w| (w[1] - w[0]).max(0.0)).collect();
	if onset.len() < 200 {
		return None;
	}
	let hz = rate as f64 / hop as f64;
	let mut best = (0.0, 0);
	for lag in (hz * 60.0 / 200.0) as usize..=(hz * 60.0 / 60.0) as usize {
		if lag >= onset.len() {
			continue;
		}
		let score = onset[lag..].iter().zip(&onset).map(|(a, b)| a * b).sum::<f32>() / (onset.len() - lag) as f32;
		if score > best.0 {
			best = (score, lag);
		}
	}
	(best.0 > 1e-10).then(|| (60.0 * hz / best.1 as f64 * 10.0).round() / 10.0)
}

fn estimate_key(mono: &[f32], rate: u32, is_loop: bool) -> Option<(u8, Option<String>)> {
	use rustfft::{FftPlanner, num_complex::Complex};
	let size = 4096;
	let mut planner = FftPlanner::<f32>::new();
	let fft = planner.plan_fft_forward(size);
	let mut chroma = [0f32; 12];
	let mut strongest = (0.0, 0.0);
	for segment in mono.chunks(size).take(if is_loop { 80 } else { 4 }) {
		let mut buffer = vec![Complex::new(0.0, 0.0); size];
		for (i, sample) in segment.iter().enumerate() {
			buffer[i].re = *sample * (0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / size as f32).cos());
		}
		fft.process(&mut buffer);
		for (i, value) in buffer.iter().enumerate().take(size / 2).skip(1) {
			let freq = i as f32 * rate as f32 / size as f32;
			if !(35.0..=4000.0).contains(&freq) {
				continue;
			}
			let power = value.norm_sqr();
			let midi = 69.0 + 12.0 * (freq / 440.0).log2();
			chroma[(midi.round() as i32).rem_euclid(12) as usize] += power.sqrt();
			if power > strongest.0 {
				strongest = (power, midi);
			}
		}
	}
	if strongest.0 < 1e-6 {
		return None;
	}
	if !is_loop {
		return Some(((strongest.1.round() as i32).rem_euclid(12) as u8, None));
	}
	let major = [6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88];
	let minor = [6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17];
	let mut best = (f32::NEG_INFINITY, 0, "major");
	for (profile, mode) in [(major, "major"), (minor, "minor")] {
		let mean = profile.iter().sum::<f32>() / 12.0;
		let norm = profile.iter().map(|v| (v - mean).powi(2)).sum::<f32>().sqrt();
		for root in 0..12 {
			let score = (0..12)
				.map(|i| chroma[(i + root) % 12] * (profile[i] - mean))
				.sum::<f32>()
				/ norm;
			if score > best.0 {
				best = (score, root, mode);
			}
		}
	}
	Some((best.1 as u8, Some(best.2.into())))
}

pub fn analyze(path: &Path, audio: &Audio) -> Result<Analysis, Error> {
	let mut metadata = riff_metadata(path)?;
	let filename = filename_metadata(path);
	if metadata.bpm.is_none() {
		metadata.bpm = filename.bpm;
		metadata.bpm_source = filename.bpm_source;
	}
	if metadata.key_root.is_none() {
		metadata.key_root = filename.key_root;
		metadata.key_mode = filename.key_mode;
		metadata.key_source = filename.key_source;
	}
	let mut meter = EbuR128::new(audio.channels as u32, audio.sample_rate, Mode::I)?;
	meter.add_frames_f32(&audio.samples)?;
	let loudness = if audio.duration() < 0.4 {
		// The momentary window includes initial zeroes; undo its fixed 400ms denominator.
		meter.loudness_momentary()? + 10.0 * ((audio.sample_rate as f64 * 0.4).round() / audio.frames() as f64).log10()
	} else {
		meter.loudness_global()?
	};
	let peak = audio.samples.iter().fold(0.0f32, |a, b| a.max(b.abs()));
	let mono = audio.mono_at(16000)?;
	let tail = mono.iter().rev().take(800).map(|s| s * s).sum::<f32>() / mono.len().clamp(1, 800) as f32;
	let mean = mono.iter().map(|s| s * s).sum::<f32>() / mono.len().max(1) as f32;
	if metadata.is_loop.is_none() {
		let sustained = audio.duration() >= 1.0 && tail > mean * 0.02 && tail > 1e-8;
		if sustained && metadata.bpm.is_none() {
			metadata.bpm = estimate_bpm(&mono, 16000);
			metadata.bpm_source = metadata.bpm.map(|_| "analysis".into());
		}
		metadata.is_loop = Some(
			sustained
				&& metadata.bpm.is_some_and(|bpm| {
					let bars = audio.duration() * bpm / 240.0;
					bars >= 0.5 && (bars - bars.round()).abs() < 0.06
				}),
		);
		if metadata.is_loop == Some(false) && metadata.bpm_source.as_deref() == Some("analysis") {
			metadata.bpm = None;
			metadata.bpm_source = None;
		}
	}
	if metadata.is_loop == Some(true) && metadata.bpm.is_none() {
		metadata.bpm = estimate_bpm(&mono, 16000);
		metadata.bpm_source = metadata.bpm.map(|_| "analysis".into());
	}
	if metadata.key_root.is_none()
		&& let Some((root, mode)) = estimate_key(&mono, 16000, metadata.is_loop == Some(true))
	{
		metadata.key_root = Some(root);
		metadata.key_mode = mode;
		metadata.key_source = Some("analysis".into());
	}
	Ok(Analysis {
		analyzer_ver: ANALYZER_VERSION,
		duration_ms: (audio.container_frames.unwrap_or(audio.frames() as u64) as f64 * 1000.0
			/ audio.sample_rate as f64)
			.round() as i64,
		sample_rate: audio.sample_rate,
		channels: audio.channels,
		lufs: loudness.is_finite().then_some(loudness),
		peak_dbfs: (peak > 0.0).then(|| 20.0 * (peak as f64).log10()),
		metadata,
	})
}

pub fn peaks(audio: &Audio, bins: usize) -> Vec<u8> {
	let bins = bins.max(1);
	let frames = audio.frames();
	let mut output = Vec::with_capacity(bins * 2);
	for i in 0..bins {
		let start = i * frames / bins;
		let end = ((i + 1) * frames / bins).max(start + 1).min(frames);
		let (mut min, mut max) = (0.0f32, 0.0f32);
		for sample in &audio.samples[start * audio.channels..end * audio.channels] {
			min = min.min(*sample);
			max = max.max(*sample);
		}
		output.push((min.clamp(-1.0, 1.0) * 127.0).round() as i8 as u8);
		output.push((max.clamp(-1.0, 1.0) * 127.0).round() as i8 as u8);
	}
	output
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn filename_parsing_has_boundaries_and_precedence() {
		for (name, bpm, key, mode) in [
			("Bass_120bpm_Am.wav", Some(120.0), Some(9), Some("minor")),
			("Loop_128_F#min.wav", Some(128.0), Some(6), Some("minor")),
			("Pad_Bbmaj_90.wav", Some(90.0), Some(10), Some("major")),
			("KCK_dark_03.wav", None, None, None),
			("FOAM_2024.wav", None, None, None),
		] {
			let m = filename_metadata(Path::new(name));
			assert_eq!((m.bpm, m.key_root, m.key_mode.as_deref()), (bpm, key, mode), "{name}");
		}
	}
	fn riff(chunks: Vec<([u8; 4], Vec<u8>)>) -> Vec<u8> {
		let mut result = b"RIFF\0\0\0\0WAVE".to_vec();
		for (id, bytes) in chunks {
			result.extend(id);
			result.extend((bytes.len() as u32).to_le_bytes());
			result.extend(&bytes);
			if bytes.len() % 2 == 1 {
				result.push(0);
			}
		}
		let size = (result.len() - 8) as u32;
		result[4..8].copy_from_slice(&size.to_le_bytes());
		result
	}
	#[test]
	fn acid_smpl_and_odd_padding_are_read_without_loading_audio() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("test.wav");
		let mut acid = vec![0; 24];
		acid[..4].copy_from_slice(&3u32.to_le_bytes());
		acid[4..6].copy_from_slice(&68u16.to_le_bytes());
		acid[20..24].copy_from_slice(&124f32.to_le_bytes());
		let mut smpl = vec![0; 36];
		smpl[12..16].copy_from_slice(&60u32.to_le_bytes());
		smpl[28..32].copy_from_slice(&1u32.to_le_bytes());
		std::fs::write(
			&path,
			riff(vec![(*b"JUNK", vec![0]), (*b"smpl", smpl), (*b"acid", acid)]),
		)
		.unwrap();
		let m = riff_metadata(&path).unwrap();
		assert_eq!(m.key_root, Some(8));
		assert_eq!(m.bpm, Some(124.0));
		assert_eq!(m.is_loop, Some(false));
		assert_eq!(m.key_source.as_deref(), Some("chunk"));
		std::fs::write(&path, riff(vec![(*b"acid", vec![0; 5])])).unwrap();
		assert!(riff_metadata(&path).is_err());
	}
	#[test]
	fn short_loudness_is_finite_and_silence_is_null() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("tone.wav");
		std::fs::write(&path, []).unwrap();
		for duration in [0.1, 1.0] {
			let audio = Audio {
				samples: (0..(duration * 48000.0) as usize)
					.map(|i| (i as f32 * 1000.0 * std::f32::consts::TAU / 48000.0).sin() * 0.1)
					.collect(),
				sample_rate: 48000,
				channels: 1,
				bits_per_sample: Some(32),
				container_frames: None,
			};
			let result = analyze(&path, &audio).unwrap();
			assert!((result.lufs.unwrap() + 23.0).abs() < 0.8);
			assert!((result.peak_dbfs.unwrap() + 20.0).abs() < 0.1);
			assert_eq!(peaks(&audio, 128).len(), 256);
		}
		let audio = Audio {
			samples: vec![0.0; 4800],
			sample_rate: 48000,
			channels: 1,
			bits_per_sample: None,
			container_frames: None,
		};
		assert!(analyze(&path, &audio).unwrap().lufs.is_none());
	}
}
