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
