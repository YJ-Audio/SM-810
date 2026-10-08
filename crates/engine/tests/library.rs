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
		collection_id: None,
		map_id: None,
		ids: None,
		similar_to: None,
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

#[test]
fn similarity_survives_restart_filters_before_paging_and_separates_model_revisions() {
	use sampler_db::BrowseQuery;
	use sampler_embed::{DIMENSIONS, MODEL};
	let dir = tempfile::tempdir().unwrap();
	let source = dir.path().join("sounds");
	fs::create_dir(&source).unwrap();
	for (i, name) in ["kick", "snare", "hat", "pad"].iter().enumerate() {
		fs::write(source.join(format!("{name}.wav")), [i as u8]).unwrap();
	}
	let path = dir.path().join("library.db");
	let engine = Engine::open(&path).unwrap();
	let root = engine.add_root(&source, "Test", Storage::Local).unwrap();
	engine.scan(root).unwrap();
	let ids: Vec<_> = ["kick", "snare", "hat", "pad"]
		.into_iter()
		.map(|name| engine.search(name, 1, 0).unwrap()[0].id)
		.collect();
	engine.tag(ids[1], "Drums/snare").unwrap();
	engine.tag(ids[2], "Drums/hat").unwrap();
	for (i, &id) in ids.iter().enumerate() {
		let mut vector = vec![0.0f32; DIMENSIONS];
		vector[0] = 1.0;
		vector[1] = i as f32;
		let bytes: Vec<_> = vector.iter().flat_map(|v| v.to_le_bytes()).collect();
		engine
			.write(move |tx| {
				tx.execute(
					"INSERT INTO embeddings(model,sample_id,dim,vec) VALUES(?1,?2,?3,?4)",
					(
						if i == 3 { "obsolete-model" } else { MODEL },
						id,
						DIMENSIONS as i64,
						bytes,
					),
				)?;
				Ok(())
			})
			.unwrap();
	}
	drop(engine);
	let engine = Engine::open(&path).unwrap();
	assert_eq!(engine.embedding_count(), 3);
	// The source bytes are deliberately not audio: this search must use the stored vectors.
	let neighbors = engine.similar(ids[0], 5).unwrap();
	assert_eq!(neighbors.iter().map(|r| r.sample.id).collect::<Vec<_>>(), ids[1..3]);
	assert!((neighbors[0].similarity.unwrap() - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
	let request = BrowseQuery {
		similar_to: Some(ids[0]),
		root_id: Some(root),
		tag: Some("Drums".into()),
		limit: Some(1),
		offset: 1,
		..Default::default()
	};
	let page = engine.browse(request.clone()).unwrap();
	assert_eq!(page.total, 2);
	assert_eq!(page.items[0].sample.id, ids[2]);
	assert_eq!(
		engine
			.browse(BrowseQuery {
				ids: Some(vec![ids[1]]),
				offset: 0,
				..request.clone()
			})
			.unwrap()
			.items[0]
			.sample
			.id,
		ids[1]
	);
	assert_eq!(
		engine
			.browse(BrowseQuery {
				root_id: Some(-1),
				..request
			})
			.unwrap()
			.total,
		0
	);
	let page = engine
		.browse(BrowseQuery {
			similar_to: Some(ids[0]),
			text: "snare".into(),
			..Default::default()
		})
		.unwrap();
	assert_eq!(page.total, 1);
	assert_eq!(page.items[0].sample.id, ids[1]);
}

#[test]
fn shared_queries_combine_hierarchical_tags_fields_and_negation() {
	use sampler_engine::query::{Comparison, Field, Query};
	let dir = tempfile::tempdir().unwrap();
	let source = dir.path().join("source");
	fs::create_dir(&source).unwrap();
	for (i, name) in ["kick", "snare", "pad"].into_iter().enumerate() {
		fs::write(source.join(format!("{name}.wav")), [i as u8]).unwrap();
	}
	let engine = Engine::open(dir.path().join("db")).unwrap();
	let root = engine.add_root(&source, "Test", Storage::Local).unwrap();
	engine.scan(root).unwrap();
	let ids: Vec<_> = ["kick", "snare", "pad"]
		.into_iter()
		.map(|name| engine.search(name, 1, 0).unwrap()[0].id)
		.collect();
	for &id in &ids[..2] {
		engine.tag(id, "Drums/one-shot").unwrap();
	}
	let id = ids[0];
	engine.write(move |tx|{tx.execute("INSERT INTO analysis(sample_id,analyzer_ver,duration_ms,sample_rate,channels,lufs,is_loop) VALUES(?1,1,150,48000,1,-10,0)",[id])?;Ok(())}).unwrap();
	let query = Query::All {
		conditions: vec![
			Query::Tag { name: "Drums".into() },
			Query::Field {
				field: Field::DurationMs,
				op: Comparison::Lt,
				value: 200.0,
			},
			Query::Not {
				condition: Box::new(Query::Text { text: "pad".into() }),
			},
		],
	};
	let json = serde_json::to_string(&query).unwrap();
	let decoded: Query = serde_json::from_str(&json).unwrap();
	assert_eq!(
		engine.query_ids(&decoded).unwrap(),
		std::collections::HashSet::from([ids[0]])
	);
	assert_eq!(
		engine
			.query_ids(&Query::Any {
				conditions: vec![Query::Text { text: "pad".into() }, Query::Tag { name: "Drums".into() }]
			})
			.unwrap()
			.len(),
		3
	);
	assert_eq!(
		engine
			.query_ids(&Query::Not {
				condition: Box::new(Query::Root { id: root })
			})
			.unwrap()
			.len(),
		0
	);
	assert_eq!(engine.query_ids(&Query::Any { conditions: vec![] }).unwrap().len(), 0);
	assert_eq!(engine.query_ids(&Query::default()).unwrap().len(), 3);
}

#[test]
fn maps_persist_positions_and_dim_filters_without_moving_points() {
	use sampler_db::BrowseQuery;
	use sampler_embed::{DIMENSIONS, MODEL};
	let dir = tempfile::tempdir().unwrap();
	let source = dir.path().join("sounds");
	fs::create_dir(&source).unwrap();
	for i in 0..18 {
		fs::write(
			source.join(format!("{}_{}.wav", if i < 9 { "kick" } else { "pad" }, i)),
			[i as u8],
		)
		.unwrap();
	}
	let path = dir.path().join("library.db");
	let engine = Engine::open(&path).unwrap();
	let root = engine.add_root(&source, "Test", Storage::Local).unwrap();
	engine.scan(root).unwrap();
	for sample in engine.search("", 100, 0).unwrap() {
		let mut vector = vec![0f32; DIMENSIONS];
		vector[0] = 1.0;
		vector[1] = sample.id as f32 / 20.0;
		let bytes: Vec<_> = vector.iter().flat_map(|v| v.to_le_bytes()).collect();
		engine
			.write(move |tx| {
				tx.execute(
					"INSERT INTO embeddings(model,sample_id,dim,vec) VALUES(?1,?2,?3,?4)",
					(MODEL, sample.id, DIMENSIONS as i64, bytes),
				)?;
				Ok(())
			})
			.unwrap();
	}
	drop(engine);
	let engine = Engine::open(&path).unwrap();
	engine.ensure_maps().unwrap();
	let id = engine.maps().unwrap()[0].id;
	let all = engine.map_binary(id, BrowseQuery::default()).unwrap();
	let filtered = engine
		.map_binary(
			id,
			BrowseQuery {
				text: "kick".into(),
				..Default::default()
			},
		)
		.unwrap();
	assert_eq!(&all[..4], b"MAP1");
	assert_eq!(u32::from_le_bytes(all[24..28].try_into().unwrap()), 18);
	let mut matched = 0;
	for (a, b) in all[32..]
		.as_chunks::<32>()
		.0
		.iter()
		.zip(filtered[32..].as_chunks::<32>().0)
	{
		assert_eq!(&a[..18], &b[..18]);
		assert_eq!(&a[19..], &b[19..]);
		if b[18] & 1 != 0 {
			matched += 1;
		}
	}
	assert_eq!(matched, 9);
	assert_eq!(engine.map_summary(id).unwrap().provisional, 18);
	drop(engine);
	let engine = Engine::open(&path).unwrap();
	assert_eq!(engine.map_binary(id, BrowseQuery::default()).unwrap(), all);
	let sidecar = std::env::var_os("SAMPLER_LAYOUT_BIN")
		.map(std::path::PathBuf::from)
		.unwrap_or_else(|| {
			std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
				.join("../../target/debug")
				.join(if cfg!(windows) {
					"sampler-layout.exe"
				} else {
					"sampler-layout"
				})
		});
	assert!(
		sidecar.exists(),
		"Run npm --prefix ui run prepare-layout before workspace tests"
	);
	engine.recompute_map(id, &sidecar, || false).unwrap();
	assert_eq!(engine.map_summary(id).unwrap().layout_rev, 1);
	assert_eq!(engine.map_summary(id).unwrap().provisional, 0);
	let first_projection = engine.map_binary(id, BrowseQuery::default()).unwrap();
	assert!(engine.recompute_map(id, &sidecar, || true).is_err());
	assert_eq!(engine.map_binary(id, BrowseQuery::default()).unwrap(), first_projection);
	engine.recompute_map(id, &sidecar, || false).unwrap();
	let second_projection = engine.map_binary(id, BrowseQuery::default()).unwrap();
	for (a, b) in first_projection[32..]
		.as_chunks::<32>()
		.0
		.iter()
		.zip(second_projection[32..].as_chunks::<32>().0)
	{
		for offset in [8, 12] {
			let x = f32::from_le_bytes(a[offset..offset + 4].try_into().unwrap());
			let y = f32::from_le_bytes(b[offset..offset + 4].try_into().unwrap());
			assert!((x - y).abs() < 1e-5, "Recomputation must retain orientation and scale");
		}
	}
	drop(engine);
	let engine = Engine::open(&path).unwrap();
	assert_eq!(
		engine.map_binary(id, BrowseQuery::default()).unwrap(),
		second_projection
	);
	let map = engine
		.maps()
		.unwrap()
		.into_iter()
		.find(|m| m.name.starts_with("Drums"))
		.unwrap();
	assert_eq!(
		engine
			.browse(BrowseQuery {
				map_id: Some(map.id),
				..Default::default()
			})
			.unwrap()
			.total,
		9
	);
}

#[test]
fn rule_reapplication_preserves_manual_tags_and_overlapping_rules() {
	use sampler_engine::organize::{Rule, RuleTarget};
	let dir = tempfile::tempdir().unwrap();
	let source = dir.path().join("sounds");
	fs::create_dir(&source).unwrap();
	fs::write(source.join("Kick_dark.wav"), b"kick").unwrap();
	fs::write(source.join("Snare_bright.wav"), b"snare").unwrap();
	let engine = Engine::open(dir.path().join("library.db")).unwrap();
	let root = engine.add_root(&source, "Test", Storage::Local).unwrap();
	engine.scan(root).unwrap();
	let kick = engine.search("Kick_dark", 10, 0).unwrap()[0].id;
	let snare = engine.search("Snare_bright", 10, 0).unwrap()[0].id;
	let rule = Rule {
		id: None,
		tag: "Drums/hit".into(),
		target: RuleTarget::Filename,
		pattern: "(?i)kick".into(),
		enabled: true,
	};
	engine.save_rule(rule.clone()).unwrap();
	engine.save_rule(rule).unwrap();
	engine.apply_rules().unwrap();
	engine.tag(kick, "Mood/keep").unwrap();
	engine.tag(snare, "Drums/hit").unwrap();
	let mut rule = engine.rules().unwrap()[0].clone();
	rule.pattern = "(?i)snare".into();
	engine.save_rule(rule.clone()).unwrap();
	engine.apply_rules().unwrap();
	// The second rule still contributes the kick; the manual snare must stay manual.
	assert!(
		engine.search("Kick_dark", 10, 0).unwrap()[0]
			.tags
			.contains(&"hit".to_string())
	);
	let second = engine.rules().unwrap()[1].id.unwrap();
	engine.delete_rule(second).unwrap();
	let tags = &engine.search("Kick_dark", 10, 0).unwrap()[0].tags;
	assert_eq!(tags, &["keep"]);
	rule.enabled = false;
	engine.save_rule(rule.clone()).unwrap();
	engine.apply_rules().unwrap();
	assert_eq!(engine.search("Snare_bright", 10, 0).unwrap()[0].tags, &["hit"]);
	let provenance: Option<i64> = engine
		.reader()
		.unwrap()
		.query_row("SELECT rule_id FROM sample_tags WHERE sample_id=?1", [snare], |r| {
			r.get(0)
		})
		.unwrap();
	assert_eq!(provenance, None);
	rule.pattern = "[".into();
	assert!(engine.save_rule(rule).is_err());
	assert_eq!(engine.rules().unwrap().len(), 1);
	assert!(engine.search("keep", 10, 0).unwrap().iter().any(|s| s.id == kick));
	assert!(!engine.search("hit", 10, 0).unwrap().iter().any(|s| s.id == kick));
}
#[test]
fn collections_share_queries_keep_manual_order_and_persist() {
	use sampler_db::BrowseQuery;
	use sampler_engine::query::Query;
	let dir = tempfile::tempdir().unwrap();
	let source = dir.path().join("sounds");
	fs::create_dir(&source).unwrap();
	for (name, bytes) in [("alpha.wav", b"alpha".as_slice()), ("zeta.wav", b"zeta".as_slice())] {
		fs::write(source.join(name), bytes).unwrap();
	}
	let path = dir.path().join("library.db");
	let engine = Engine::open(&path).unwrap();
	let root = engine.add_root(&source, "Test", Storage::Local).unwrap();
	engine.scan(root).unwrap();
	let alpha = engine.search("alpha", 10, 0).unwrap()[0].id;
	let zeta = engine.search("zeta", 10, 0).unwrap()[0].id;
	let id = engine.save_collection(None, "Ideas".into(), None).unwrap();
	engine
		.edit_collection_items(id, vec![zeta, alpha, zeta], false)
		.unwrap();
	let request = || BrowseQuery {
		collection_id: Some(id),
		..Default::default()
	};
	assert_eq!(
		engine
			.browse(request())
			.unwrap()
			.items
			.iter()
			.map(|r| r.sample.id)
			.collect::<Vec<_>>(),
		[zeta, alpha]
	);
	engine.move_collection_item(id, alpha, true).unwrap();
	assert_eq!(engine.browse(request()).unwrap().items[0].sample.id, alpha);
	let smart = engine
		.save_collection(
			None,
			"Tagged".into(),
			Some(Query::Tag {
				name: "Mood/bright".into(),
			}),
		)
		.unwrap();
	assert_eq!(engine.collection_ids(smart).unwrap().len(), 0);
	engine.tag(alpha, "Mood/bright").unwrap();
	assert_eq!(engine.collection_ids(smart).unwrap().len(), 1);
	assert!(engine.edit_collection_items(smart, vec![zeta], false).is_err());
	assert_eq!(engine.query_ids(&Query::Collection { id }).unwrap().len(), 2);
	let dependent = engine
		.save_collection(None, "Following".into(), Some(Query::Collection { id: smart }))
		.unwrap();
	assert!(
		engine
			.save_collection(Some(smart), "Cycle".into(), Some(Query::Collection { id: dependent }))
			.is_err()
	);
	assert_eq!(engine.collection_ids(dependent).unwrap().len(), 1);
	engine.delete_collection(dependent).unwrap();
	drop(engine);
	let engine = Engine::open(&path).unwrap();
	assert_eq!(engine.collections().unwrap().len(), 2);
	assert_eq!(engine.browse(request()).unwrap().items[0].sample.id, alpha);
	engine.edit_collection_items(id, vec![alpha], true).unwrap();
	assert_eq!(engine.browse(request()).unwrap().total, 1);
	assert!(source.join("alpha.wav").exists());
}
