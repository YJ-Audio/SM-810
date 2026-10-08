use std::{
	fs::File,
	io::{Read, Seek, SeekFrom},
	path::{Path, PathBuf},
	sync::{
		Arc,
		atomic::{AtomicBool, Ordering},
	},
	time::UNIX_EPOCH,
};

use ignore::WalkBuilder;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use thiserror::Error;
use xxhash_rust::xxh3::Xxh3;

#[derive(Debug, Error)]
pub enum Error {
	#[error("{path}: {source}")]
	Io { path: PathBuf, source: std::io::Error },
	#[error("filesystem watcher: {0}")]
	Watch(#[from] notify::Error),
	#[error("file changed while reading: {0}")]
	Changed(PathBuf),
	#[error("path cannot be represented as UTF-8: {0:?}")]
	Encoding(PathBuf),
}

#[derive(Debug, Clone)]
pub struct Entry {
	pub rel_path: String,
	pub size: u64,
	pub mtime: i64,
	pub quick_hash: [u8; 8],
	pub compressed: bool,
}

#[derive(Debug, Default)]
pub struct Report {
	pub files: usize,
	pub errors: Vec<String>,
}

fn io(path: &Path, source: std::io::Error) -> Error {
	Error::Io {
		path: path.to_path_buf(),
		source,
	}
}

pub fn supported(path: &Path) -> bool {
	path.extension()
		.and_then(|s| s.to_str())
		.is_some_and(|ext| ["wav", "aif", "aiff", "flac", "mp3", "ogg"].contains(&ext.to_ascii_lowercase().as_str()))
}

pub fn fingerprint(root: &Path, path: &Path) -> Result<Entry, Error> {
	let mut file = File::open(path).map_err(|e| io(path, e))?;
	let before = file.metadata().map_err(|e| io(path, e))?;
	let size = before.len();
	let mut hash = Xxh3::new();
	hash.update(&size.to_le_bytes());
	let mut bytes = vec![0u8; size.min(128 * 1024) as usize];
	if size <= 128 * 1024 {
		file.read_exact(&mut bytes).map_err(|e| io(path, e))?;
		hash.update(&bytes);
	} else {
		file.read_exact(&mut bytes[..64 * 1024]).map_err(|e| io(path, e))?;
		hash.update(&bytes[..64 * 1024]);
		file.seek(SeekFrom::End(-65536)).map_err(|e| io(path, e))?;
		file.read_exact(&mut bytes[..64 * 1024]).map_err(|e| io(path, e))?;
		hash.update(&bytes[..64 * 1024]);
	}
	let after = file.metadata().map_err(|e| io(path, e))?;
	if before.len() != after.len() || before.modified().ok() != after.modified().ok() {
		return Err(Error::Changed(path.to_path_buf()));
	}
	let rel = path
		.strip_prefix(root)
		.map_err(|_| Error::Encoding(path.to_path_buf()))?;
	let rel_path = rel
		.to_str()
		.ok_or_else(|| Error::Encoding(path.to_path_buf()))?
		.to_owned();
	// Persist one separator convention so path rules behave the same on every OS.
	#[cfg(windows)]
	let rel_path = rel_path.replace('\\', "/");
	let mtime = before
		.modified()
		.map_err(|e| io(path, e))?
		.duration_since(UNIX_EPOCH)
		.unwrap_or_default()
		.as_nanos()
		.min(i64::MAX as u128) as i64;
	let ext = path
		.extension()
		.and_then(|e| e.to_str())
		.unwrap_or_default()
		.to_ascii_lowercase();
	Ok(Entry {
		rel_path,
		size,
		mtime,
		quick_hash: hash.digest().to_le_bytes(),
		compressed: matches!(ext.as_str(), "flac" | "mp3" | "ogg"),
	})
}

pub fn full_hash(path: &Path) -> Result<[u8; 16], Error> {
	let mut file = File::open(path).map_err(|e| io(path, e))?;
	let before = file.metadata().map_err(|e| io(path, e))?;
	let mut buffer = [0u8; 64 * 1024];
	let mut hash = Xxh3::new();
	loop {
		let count = file.read(&mut buffer).map_err(|e| io(path, e))?;
		if count == 0 {
			break;
		}
		hash.update(&buffer[..count]);
	}
	let after = file.metadata().map_err(|e| io(path, e))?;
	if before.len() != after.len() || before.modified().ok() != after.modified().ok() {
		return Err(Error::Changed(path.to_path_buf()));
	}
	Ok(hash.digest128().to_le_bytes())
}

pub fn walk(root: &Path, mut emit: impl FnMut(Entry) -> bool) -> Report {
	let mut report = Report::default();
	// Sample packs often contain dot directories or .gitignore files intended for other tools.
	for entry in WalkBuilder::new(root)
		.standard_filters(false)
		.follow_links(false)
		.build()
	{
		match entry {
			Ok(entry) if entry.file_type().is_some_and(|kind| kind.is_file()) && supported(entry.path()) => {
				match fingerprint(root, entry.path()) {
					Ok(entry) => {
						report.files += 1;
						if !emit(entry) {
							break;
						}
					}
					Err(error) => report.errors.push(error.to_string()),
				}
			}
			Ok(_) => {}
			Err(error) => report.errors.push(error.to_string()),
		}
	}
	report
}

pub struct Watch {
	_watcher: RecommendedWatcher,
	dirty: Arc<AtomicBool>,
}

impl Watch {
	pub fn new(path: &Path) -> Result<Self, Error> {
		let dirty = Arc::new(AtomicBool::new(false));
		let flag = dirty.clone();
		// Coalescing to a flag bounds memory during large copies; errors also force a rescan.
		let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
			if !event
				.as_ref()
				.is_ok_and(|event| matches!(event.kind, notify::EventKind::Access(_)))
			{
				flag.store(true, Ordering::Release);
			}
		})?;
		watcher.watch(path, RecursiveMode::Recursive)?;
		Ok(Self {
			_watcher: watcher,
			dirty,
		})
	}

	pub fn take_dirty(&self) -> bool {
		self.dirty.swap(false, Ordering::AcqRel)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use xxhash_rust::xxh3::xxh3_64;

	#[test]
	fn quick_hash_matches_boundary_definition() {
		let dir = tempfile::tempdir().unwrap();
		for size in [0, 65536, 131072, 131073, 200000] {
			let bytes: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
			let path = dir.path().join("test.WAV");
			std::fs::write(&path, &bytes).unwrap();
			let mut input = (size as u64).to_le_bytes().to_vec();
			if size <= 131072 {
				input.extend(&bytes);
			} else {
				input.extend(&bytes[..65536]);
				input.extend(&bytes[size - 65536..]);
			}
			assert_eq!(
				fingerprint(dir.path(), &path).unwrap().quick_hash,
				xxh3_64(&input).to_le_bytes()
			);
		}
	}

	#[test]
	fn only_supported_files_are_emitted_including_hidden() {
		let dir = tempfile::tempdir().unwrap();
		for name in [".kick.WAV", "a.aif", "b.aiff", "c.flac", "d.mp3", "e.ogg", "cover.png"] {
			std::fs::write(dir.path().join(name), b"fixture").unwrap();
		}
		let report = walk(dir.path(), |_| true);
		assert_eq!(report.files, 6);
		assert!(report.errors.is_empty());
	}
}
