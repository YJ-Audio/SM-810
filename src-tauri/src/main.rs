#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use sampler_engine::{
	Engine,
	audition::{Auditioner, Settings},
};
use serde_json::{Value, json};
use std::{
	path::PathBuf,
	sync::{
		Arc, Mutex,
		atomic::{AtomicBool, AtomicU64, Ordering},
	},
	time::Duration,
};
use tauri::{Emitter, Manager, State};

struct App {
	engine: Arc<Engine>,
	audio: Mutex<Option<Auditioner>>,
	audio_error: Option<String>,
	analyzing: Arc<AtomicBool>,
	embedding: Arc<AtomicBool>,
	pause_embedding: Arc<AtomicBool>,
	downloading: Arc<AtomicBool>,
	download_bytes: Arc<AtomicU64>,
	download_total: Arc<AtomicU64>,
}
type Reply = Result<Value, String>;
fn json_result<T: serde::Serialize>(value: sampler_engine::Result<T>) -> Reply {
	value
		.map_err(|e| e.to_string())
		.and_then(|value| serde_json::to_value(value).map_err(|e| e.to_string()))
}
async fn work(f: impl FnOnce() -> Reply + Send + 'static) -> Reply {
	tauri::async_runtime::spawn_blocking(f)
		.await
		.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn bootstrap(state: State<'_, App>) -> Reply {
	let engine = state.engine.clone();
	let audio_error = state.audio_error.clone();
	let analyzing = state.analyzing.load(Ordering::Acquire);
	let embedding = state.embedding.load(Ordering::Acquire);
	let downloading = state.downloading.load(Ordering::Acquire);
	let download_bytes = state.download_bytes.load(Ordering::Relaxed);
	let download_total = state.download_total.load(Ordering::Relaxed);
	work(move || {
		Ok(json!({
			"roots": engine.roots().map_err(|e| e.to_string())?,
			"tags": engine.tags().map_err(|e| e.to_string())?,
			"jobs": engine.jobs().map_err(|e| e.to_string())?,
			"audio_error": audio_error,
			"analyzing": analyzing,
			"embedding": embedding, "model_ready": engine.model_ready(), "embedded": engine.embedding_count(),
			"downloading": downloading, "download_bytes": download_bytes, "download_total": download_total,
		}))
	})
	.await
}
#[tauri::command]
async fn browse(state: State<'_, App>, query: sampler_db::BrowseQuery) -> Reply {
	let engine = state.engine.clone();
	work(move || {
		json_result((|| {
			let result = engine.browse(query)?;
			engine.prioritize_embeddings(result.items.iter().map(|r| r.sample.id).collect())?;
			Ok(result)
		})())
	})
	.await
}
#[tauri::command]
async fn waveform(state: State<'_, App>, id: i64) -> Reply {
	let engine = state.engine.clone();
	work(move || json_result(engine.waveform(id))).await
}
#[tauri::command]
fn audition(state: State<'_, App>, id: Option<i64>, settings: Settings) -> Result<(), String> {
	let guard = state.audio.lock().map_err(|e| e.to_string())?;
	guard
		.as_ref()
		.ok_or_else(|| state.audio_error.clone().unwrap_or("Audio unavailable".into()))?
		.play(id, settings)
		.map_err(|e| e.to_string())
}
#[tauri::command]
fn prefetch(state: State<'_, App>, ids: Vec<i64>, settings: Settings) -> Result<(), String> {
	if let Some(audio) = state.audio.lock().map_err(|e| e.to_string())?.as_ref() {
		audio.prefetch(ids, settings);
	}
	Ok(())
}
#[tauri::command]
fn playback_status(state: State<'_, App>) -> Reply {
	let guard = state.audio.lock().map_err(|e| e.to_string())?;
	Ok(guard
		.as_ref()
		.map(|audio| json!(audio.status()))
		.unwrap_or_else(|| json!({"playing":false,"sample_id":0,"seconds":0,"error":state.audio_error})))
}
#[tauri::command]
async fn add_source(state: State<'_, App>, path: PathBuf, label: String, storage: sampler_db::Storage) -> Reply {
	let engine = state.engine.clone();
	work(move || {
		let id = engine.add_root(&path, &label, storage).map_err(|e| e.to_string())?;
		json_result(engine.scan(id))
	})
	.await
}
#[tauri::command]
async fn rescan(state: State<'_, App>, id: i64) -> Reply {
	let engine = state.engine.clone();
	work(move || json_result(engine.scan(id))).await
}
#[tauri::command]
async fn edit_tag(state: State<'_, App>, ids: Vec<i64>, name: String, remove: bool) -> Reply {
	let engine = state.engine.clone();
	work(move || {
		for id in ids {
			if remove {
				engine.remove_tag(id, name.clone())
			} else {
				engine.tag(id, &name)
			}
			.map_err(|e| e.to_string())?;
		}
		Ok(Value::Null)
	})
	.await
}
#[tauri::command]
async fn edit_metadata(
	state: State<'_, App>,
	id: i64,
	bpm: Option<f64>,
	key: Option<u8>,
	mode: Option<String>,
) -> Reply {
	let engine = state.engine.clone();
	work(move || json_result(engine.set_manual(id, bpm, key, mode))).await
}
#[tauri::command]
async fn snap_slice(state: State<'_, App>, id: i64, start: usize, end: usize) -> Reply {
	let engine = state.engine.clone();
	work(move || json_result(engine.snap_slice(id, start, end))).await
}
#[tauri::command]
async fn prepare_drag(state: State<'_, App>, id: i64, start: Option<usize>, end: Option<usize>) -> Reply {
	let engine = state.engine.clone();
	work(move || match (start, end) {
		(Some(a), Some(b)) => json_result(engine.export_slice(id, a, b)),
		(None, None) => json_result(engine.file_path(id)),
		_ => Err("Both slice endpoints are required".into()),
	})
	.await
}
#[tauri::command]
fn start_analysis(app: tauri::AppHandle, state: State<'_, App>) -> Result<(), String> {
	if state.analyzing.swap(true, Ordering::AcqRel) {
		return Ok(());
	}
	let engine = state.engine.clone();
	let active = state.analyzing.clone();
	std::thread::spawn(move || {
		let result = engine.analyze_pending(usize::MAX);
		active.store(false, Ordering::Release);
		let _ = app.emit("library-updated", result.err().map(|e| e.to_string()));
	});
	Ok(())
}
#[tauri::command]
async fn similar(state: State<'_, App>, id: i64) -> Reply {
	let engine = state.engine.clone();
	work(move || json_result(engine.similar(id, 5))).await
}
#[tauri::command]
fn pause_embedding(state: State<'_, App>) {
	state.pause_embedding.store(true, Ordering::Release);
}
#[tauri::command]
fn start_embedding(app: tauri::AppHandle, state: State<'_, App>) -> Result<(), String> {
	if state.embedding.swap(true, Ordering::AcqRel) {
		return Ok(());
	}
	state.pause_embedding.store(false, Ordering::Release);
	let engine = state.engine.clone();
	let active = state.embedding.clone();
	let pause = state.pause_embedding.clone();
	std::thread::spawn(move || {
		let result = engine.embed_until(usize::MAX, || pause.load(Ordering::Acquire));
		active.store(false, Ordering::Release);
		let _ = app.emit("library-updated", result.err().map(|e| e.to_string()));
	});
	Ok(())
}
#[tauri::command]
async fn download_model(state: State<'_, App>) -> Reply {
	if state.downloading.swap(true, Ordering::AcqRel) {
		return Err("Model download is already running".into());
	}
	let engine = state.engine.clone();
	let active = state.downloading.clone();
	let bytes = state.download_bytes.clone();
	let total = state.download_total.clone();
	work(move || {
		let result = engine.download_model(|done, size| {
			bytes.store(done, Ordering::Relaxed);
			total.store(size, Ordering::Relaxed);
		});
		active.store(false, Ordering::Release);
		json_result(result)
	})
	.await
}
fn main() {
	tauri::Builder::default()
		.plugin(tauri_plugin_dialog::init())
		.plugin(tauri_plugin_drag::init())
		.setup(|app| {
			let dirs =
				directories::ProjectDirs::from("studio", "poti", "sampler").ok_or("No application data directory")?;
			// The CLI and desktop share one database; the engine never depends on Tauri.
			let path = std::env::var_os("SAMPLER_DB")
				.map(PathBuf::from)
				.unwrap_or_else(|| dirs.data_local_dir().join("library.sqlite3"));
			let engine = Arc::new(Engine::open(path)?);
			let (audio, audio_error) = match Auditioner::open(engine.clone()) {
				Ok(audio) => (Some(audio), None),
				Err(error) => (None, Some(error.to_string())),
			};
			app.manage(App {
				engine,
				audio: Mutex::new(audio),
				audio_error,
				analyzing: Arc::new(AtomicBool::new(false)),
				embedding: Arc::new(AtomicBool::new(false)),
				pause_embedding: Arc::new(AtomicBool::new(false)),
				downloading: Arc::new(AtomicBool::new(false)),
				download_bytes: Arc::new(AtomicU64::new(0)),
				download_total: Arc::new(AtomicU64::new(0)),
			});
			let handle = app.handle().clone();
			std::thread::spawn(move || {
				loop {
					std::thread::sleep(Duration::from_secs(2));
					let state = handle.state::<App>();
					if state.analyzing.load(Ordering::Acquire)
						|| state.embedding.load(Ordering::Acquire)
						|| state.downloading.load(Ordering::Acquire)
					{
						let _ = handle.emit("analysis-progress", ());
					}
				}
			});
			Ok(())
		})
		.invoke_handler(tauri::generate_handler![
			bootstrap,
			browse,
			waveform,
			audition,
			playback_status,
			prefetch,
			add_source,
			rescan,
			edit_tag,
			edit_metadata,
			snap_slice,
			prepare_drag,
			start_analysis,
			similar,
			start_embedding,
			pause_embedding,
			download_model
		])
		.run(tauri::generate_context!())
		.expect("Tauri application initialization failed");
}
