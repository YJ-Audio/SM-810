use sampler_db::Storage;
use sampler_engine::Engine;
use std::fs;

#[test]
fn rename_duplicate_delete_disconnect_and_relink_preserve_identity_and_tags() {
	let dir = tempfile::tempdir().unwrap();
	let source = dir.path().join("source");
	fs::create_dir(&source).unwrap();
	let original = source.join("KCK_dark_03.wav");
	fs::write(&original, b"generated-audio-content").unwrap();
	let engine = Engine::open(dir.path().join("library.db")).unwrap();
	let root = engine.add_root(&source, "Main", Storage::External).unwrap();
	assert_eq!(engine.scan(root).unwrap().files, 1);
	let id = engine.search("dark", 10, 0).unwrap()[0].id;
	engine.tag(id, "Drums/kick").unwrap();
	let renamed = source.join("renamed.wav");
	fs::rename(&original, &renamed).unwrap();
	engine.scan(root).unwrap();
	let sample = &engine.search("renamed", 10, 0).unwrap()[0];
	assert_eq!(sample.id, id);
	assert!(sample.available);
	assert_eq!(sample.tags, vec!["kick"]);
	fs::copy(&renamed, source.join("duplicate.wav")).unwrap();
	engine.scan(root).unwrap();
	assert_eq!(engine.search("", 10, 0).unwrap().len(), 1);
	assert_eq!(engine.roots().unwrap()[0].files, 3);
	engine.verify(10).unwrap();
	fs::remove_file(&renamed).unwrap();
	engine.scan(root).unwrap();
	assert!(engine.search("", 10, 0).unwrap()[0].available);
	let detached = dir.path().join("detached");
	fs::rename(&source, &detached).unwrap();
	assert_eq!(engine.scan(root).unwrap().status, "offline");
	assert!(!engine.search("", 10, 0).unwrap()[0].available);
	assert_eq!(engine.roots().unwrap()[0].files, 3);
	engine.relocate_root(root, &detached).unwrap();
	engine.scan(root).unwrap();
	let sample = &engine.search("kick", 10, 0).unwrap()[0];
	assert!(sample.available);
	assert_eq!(sample.id, id);
	assert_eq!(sample.tags, vec!["kick"]);
	assert!(sample.path.starts_with(fs::canonicalize(detached).unwrap()));
}

#[test]
fn quick_collision_splits_on_full_verification_and_copies_tags() {
	let dir = tempfile::tempdir().unwrap();
	let root_path = dir.path().join("source");
	fs::create_dir(&root_path).unwrap();
	let a = vec![0u8; 200000];
	let mut b = a.clone();
	b[100000] = 1;
	fs::write(root_path.join("a.wav"), &a).unwrap();
	fs::write(root_path.join("b.wav"), &b).unwrap();
	let engine = Engine::open(dir.path().join("library.db")).unwrap();
	let root = engine.add_root(&root_path, "Main", Storage::Local).unwrap();
	engine.scan(root).unwrap();
	let samples = engine.search("", 10, 0).unwrap();
	assert_eq!(samples.len(), 1);
	engine.tag(samples[0].id, "collision").unwrap();
	engine.verify(10).unwrap();
	let samples = engine.search("", 10, 0).unwrap();
	assert_eq!(samples.len(), 2);
	assert!(samples.iter().all(|s| s.tags == vec!["collision"]));
	let ids: Vec<_> = samples.iter().map(|s| s.id).collect();
	engine.scan(root).unwrap();
	engine.verify(10).unwrap();
	assert_eq!(
		engine
			.search("", 10, 0)
			.unwrap()
			.iter()
			.map(|s| s.id)
			.collect::<Vec<_>>(),
		ids
	);
	assert_eq!(fs::read(root_path.join("a.wav")).unwrap(), a);
	assert_eq!(fs::read(root_path.join("b.wav")).unwrap(), b);
}

