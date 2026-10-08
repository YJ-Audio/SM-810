pub mod assets;
pub mod preprocess;
use ort::{
	ep,
	session::{Session, builder::GraphOptimizationLevel},
	value::Tensor,
};
use preprocess::{FRAMES, MELS, Preprocessor, RATE};
use std::{
	collections::VecDeque,
	path::{Path, PathBuf},
	sync::Once,
};
use thiserror::Error;
use tokenizers::{Tokenizer, TruncationParams};

pub const DIMENSIONS: usize = 512;
pub const MODEL: &str =
	"Xenova/clap-htsat-unfused@c28f2883575e590e04d3146ff0713c2448d691ba:repeatpad-slaney-first10-v1";
pub const DIRECTORY: &str = "clap-htsat-unfused-c28f288";
#[derive(Debug, Error)]
pub enum Error {
	#[error("CLAP runtime: {0}")]
	Runtime(#[from] ort::Error),
	#[error("CLAP decode: {0}")]
	Decode(#[from] sampler_decode::Error),
	#[error("CLAP: {0}")]
	Invalid(String),
	#[error("CLAP vector: {0}")]
	Vector(#[from] sampler_similarity::Error),
}
static INIT: Once = Once::new();
fn session(path: PathBuf, accelerate: bool) -> Result<Session, ort::Error> {
	let mut builder = Session::builder()?
		.with_optimization_level(GraphOptimizationLevel::Level3)?
		.with_intra_threads(2)?;
	if accelerate {
		#[cfg(target_os = "macos")]
		{
			builder = builder.with_execution_providers([ep::CoreML::default().build()])?;
		}
		#[cfg(target_os = "windows")]
		{
			builder = builder
				.with_parallel_execution(false)?
				.with_memory_pattern(false)?
				.with_execution_providers([ep::DirectML::default().build()])?;
		}
	}
	builder.commit_from_file(path)
}
pub struct Model {
	audio: Session,
	text: Session,
	tokenizer: Tokenizer,
	preprocess: Preprocessor,
	text_cache: VecDeque<(String, Vec<f32>)>,
}
impl Model {
	pub fn open(directory: &Path) -> Result<Self, Error> {
		Self::with_acceleration(directory, true)
	}
	pub fn with_acceleration(directory: &Path, accelerate: bool) -> Result<Self, Error> {
		assets::verify(directory)?;
		INIT.call_once(|| {
			ort::init().with_telemetry(false).commit();
		});
		let open = |name: &str| {
			let path = directory.join("onnx").join(name);
			session(path.clone(), accelerate).or_else(|_| session(path, false))
		};
		let audio = open("audio_model.onnx")?;
		let text = open("text_model.onnx")?;
		let mut tokenizer =
			Tokenizer::from_file(directory.join("tokenizer.json")).map_err(|e| Error::Invalid(e.to_string()))?;
		tokenizer
			.with_truncation(Some(TruncationParams {
				max_length: 77,
				..Default::default()
			}))
			.map_err(|e| Error::Invalid(e.to_string()))?;
		Ok(Self {
			audio,
			text,
			tokenizer,
			preprocess: Preprocessor::default(),
			text_cache: VecDeque::new(),
		})
	}
	pub fn audio_file(&mut self, path: &Path) -> Result<Vec<f32>, Error> {
		let decoded = sampler_decode::decode_head(path, Some(10.0))?;
		self.audio(&decoded.mono_at(RATE)?)
	}
	pub fn audio(&mut self, mono: &[f32]) -> Result<Vec<f32>, Error> {
		let features = self.preprocess.features(mono).map_err(|e| Error::Invalid(e.into()))?;
		let input = Tensor::from_array(([1usize, 1, FRAMES, MELS], features))?;
		let outputs = self.audio.run(ort::inputs!["input_features" => input])?;
		let (_, data) = outputs["audio_embeds"].try_extract_tensor::<f32>()?;
		normalized(data)
	}
	pub fn text(&mut self, text: &str) -> Result<Vec<f32>, Error> {
		if text.trim().is_empty() {
			return Err(Error::Invalid("Search text is empty".into()));
		}
		if let Some(index) = self.text_cache.iter().position(|(query, _)| query == text) {
			let entry = self.text_cache.remove(index).expect("cache index found above");
			let result = entry.1.clone();
			self.text_cache.push_back(entry);
			return Ok(result);
		}
		let encoding = self
			.tokenizer
			.encode(text, true)
			.map_err(|e| Error::Invalid(e.to_string()))?;
		let ids: Vec<i64> = encoding.get_ids().iter().map(|id| *id as i64).collect();

		let shape = [1usize, ids.len()];
		let outputs = self
			.text
			.run(ort::inputs!["input_ids" => Tensor::from_array((shape, ids))?])?;
		let (_, data) = outputs["text_embeds"].try_extract_tensor::<f32>()?;
		let vector = normalized(data)?;
		if self.text_cache.len() == 32 {
			self.text_cache.pop_front();
		}
		self.text_cache.push_back((text.into(), vector.clone()));
		Ok(vector)
	}
}
fn normalized(data: &[f32]) -> Result<Vec<f32>, Error> {
	if data.len() != DIMENSIONS {
		return Err(Error::Invalid("Unexpected CLAP output dimensions".into()));
	}
	let mut data = data.to_vec();
	sampler_similarity::normalize(&mut data)?;
	Ok(data)
}
