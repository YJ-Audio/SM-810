use crate::{Engine, Error, Result};
use sampler_db::{self as db, OptionalExtension};
use serde::Serialize;
use std::{io::Write, path::PathBuf};

#[derive(Serialize)]
pub struct Row {
	pub similarity: Option<f32>,
	#[serde(flatten)]
	pub sample: db::Sample,
	pub analysis: Option<db::AnalysisRecord>,
	pub peaks: Vec<i8>,
}
#[derive(Serialize)]
pub struct Page {
	pub items: Vec<Row>,
	pub total: usize,
}
#[derive(Serialize)]
pub struct Waveform {
	pub peaks: Vec<i8>,
	pub frames: usize,
	pub sample_rate: u32,
	pub channels: usize,
	pub bits_per_sample: Option<u32>,
}

impl Engine {
	pub fn browse(&self, mut request: db::BrowseQuery) -> Result<Page> {
		self.scope_map(&mut request)?;
		if request.text.trim_start().starts_with('~') || request.similar_to.is_some() {
			return self.semantic_browse(request);
		}
		let reader = self.reader()?;
		let page = db::browse(&reader, &request)?;
		let mut items = Vec::with_capacity(page.items.len());
		for sample in page.items {
			let analysis = db::analysis(&reader, sample.id)?;
			let peaks = self.peaks_with_reader(&reader, sample.id)?;
			items.push(Row {
				similarity: None,
				sample,
				analysis,
				peaks,
			});
		}
		Ok(Page {
			items,
			total: page.total,
		})
	}
	pub fn tags(&self) -> Result<Vec<db::TagSummary>> {
		Ok(db::tags(&self.reader()?)?)
	}
	pub fn remove_tag(&self, id: i64, name: String) -> Result<()> {
		self.write(move |tx| db::remove_tag(tx, id, &name))
	}
	pub fn file_path(&self, id: i64) -> Result<PathBuf> {
		db::hash_files(&self.reader()?, id)?
			.into_iter()
			.find(|file| file.path.is_file())
			.map(|file| file.path)
			.ok_or_else(|| Error::Invalid("Sample is offline. Reconnect its source and rescan.".into()))
	}
	pub fn waveform(&self, id: i64) -> Result<Waveform> {
		let audio = sampler_decode::decode(&self.file_path(id)?)?;
		Ok(Waveform {
			peaks: sampler_analysis::peaks(&audio, 1024)
				.into_iter()
				.map(|v| v as i8)
				.collect(),
			frames: audio.frames(),
			sample_rate: audio.sample_rate,
			channels: audio.channels,
			bits_per_sample: audio.bits_per_sample,
		})
	}
	pub fn snap_slice(&self, id: i64, start: usize, end: usize) -> Result<(usize, usize)> {
		let audio = sampler_decode::decode(&self.file_path(id)?)?;
		if start >= end || end > audio.frames() {
			return Err(Error::Invalid("Invalid slice range".into()));
		}
		let mono = audio.mono();
		let radius = (audio.sample_rate as usize / 200).max(1);
		let snap = |frame: usize| -> usize {
			if frame == 0 || frame == mono.len() {
				return frame;
			}
			let left = frame.saturating_sub(radius).max(1);
			let right = (frame + radius).min(mono.len() - 1);
			(left..=right)
				.filter(|&i| mono[i].signum() != mono[i - 1].signum() || (mono[i].abs() - mono[i - 1].abs()) > 0.2)
				.min_by_key(|i| i.abs_diff(frame))
				.unwrap_or(frame)
		};
		let (a, b) = (snap(start), snap(end));
		Ok(if a < b { (a, b) } else { (start, end) })
	}
	pub fn export_slice(&self, id: i64, start: usize, end: usize) -> Result<PathBuf> {
		if start >= end {
			return Err(Error::Invalid("Slice must contain at least one frame".into()));
		}
		let reader = self.reader()?;
		let existing: Option<String> = reader
			.query_row(
				"SELECT rel_path FROM slice_exports WHERE sample_id=?1 AND start_frame=?2 AND end_frame=?3",
				(id, start as i64, end as i64),
				|r| r.get(0),
			)
			.optional()
			.map_err(db::Error::from)?;
		let base = self
			.path
			.parent()
			.ok_or_else(|| Error::Invalid("No application data directory".into()))?;
		if let Some(existing) = existing {
			let path = base.join(existing);
			if path.is_file() {
				return Ok(path);
			}
		}
		let audio = sampler_decode::decode(&self.file_path(id)?)?;
		if end > audio.frames() {
			return Err(Error::Invalid("Slice extends beyond the source".into()));
		}
		let rel = format!("slices/{id}-{start}-{end}.wav");
		let destination = base.join(&rel);
		std::fs::create_dir_all(base.join("slices"))?;
		let mut temporary = tempfile::NamedTempFile::new_in(base.join("slices"))?;
		{
			let mut writer = hound::WavWriter::new(
				temporary.as_file_mut(),
				hound::WavSpec {
					channels: audio.channels as u16,
					sample_rate: audio.sample_rate,
					bits_per_sample: 32,
					sample_format: hound::SampleFormat::Float,
				},
			)?;
			for sample in &audio.samples[start * audio.channels..end * audio.channels] {
				writer.write_sample(*sample)?;
			}
			writer.finalize()?;
		}
		temporary.flush()?;
		temporary.as_file().sync_all()?;
		match temporary.persist_noclobber(&destination) {
			Ok(_) => {}
			Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {}
			Err(error) => return Err(error.error.into()),
		}
		self.write(move |tx| {
			tx.execute(
				"INSERT OR IGNORE INTO slice_exports(sample_id,start_frame,end_frame,rel_path) VALUES(?1,?2,?3,?4)",
				(id, start as i64, end as i64, rel),
			)?;
			Ok(())
		})?;
		Ok(destination)
	}
}
