use sampler_embed::{Model, preprocess::Preprocessor};
use std::path::Path;
fn floats(path: &Path) -> Vec<f32> {
	std::fs::read(path)
		.unwrap()
		.as_chunks::<4>()
		.0
		.iter()
		.map(|v| f32::from_le_bytes(*v))
		.collect()
}
#[test]
fn log_mel_matches_transformers_including_short_one_shot_padding() {
	let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
	let processor = Preprocessor::default();
	for name in ["short_kick", "noise_hit", "tone", "silence"] {
		let actual = processor.features(&floats(&dir.join(format!("{name}.pcm")))).unwrap();
		let expected = floats(&dir.join(format!("{name}.mel")));
		assert_eq!(actual.len(), expected.len());
		let error = actual
			.iter()
			.zip(&expected)
			.map(|(a, b)| (a - b).abs())
			.fold(0.0f32, f32::max);
		assert!(error < 0.002, "{name}: max log-mel error {error}");
	}
}
#[test]
#[ignore = "requires pinned CLAP models in SAMPLER_MODEL_DIR"]
fn rust_audio_and_text_embeddings_match_original_transformers() {
	let directory = std::env::var_os("SAMPLER_MODEL_DIR").expect("SAMPLER_MODEL_DIR");
	let accelerate = std::env::var_os("SAMPLER_TEST_ACCELERATION").is_some();
	let mut model = Model::with_acceleration(Path::new(&directory), accelerate).unwrap();
	let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
	let fixture: serde_json::Value =
		serde_json::from_slice(&std::fs::read(dir.join("reference.json")).unwrap()).unwrap();
	for record in fixture["records"].as_array().unwrap() {
		let (name, actual) = if let Some(name) = record["name"].as_str() {
			(name, model.audio(&floats(&dir.join(format!("{name}.pcm")))).unwrap())
		} else {
			let text = record["text"].as_str().unwrap();
			(text, model.text(text).unwrap())
		};
		let mut expected: Vec<f32> = record["embedding"]
			.as_array()
			.unwrap()
			.iter()
			.map(|v| v.as_f64().unwrap() as f32)
			.collect();
		sampler_similarity::normalize(&mut expected).unwrap();
		let cosine: f32 = actual.iter().zip(&expected).map(|(a, b)| a * b).sum();
		eprintln!("{name}: cosine {cosine}");
		assert!(cosine >= 0.99, "{name}: cosine {cosine}");
	}
}