#[test]
fn replacing_a_path_does_not_inherit_unrelated_tags() {
	let dir = tempfile::tempdir().unwrap();
	let source = dir.path().join("source");
	fs::create_dir(&source).unwrap();
	let path = source.join("sound.wav");
	fs::write(&path, b"first").unwrap();
	let engine = Engine::open(dir.path().join("library.db")).unwrap();
	let root = engine.add_root(&source, "Main", Storage::Local).unwrap();
	engine.scan(root).unwrap();
	let old = engine.search("", 10, 0).unwrap()[0].id;
	engine.tag(old, "old").unwrap();
	fs::write(&path, b"different audio").unwrap();
	engine.scan(root).unwrap();
	let sample = &engine.search("sound", 10, 0).unwrap()[0];
	assert_ne!(sample.id, old);
	assert!(sample.tags.is_empty());
}

#[test]
fn short_queries_and_punctuation_are_literal_and_pages_do_not_overlap() {
	let dir = tempfile::tempdir().unwrap();
	let source = dir.path().join("source");
	fs::create_dir(&source).unwrap();
	for (i, name) in ["a_100%.wav", "b_日本語.wav", "c_quo\"te.wav"].iter().enumerate() {
		fs::write(source.join(name), [i as u8]).unwrap();
	}
	let engine = Engine::open(dir.path().join("library.db")).unwrap();
	let root = engine.add_root(&source, "Main", Storage::Local).unwrap();
	engine.scan(root).unwrap();
	for query in ["%", "日本", "日本語", "quo\"te"] {
		assert_eq!(engine.search(query, 10, 0).unwrap().len(), 1);
	}
	assert_ne!(
		engine.search("", 1, 0).unwrap()[0].id,
		engine.search("", 1, 1).unwrap()[0].id
	);
}

#[test]
fn analysis_jobs_resume_and_preserve_manual_values_and_peaks() {
	let dir = tempfile::tempdir().unwrap();
	let source = dir.path().join("source");
	fs::create_dir(&source).unwrap();
	let path = source.join("tone_120bpm_Am.wav");
	let mut writer = hound::WavWriter::create(
		&path,
		hound::WavSpec {
			channels: 1,
			sample_rate: 48000,
			bits_per_sample: 16,
			sample_format: hound::SampleFormat::Int,
		},
	)
	.unwrap();
	for i in 0..9600 {
		writer
			.write_sample(((i as f32 * 440.0 * std::f32::consts::TAU / 48000.0).sin() * 12000.0) as i16)
			.unwrap();
	}
	writer.finalize().unwrap();
	let db = dir.path().join("test.db");
	let engine = Engine::open(&db).unwrap();
	let root = engine.add_root(&source, "Test", Storage::Local).unwrap();
	engine.scan(root).unwrap();
	let id = engine.search("", 1, 0).unwrap()[0].id;
	engine
		.write(|tx| {
			tx.execute("UPDATE jobs SET state='running',attempts=1 WHERE kind='analyze'", [])?;
			Ok(())
		})
		.unwrap();
	drop(engine);
	let engine = Engine::open(&db).unwrap();
	engine.analyze_pending(10).unwrap();
	assert_eq!(engine.analysis(id).unwrap().unwrap().bpm, Some(120.0));
	assert_eq!(engine.peaks(id).unwrap().len(), 512);
	engine
		.set_manual(id, Some(127.0), Some(4), Some("major".into()))
		.unwrap();
	engine
		.write(move |tx| {
			tx.execute("UPDATE analysis SET analyzer_ver=0 WHERE sample_id=?1", [id])?;
			Ok(())
		})
		.unwrap();
	engine.analyze_pending(10).unwrap();
	let result = engine.analysis(id).unwrap().unwrap();
	assert_eq!(result.bpm, Some(127.0));
	assert_eq!(result.bpm_source.as_deref(), Some("manual"));
	assert_eq!(result.key_root, Some(4));
	assert_eq!(result.key_mode.as_deref(), Some("major"));
	assert_eq!(result.key_source.as_deref(), Some("manual"));
	assert_eq!(result.analyzer_ver, sampler_analysis::ANALYZER_VERSION);
	drop(engine);
	let engine = Engine::open(&db).unwrap();
	assert_eq!(engine.peaks(id).unwrap().len(), 512);
}

