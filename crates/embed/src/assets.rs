use crate::Error;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
	fs::{self, File},
	io::{Read, Write},
	path::Path,
};
#[derive(Deserialize)]
struct Manifest {
	repository: String,
	revision: String,
	assets: Vec<Asset>,
}
#[derive(Deserialize)]
struct Asset {
	path: String,
	bytes: u64,
	sha256: String,
}
fn manifest() -> Manifest {
	serde_json::from_str(include_str!("../../../tools/models/clap-manifest.json")).expect("embedded model manifest")
}
fn valid(path: &Path, asset: &Asset) -> bool {
	let Ok(mut file) = File::open(path) else {
		return false;
	};
	if file.metadata().map(|m| m.len()).unwrap_or(0) != asset.bytes {
		return false;
	}
	let mut sha = Sha256::new();
	let mut bytes = [0u8; 65536];
	loop {
		match file.read(&mut bytes) {
			Ok(0) => break,
			Ok(n) => sha.update(&bytes[..n]),
			Err(_) => return false,
		}
	}
	sha.finalize().iter().map(|b| format!("{b:02x}")).collect::<String>() == asset.sha256
}
pub fn is_installed(directory: &Path) -> bool {
	manifest()
		.assets
		.iter()
		.all(|a| fs::metadata(directory.join(&a.path)).is_ok_and(|m| m.len() == a.bytes))
}
pub fn verify(directory: &Path) -> Result<(), Error> {
	for asset in manifest().assets {
		if !valid(&directory.join(&asset.path), &asset) {
			return Err(Error::Invalid(format!(
				"CLAP asset is missing or has changed: {}. Download the model again.",
				asset.path
			)));
		}
	}
	Ok(())
}
pub fn download(directory: &Path, mut progress: impl FnMut(u64, u64)) -> Result<(), Error> {
	let manifest = manifest();
	let total = manifest.assets.iter().map(|a| a.bytes).sum();
	let mut done = 0;
	for asset in &manifest.assets {
		let destination = directory.join(&asset.path);
		if !valid(&destination, asset) {
			if let Some(parent) = destination.parent() {
				fs::create_dir_all(parent).map_err(|e| Error::Invalid(e.to_string()))?;
			}
			let url = format!(
				"https://huggingface.co/{}/resolve/{}/{}",
				manifest.repository, manifest.revision, asset.path
			);
			let mut response = ureq::get(&url)
				.call()
				.map_err(|e| Error::Invalid(format!("Model download: {e}")))?;
			let mut reader = response.body_mut().as_reader();
			let mut file = tempfile::NamedTempFile::new_in(destination.parent().expect("model asset parent"))
				.map_err(|e| Error::Invalid(e.to_string()))?;
			let mut buffer = [0u8; 65536];
			let mut received = 0;
			loop {
				let n = reader.read(&mut buffer).map_err(|e| Error::Invalid(e.to_string()))?;
				if n == 0 {
					break;
				}
				received += n as u64;
				if received > asset.bytes {
					return Err(Error::Invalid("Model download exceeded expected size".into()));
				}
				file.write_all(&buffer[..n])
					.map_err(|e| Error::Invalid(e.to_string()))?;
				progress(done + received, total);
			}
			file.as_file().sync_all().map_err(|e| Error::Invalid(e.to_string()))?;
			if !valid(file.path(), asset) {
				return Err(Error::Invalid(format!(
					"Model download checksum mismatch: {}",
					asset.path
				)));
			}
			file.persist(destination).map_err(|e| Error::Invalid(e.to_string()))?;
		}
		done += asset.bytes;
		progress(done, total);
	}
	Ok(())
}
