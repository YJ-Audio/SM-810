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
		atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering},
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
	layout_active: Arc<AtomicI64>,
	layout_cancel: Arc<AtomicBool>,
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
	let layout_active = state.layout_active.load(Ordering::Acquire);
	work(move || {
		Ok(json!({
			"maps": engine.maps().map_err(|e|e.to_string())?, "layout_active": layout_active,
			"roots": engine.roots().map_err(|e| e.to_string())?,
			"collections": engine.collections().map_err(|e|e.to_string())?,
			"rules": engine.rules().map_err(|e|e.to_string())?,
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
fn audition(
	state: State<'_, App>,
	id: Option<i64>,
	settings: Settings,
	issued_ms: Option<f64>,
	probe: Option<u64>,
) -> Result<(), String> {
	let guard = state.audio.lock().map_err(|e| e.to_string())?;
	guard
		.as_ref()
		.ok_or_else(|| state.audio_error.clone().unwrap_or("Audio unavailable".into()))?
		.play_measured(id, settings, issued_ms, probe)
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
#[tauri::command]
async fn sample(state: State<'_, App>, id: i64) -> Reply {
	let engine = state.engine.clone();
	work(move || {
		json_result(engine.browse(sampler_db::BrowseQuery {
			ids: Some(vec![id]),
			limit: Some(1),
			..Default::default()
		}))
	})
	.await
}
#[tauri::command]
async fn map_points(
	state: State<'_, App>,
	id: i64,
	query: sampler_db::BrowseQuery,
) -> Result<tauri::ipc::Response, String> {
	let engine = state.engine.clone();
	tauri::async_runtime::spawn_blocking(move || engine.map_binary(id, query))
		.await
		.map_err(|e| e.to_string())?
		.map(tauri::ipc::Response::new)
		.map_err(|e| e.to_string())
}
#[tauri::command]
async fn map_summary(state: State<'_, App>, id: i64) -> Reply {
	let engine = state.engine.clone();
	work(move || json_result(engine.map_summary(id))).await
}
#[tauri::command]
async fn create_map(state: State<'_, App>, name: String, query: sampler_engine::query::Query) -> Reply {
	let engine = state.engine.clone();
	work(move || json_result(engine.create_map(name, query))).await
}
#[tauri::command]
fn cancel_layout(state: State<'_, App>) {
	state.layout_cancel.store(true, Ordering::Release);
}
#[tauri::command]
async fn save_rule(state: State<'_, App>, rule: sampler_engine::organize::Rule) -> Reply {
	let engine = state.engine.clone();
	work(move || {
		engine.save_rule(rule).map_err(|e| e.to_string())?;
		json_result(engine.apply_rules())
	})
	.await
}
#[tauri::command]
async fn apply_rules(state: State<'_, App>) -> Reply {
	let engine = state.engine.clone();
	work(move || json_result(engine.apply_rules())).await
}
#[tauri::command]
async fn delete_rule(state: State<'_, App>, id: i64) -> Reply {
	let engine = state.engine.clone();
	work(move || json_result(engine.delete_rule(id))).await
}
#[tauri::command]
async fn save_collection(
	state: State<'_, App>,
	id: Option<i64>,
	name: String,
	query: Option<sampler_engine::query::Query>,
) -> Reply {
	let engine = state.engine.clone();
	work(move || json_result(engine.save_collection(id, name, query))).await
}
#[tauri::command]
async fn edit_collection_items(state: State<'_, App>, id: i64, ids: Vec<i64>, remove: bool) -> Reply {
	let engine = state.engine.clone();
	work(move || json_result(engine.edit_collection_items(id, ids, remove))).await
}
#[tauri::command]
async fn move_collection_item(state: State<'_, App>, id: i64, sample: i64, earlier: bool) -> Reply {
	let engine = state.engine.clone();
	work(move || json_result(engine.move_collection_item(id, sample, earlier))).await
}
#[tauri::command]
async fn delete_collection(state: State<'_, App>, id: i64) -> Reply {
	let engine = state.engine.clone();
	work(move || json_result(engine.delete_collection(id))).await
}
#[tauri::command]
fn start_layout(app: tauri::AppHandle, state: State<'_, App>, id: i64) -> Result<(), String> {
	if id <= 0 {
		return Err("Invalid map ID".into());
	}
	state
		.layout_active
		.compare_exchange(0, id, Ordering::AcqRel, Ordering::Acquire)
		.map_err(|_| "A layout is already being recomputed")?;
	state.layout_cancel.store(false, Ordering::Release);
	let engine = state.engine.clone();
	let active = state.layout_active.clone();
	let cancel = state.layout_cancel.clone();
	let program = std::env::var_os("SAMPLER_LAYOUT_BIN").map(PathBuf::from).or_else(|| {
		std::env::current_exe().ok().and_then(|p| {
			p.parent().map(|p| {
				p.join(if cfg!(windows) {
					"sampler-layout.exe"
				} else {
					"sampler-layout"
				})
			})
		})
	});
	std::thread::spawn(move || {
		let result = program
			.ok_or_else(|| sampler_engine::Error::Invalid("Layout sidecar was not found".into()))
			.and_then(|program| engine.recompute_map(id, &program, || cancel.load(Ordering::Acquire)));
		active.store(0, Ordering::Release);
		let error = if cancel.load(Ordering::Acquire) {
			None
		} else {
			result.err().map(|e| e.to_string())
		};
		let _ = app.emit("library-updated", error);
	});
	Ok(())
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
			engine.ensure_maps()?;
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
				layout_active: Arc::new(AtomicI64::new(0)),
				layout_cancel: Arc::new(AtomicBool::new(false)),
				download_bytes: Arc::new(AtomicU64::new(0)),
				download_total: Arc::new(AtomicU64::new(0)),
			});
			if std::env::var_os("SAMPLER_MAP_BENCH").is_some() {
				let window = app.get_webview_window("main").ok_or("No benchmark window")?;
				let mut url = window.url()?;
				url.set_path("/bench.html");
				window.navigate(url)?;
				window.set_title("Sampler · Map Benchmark")?;
			}
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
			download_model,
			sample,
			map_points,
			map_summary,
			create_map,
			start_layout,
			cancel_layout,
			save_rule,
			apply_rules,
			delete_rule,
			save_collection,
			edit_collection_items,
			move_collection_item,
			delete_collection
		])
		.build(tauri::generate_context!())
		.expect("Tauri application initialization failed")
		.run(|app, event| {
			if matches!(event, tauri::RunEvent::ExitRequested { .. }) {
				let state = app.state::<App>();
				state.layout_cancel.store(true, Ordering::Release);
				// Reap the sidecar before the process exits so cancelled work cannot remain orphaned.
				for _ in 0..100 {
					if state.layout_active.load(Ordering::Acquire) == 0 {
						break;
					}
					std::thread::sleep(Duration::from_millis(10));
				}
			}
		});
}