#[test]
fn slice_export_is_exact_persistent_reused_and_never_changes_source() {
	let dir = tempfile::tempdir().unwrap();
	let source = dir.path().join("source");
	fs::create_dir(&source).unwrap();
	let path = source.join("stereo.wav");
	let mut writer = hound::WavWriter::create(
		&path,
		hound::WavSpec {
			channels: 2,
			sample_rate: 48000,
			bits_per_sample: 32,
			sample_format: hound::SampleFormat::Float,
		},
	)
	.unwrap();
	let values: Vec<f32> = (0..1000)
		.flat_map(|i| [i as f32 / 1000.0, -(i as f32) / 1000.0])
		.collect();
	for value in &values {
		writer.write_sample(*value).unwrap();
	}
	writer.finalize().unwrap();
	let original = fs::read(&path).unwrap();
	let db = dir.path().join("library.db");
	let engine = Engine::open(&db).unwrap();
	let root = engine.add_root(&source, "Source", Storage::Local).unwrap();
	engine.scan(root).unwrap();
	let id = engine.search("", 1, 0).unwrap()[0].id;
	let exported = engine.export_slice(id, 100, 300).unwrap();
	assert!(exported.starts_with(dir.path().join("slices")));
	let mut reader = hound::WavReader::open(&exported).unwrap();
	assert_eq!(reader.spec().channels, 2);
	assert_eq!(reader.duration(), 200);
	assert_eq!(
		reader.samples::<f32>().collect::<Result<Vec<_>, _>>().unwrap(),
		values[200..600]
	);
	let modified = fs::metadata(&exported).unwrap().modified().unwrap();
	drop(engine);
	let engine = Engine::open(&db).unwrap();
	assert_eq!(engine.export_slice(id, 100, 300).unwrap(), exported);
	assert_eq!(fs::metadata(exported).unwrap().modified().unwrap(), modified);
	assert!(engine.export_slice(id, 300, 100).is_err());
	assert!(engine.export_slice(id, 0, 1001).is_err());
	assert_eq!(fs::read(path).unwrap(), original);
}

#[test]
fn browsing_filters_hierarchy_and_root_before_paging() {
	let dir = tempfile::tempdir().unwrap();
	let source = dir.path().join("source");
	fs::create_dir(&source).unwrap();
	for (i, name) in ["kick.wav", "snare.wav", "pad.wav"].iter().enumerate() {
		fs::write(source.join(name), [i as u8]).unwrap();
	}
	let engine = Engine::open(dir.path().join("db")).unwrap();
	let root = engine.add_root(&source, "Source", Storage::Local).unwrap();
	engine.scan(root).unwrap();
	let kick = engine.search("kick", 1, 0).unwrap()[0].id;
	engine.tag(kick, "Drums/One shots/kick").unwrap();
	let snare = engine.search("snare", 1, 0).unwrap()[0].id;
	engine.tag(snare, "Drums/One shots/snare").unwrap();
	let request = sampler_db::BrowseQuery {
		text: String::new(),
		root_id: Some(root),
		tag: Some("Drums/One shots".into()),
		offset: 0,
		limit: Some(1),
	};
	let page = engine.browse(request.clone()).unwrap();
	assert_eq!(page.total, 2);
	assert_eq!(page.items.len(), 1);
	assert_eq!(page.items[0].sample.id, kick);
	let page = engine
		.browse(sampler_db::BrowseQuery {
			offset: 1,
			..request.clone()
		})
		.unwrap();
	assert_eq!(page.items[0].sample.id, snare);
	assert_eq!(
		engine
			.browse(sampler_db::BrowseQuery {
				root_id: Some(9999),
				..request
			})
			.unwrap()
			.total,
		0
	);
}

#[test]
fn library_lock_prevents_concurrent_job_recovery_and_releases_on_close() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("library.db");
	let first = Engine::open(&path).unwrap();
	assert!(matches!(Engine::open(&path), Err(sampler_engine::Error::Invalid(_))));
	drop(first);
	let second = Engine::open(&path).unwrap();
	assert!(second.roots().unwrap().is_empty());
}
