pub use rusqlite::OptionalExtension;
use rusqlite::params;
pub use rusqlite::{Connection, Transaction};
use rusqlite_migration::{M, Migrations};
use serde::{Deserialize, Serialize};
use std::{
	path::{Path, PathBuf},
	time::Duration,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
	#[error("I/O: {0}")]
	Io(#[from] std::io::Error),
	#[error("database: {0}")]
	Sql(#[from] rusqlite::Error),
	#[error("migration: {0}")]
	Migration(#[from] rusqlite_migration::Error),
	#[error("invalid input: {0}")]
	Invalid(String),
}
pub type Result<T> = std::result::Result<T, Error>;
const MIGRATION_STEPS: &[M<'static>] = &[
	M::up(include_str!("../migrations/001_initial.sql")),
	M::up(include_str!("../migrations/002_scan_state.sql")),
	M::up(include_str!("../migrations/003_preview_complete.sql")),
];
const MIGRATIONS: Migrations<'static> = Migrations::from_slice(MIGRATION_STEPS);

pub fn open_writer(path: &Path) -> Result<Connection> {
	let mut db = Connection::open(path)?;
	db.busy_timeout(Duration::from_secs(5))?;
	db.pragma_update(None, "journal_mode", "WAL")?;
	// Migration 2 rebuilds the identity table while preserving all referencing rows.
	db.pragma_update(None, "foreign_keys", false)?;
	MIGRATIONS.to_latest(&mut db)?;
	db.pragma_update(None, "foreign_keys", true)?;
	let broken: Option<String> = db
		.query_row("PRAGMA foreign_key_check", [], |row| row.get(0))
		.optional()?;
	if let Some(table) = broken {
		return Err(Error::Invalid(format!("foreign key check failed: {table}")));
	}
	db.execute("UPDATE jobs SET state='pending' WHERE state='running'", [])?;
	db.execute(
		"UPDATE root_state SET status='partial',error='Previous scan interrupted' WHERE status='scanning'",
		[],
	)?;
	Ok(db)
}

pub fn open_reader(path: &Path) -> Result<Connection> {
	let db = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
	db.busy_timeout(Duration::from_secs(5))?;
	db.pragma_update(None, "foreign_keys", true)?;
	Ok(db)
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Storage {
	#[default]
	Local,
	External,
	Network,
}
impl Storage {
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Local => "local",
			Self::External => "external",
			Self::Network => "network",
		}
	}
}

#[derive(Debug, Clone, Serialize)]
pub struct Root {
	pub id: i64,
	pub path: PathBuf,
	pub label: String,
	pub storage: String,
	pub enabled: bool,
	pub status: String,
	pub files: i64,
	pub missing: i64,
}

pub fn roots(db: &Connection) -> Result<Vec<Root>> {
	let mut stmt = db.prepare("SELECT r.id,r.path,r.label,r.storage,r.enabled,s.status,COUNT(f.id),SUM(CASE WHEN f.id IS NOT NULL AND (s.status='offline' OR f.last_seen_scan<s.complete_generation) THEN 1 ELSE 0 END) FROM roots r JOIN root_state s ON s.root_id=r.id LEFT JOIN files f ON f.root_id=r.id GROUP BY r.id ORDER BY r.id")?;
	Ok(stmt
		.query_map([], |r| {
			Ok(Root {
				id: r.get(0)?,
				path: PathBuf::from(r.get::<_, String>(1)?),
				label: r.get(2)?,
				storage: r.get(3)?,
				enabled: r.get(4)?,
				status: r.get(5)?,
				files: r.get(6)?,
				missing: r.get(7)?,
			})
		})?
		.collect::<std::result::Result<_, _>>()?)
}

pub fn add_root(db: &Transaction<'_>, path: &str, label: &str, storage: Storage) -> Result<i64> {
	if label.trim().is_empty() {
		return Err(Error::Invalid("source label is empty".into()));
	}
	db.execute("INSERT INTO roots(path,label,storage) VALUES(?1,?2,?3) ON CONFLICT(path) DO UPDATE SET label=excluded.label,storage=excluded.storage", params![path,label,storage.as_str()])?;
	let id = db.query_row("SELECT id FROM roots WHERE path=?1", [path], |r| r.get(0))?;
	db.execute("INSERT OR IGNORE INTO root_state(root_id) VALUES(?1)", [id])?;
	Ok(id)
}

pub fn relocate_root(db: &Transaction<'_>, id: i64, path: &str) -> Result<()> {
	if db.execute("UPDATE roots SET path=?1 WHERE id=?2", params![path, id])? == 0 {
		return Err(Error::Invalid("unknown source".into()));
	}
	Ok(())
}

pub fn begin_scan(db: &Transaction<'_>, root_id: i64) -> Result<i64> {
	if db.execute(
		"UPDATE root_state SET next_generation=next_generation+1,status='scanning',error=NULL WHERE root_id=?1",
		[root_id],
	)? == 0
	{
		return Err(Error::Invalid("unknown source".into()));
	}
	Ok(db.query_row(
		"SELECT next_generation FROM root_state WHERE root_id=?1",
		[root_id],
		|r| r.get(0),
	)?)
}

pub fn finish_scan(db: &Transaction<'_>, id: i64, generation: i64, status: &str, error: Option<&str>) -> Result<()> {
	db.execute("UPDATE root_state SET status=?1,error=?2,complete_generation=CASE WHEN ?1='online' THEN ?3 ELSE complete_generation END WHERE root_id=?4", params![status,error,generation,id])?;
	Ok(())
}

pub struct FileInput<'a> {
	pub rel_path: &'a str,
	pub size: u64,
	pub mtime: i64,
	pub quick_hash: &'a [u8],
	pub preview_cache: bool,
}

fn enqueue(db: &Connection, sample_id: i64, preview: bool) -> Result<()> {
	for kind in ["full_hash", "analyze", "embed", "peaks"] {
		db.execute(
			"INSERT OR IGNORE INTO jobs(sample_id,kind) VALUES(?1,?2)",
			params![sample_id, kind],
		)?;
	}
	if preview {
		db.execute(
			"INSERT OR IGNORE INTO jobs(sample_id,kind) VALUES(?1,'preview_cache')",
			[sample_id],
		)?;
	}
	Ok(())
}

pub fn upsert_file(db: &Transaction<'_>, root: i64, generation: i64, file: FileInput<'_>) -> Result<i64> {
	let old: Option<(i64,i64,i64,Vec<u8>)> = db.query_row("SELECT f.sample_id,f.mtime,s.size,s.quick_hash FROM files f JOIN samples s ON s.id=f.sample_id WHERE f.root_id=?1 AND f.rel_path=?2",params![root,file.rel_path],|r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
	let unchanged = old.as_ref().is_some_and(|(_, mtime, size, hash)| {
		*mtime == file.mtime && *size == file.size as i64 && hash == file.quick_hash
	});
	let id = if let Some((id, _, _, _)) = old.as_ref().filter(|_| unchanged) {
		*id
	} else {
		match db
			.query_row(
				"SELECT id FROM samples WHERE size=?1 AND quick_hash=?2 ORDER BY id LIMIT 1",
				params![file.size as i64, file.quick_hash],
				|r| r.get(0),
			)
			.optional()?
		{
			Some(id) => id,
			None => {
				db.execute(
					"INSERT INTO samples(size,quick_hash) VALUES(?1,?2)",
					params![file.size as i64, file.quick_hash],
				)?;
				db.last_insert_rowid()
			}
		}
	};
	db.execute("INSERT INTO files(sample_id,root_id,rel_path,mtime,last_seen_scan) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(root_id,rel_path) DO UPDATE SET sample_id=excluded.sample_id,mtime=excluded.mtime,last_seen_scan=excluded.last_seen_scan",params![id,root,file.rel_path,file.mtime,generation])?;
	enqueue(db, id, file.preview_cache)?;
	if !unchanged {
		db.execute(
			"UPDATE jobs SET state='pending',error=NULL WHERE sample_id=?1 AND kind='full_hash'",
			[id],
		)?;
	}
	rebuild_search(db, id)?;
	if let Some((old_id, _, _, _)) = old.filter(|(old_id, _, _, _)| *old_id != id) {
		rebuild_search(db, old_id)?;
	}
	Ok(id)
}

pub fn rebuild_search(db: &Connection, id: i64) -> Result<()> {
	db.execute("DELETE FROM search_fts WHERE rowid=?1", [id])?;
	db.execute("INSERT INTO search_fts(rowid,name,paths,tags) SELECT ?1,COALESCE((SELECT group_concat(rel_path,' ') FROM files WHERE sample_id=?1),''),COALESCE((SELECT group_concat(r.path || '/' || f.rel_path,' ') FROM files f JOIN roots r ON r.id=f.root_id WHERE sample_id=?1),''),COALESCE((SELECT group_concat(t.name,' ') FROM sample_tags st JOIN tags t ON t.id=st.tag_id WHERE sample_id=?1),'')",[id])?;
	Ok(())
}

#[derive(Debug, Clone, Serialize)]
pub struct Sample {
	pub id: i64,
	pub name: String,
	pub path: PathBuf,
	pub root_id: i64,
	pub available: bool,
	pub tags: Vec<String>,
	pub tag_paths: Vec<String>,
	pub size: u64,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct BrowseQuery {
	pub collection_id: Option<i64>,
	pub map_id: Option<i64>,
	pub ids: Option<Vec<i64>>,
	pub similar_to: Option<i64>,
	#[serde(default)]
	pub text: String,
	pub root_id: Option<i64>,
	pub tag: Option<String>,
	#[serde(default)]
	pub offset: usize,
	pub limit: Option<usize>,
}
#[derive(Debug, Serialize)]
pub struct Page {
	pub items: Vec<Sample>,
	pub total: usize,
}
pub fn search(db: &Connection, text: &str, limit: usize, offset: usize) -> Result<Vec<Sample>> {
	Ok(browse(
		db,
		&BrowseQuery {
			text: text.into(),
			offset,
			limit: Some(limit),
			..Default::default()
		},
	)?
	.items)
}
fn filters(request: &BrowseQuery) -> (String, String) {
	let text = &request.text;
	let quoted = format!("\"{}\"", text.replace('"', "\"\""));
	let pattern = format!(
		"%{}%",
		text.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
	);
	let (filter, query) = if text.is_empty() {
		("1=1", String::new())
	} else if text.chars().count() >= 3 {
		(
			"s.id IN (SELECT rowid FROM search_fts WHERE search_fts MATCH ?1)",
			quoted,
		)
	} else {
		(
			"s.id IN (SELECT rowid FROM search_fts WHERE name LIKE ?1 ESCAPE '\\' OR paths LIKE ?1 ESCAPE '\\' OR tags LIKE ?1 ESCAPE '\\')",
			pattern,
		)
	};
	let filter = format!(
		"({filter}) AND (?6 IS NULL OR s.id IN (SELECT value FROM json_each(?6))) AND (?4 IS NULL OR f.root_id=?4) AND (?5 IS NULL OR s.id IN (WITH RECURSIVE tag_paths(id,path) AS (SELECT id,name FROM tags WHERE parent_id IS NULL UNION ALL SELECT t.id,p.path || '/' || t.name FROM tags t JOIN tag_paths p ON t.parent_id=p.id), descendants(id) AS (SELECT id FROM tags WHERE name=?5 COLLATE NOCASE OR id IN (SELECT id FROM tag_paths WHERE path=?5 COLLATE NOCASE) UNION ALL SELECT tags.id FROM tags JOIN descendants ON tags.parent_id=descendants.id) SELECT st.sample_id FROM sample_tags st JOIN descendants d ON d.id=st.tag_id))"
	);
	(filter, query)
}
fn ids_json(request: &BrowseQuery) -> Option<String> {
	request
		.ids
		.as_ref()
		.map(|ids| format!("[{}]", ids.iter().map(i64::to_string).collect::<Vec<_>>().join(",")))
}
pub fn matching_ids(db: &Connection, request: &BrowseQuery) -> Result<Vec<i64>> {
	let (filter, query) = filters(request);
	let mut statement = db.prepare(&format!(
		"SELECT DISTINCT s.id FROM samples s JOIN files f ON f.sample_id=s.id WHERE {filter} AND ?2 IS NULL AND ?3 IS NULL"
	))?;
	Ok(statement
		.query_map(
			params![
				query,
				Option::<i64>::None,
				Option::<i64>::None,
				request.root_id,
				request.tag,
				ids_json(request)
			],
			|row| row.get(0),
		)?
		.collect::<std::result::Result<Vec<_>, _>>()?)
}
pub fn browse(db: &Connection, request: &BrowseQuery) -> Result<Page> {
	let (filter, query) = filters(request);
	let limit = request.limit.unwrap_or(100);
	let offset = request.offset;
	let total:usize=db.query_row(&format!("SELECT COUNT(DISTINCT s.id) FROM samples s JOIN files f ON f.sample_id=s.id WHERE {filter} AND ?2 IS NULL AND ?3 IS NULL"),params![query,Option::<i64>::None,Option::<i64>::None,request.root_id,request.tag,ids_json(request)],|row|Ok(row.get::<_,i64>(0)? as usize))?;
	let sql = format!(
		"WITH ranked AS (SELECT s.id,s.size,f.rel_path,r.path,r.id AS root_id,(r.enabled AND rs.status!='offline' AND f.last_seen_scan>=rs.complete_generation) AS available,ROW_NUMBER() OVER (PARTITION BY s.id ORDER BY (r.enabled AND rs.status!='offline' AND f.last_seen_scan>=rs.complete_generation) DESC,f.id) AS rank FROM samples s JOIN files f ON f.sample_id=s.id JOIN roots r ON r.id=f.root_id JOIN root_state rs ON rs.root_id=r.id WHERE {filter}) SELECT id,size,rel_path,path,root_id,available FROM ranked WHERE rank=1 ORDER BY (SELECT ci.position FROM collection_items ci WHERE ci.collection_id=?7 AND ci.sample_id=ranked.id),rel_path COLLATE NOCASE,id LIMIT ?2 OFFSET ?3"
	);
	let mut stmt = db.prepare(&sql)?;
	let mut rows = stmt.query(params![
		query,
		limit.min(100000) as i64,
		offset as i64,
		request.root_id,
		request.tag,
		ids_json(request),
		request.collection_id
	])?;
	let mut results = Vec::new();
	while let Some(row) = rows.next()? {
		let id = row.get(0)?;
		let rel: String = row.get(2)?;
		let root: String = row.get(3)?;
		let mut statement = db.prepare("WITH RECURSIVE paths(id,name,path) AS (SELECT id,name,name FROM tags WHERE parent_id IS NULL UNION ALL SELECT t.id,t.name,p.path || '/' || t.name FROM tags t JOIN paths p ON t.parent_id=p.id) SELECT p.name,p.path FROM paths p JOIN sample_tags st ON st.tag_id=p.id WHERE sample_id=?1 ORDER BY p.name,p.path")?;
		let pairs = statement
			.query_map([id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
			.collect::<std::result::Result<Vec<_>, _>>()?;
		let (tags, tag_paths) = pairs.into_iter().unzip();
		results.push(Sample {
			id,
			size: row.get::<_, i64>(1)? as u64,
			name: Path::new(&rel)
				.file_stem()
				.unwrap_or_default()
				.to_string_lossy()
				.into_owned(),
			path: PathBuf::from(root).join(rel),
			root_id: row.get(4)?,
			available: row.get(5)?,
			tags,
			tag_paths,
		});
	}
	Ok(Page { items: results, total })
}

pub fn ensure_tag(db: &Transaction<'_>, name: &str) -> Result<i64> {
	let mut parent: Option<i64> = None;
	for component in name.split('/') {
		let component = component.trim();
		if component.is_empty() {
			return Err(Error::Invalid("empty tag component".into()));
		}
		db.execute(
			"INSERT OR IGNORE INTO tags(parent_id,name) VALUES(?1,?2)",
			params![parent, component],
		)?;
		parent = Some(db.query_row(
			"SELECT id FROM tags WHERE parent_id IS ?1 AND name=?2",
			params![parent, component],
			|r| r.get(0),
		)?);
	}
	parent.ok_or_else(|| Error::Invalid("empty tag path".into()))
}
pub fn tag(db: &Transaction<'_>, sample: i64, name: &str) -> Result<()> {
	let tag = ensure_tag(db, name)?;
	db.execute("INSERT INTO sample_tags(sample_id,tag_id,rule_id) VALUES(?1,?2,NULL) ON CONFLICT(sample_id,tag_id) DO UPDATE SET rule_id=NULL",params![sample,tag])?;
	rebuild_search(db, sample)?;
	Ok(())
}

#[derive(Debug, Clone)]
pub struct HashFile {
	pub id: i64,
	pub sample_id: i64,
	pub path: PathBuf,
	pub mtime: i64,
	pub size: u64,
	pub quick_hash: Vec<u8>,
}
pub fn hash_files(db: &Connection, sample: i64) -> Result<Vec<HashFile>> {
	let mut stmt=db.prepare("SELECT f.id,f.sample_id,r.path,f.rel_path,f.mtime,s.size,s.quick_hash FROM files f JOIN roots r ON r.id=f.root_id JOIN root_state rs ON rs.root_id=r.id JOIN samples s ON s.id=f.sample_id WHERE f.sample_id=?1 AND r.enabled=1 AND rs.status!='offline' AND f.last_seen_scan>=rs.complete_generation")?;
	Ok(stmt
		.query_map([sample], |r| {
			Ok(HashFile {
				id: r.get(0)?,
				sample_id: r.get(1)?,
				path: PathBuf::from(r.get::<_, String>(2)?).join(r.get::<_, String>(3)?),
				mtime: r.get(4)?,
				size: r.get::<_, i64>(5)? as u64,
				quick_hash: r.get(6)?,
			})
		})?
		.collect::<std::result::Result<_, _>>()?)
}

pub fn store_hash(db: &Transaction<'_>, file_id: i64, sample: i64, hash: &[u8]) -> Result<i64> {
	let existing: Option<Vec<u8>> =
		db.query_row("SELECT full_hash FROM samples WHERE id=?1", [sample], |r| r.get(0))?;
	let target = match existing {
		None => {
			db.execute("UPDATE samples SET full_hash=?1 WHERE id=?2", params![hash, sample])?;
			sample
		}
		Some(old) if old == hash => sample,
		Some(_) => {
			let target:Option<i64>=db.query_row("SELECT id FROM samples WHERE full_hash=?1 AND (size,quick_hash)=(SELECT size,quick_hash FROM samples WHERE id=?2)",params![hash,sample],|r|r.get(0)).optional()?;
			let target = if let Some(target) = target {
				target
			} else {
				db.execute(
					"INSERT INTO samples(size,quick_hash,full_hash) SELECT size,quick_hash,?1 FROM samples WHERE id=?2",
					params![hash, sample],
				)?;
				let id = db.last_insert_rowid();
				enqueue(db, id, true)?;
				id
			};
			db.execute("INSERT INTO sample_tags(sample_id,tag_id,rule_id) SELECT ?1,tag_id,rule_id FROM sample_tags WHERE sample_id=?2 ON CONFLICT(sample_id,tag_id) DO UPDATE SET rule_id=CASE WHEN sample_tags.rule_id IS NULL OR excluded.rule_id IS NULL THEN NULL ELSE sample_tags.rule_id END",params![target,sample])?;
			db.execute(
				"UPDATE files SET sample_id=?1 WHERE id=?2 AND sample_id=?3",
				params![target, file_id, sample],
			)?;
			rebuild_search(db, sample)?;
			target
		}
	};
	rebuild_search(db, target)?;
	Ok(target)
}

pub fn claim_hash_job(db: &Transaction<'_>) -> Result<Option<i64>> {
	let id: Option<i64> = db
		.query_row(
			"SELECT sample_id FROM jobs WHERE kind='full_hash' AND state='pending' ORDER BY priority DESC,sample_id LIMIT 1",
			[],
			|r| r.get(0),
		)
		.optional()?;
	if let Some(id) = id {
		db.execute(
			"UPDATE jobs SET state='running',attempts=attempts+1 WHERE sample_id=?1 AND kind='full_hash'",
			[id],
		)?;
	}
	Ok(id)
}
pub fn finish_hash_job(db: &Transaction<'_>, id: i64, error: Option<&str>) -> Result<()> {
	db.execute(
		"UPDATE jobs SET state=?1,error=?2 WHERE sample_id=?3 AND kind='full_hash'",
		params![if error.is_some() { "failed" } else { "done" }, error, id],
	)?;
	Ok(())
}

#[derive(Debug, Serialize)]
pub struct JobCount {
	pub kind: String,
	pub state: String,
	pub count: i64,
}
pub fn jobs(db: &Connection) -> Result<Vec<JobCount>> {
	let mut stmt = db.prepare("SELECT kind,state,COUNT(*) FROM jobs GROUP BY kind,state ORDER BY kind,state")?;
	Ok(stmt
		.query_map([], |r| {
			Ok(JobCount {
				kind: r.get(0)?,
				state: r.get(1)?,
				count: r.get(2)?,
			})
		})?
		.collect::<std::result::Result<_, _>>()?)
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn migrations_are_valid() {
		MIGRATIONS.validate().unwrap();
	}
	#[test]
	fn reopening_preserves_data_and_recovers_jobs() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("test.db");
		let mut db = open_writer(&path).unwrap();
		let tx = db.transaction().unwrap();
		let root = add_root(&tx, "/samples", "Test", Storage::Local).unwrap();
		let generation = begin_scan(&tx, root).unwrap();
		let sample = upsert_file(
			&tx,
			root,
			generation,
			FileInput {
				rel_path: "kick_dark.wav",
				size: 100,
				mtime: 1,
				quick_hash: &[1; 8],
				preview_cache: false,
			},
		)
		.unwrap();
		tag(&tx, sample, "Drums/kick").unwrap();
		tx.execute("UPDATE jobs SET state='running'", []).unwrap();
		tx.commit().unwrap();
		drop(db);
		let db = open_writer(&path).unwrap();
		assert!(jobs(&db).unwrap().iter().all(|job| job.state == "pending"));
		assert_eq!(search(&db, "ark", 10, 0).unwrap()[0].id, sample);
		assert_eq!(search(&db, "kick", 10, 0).unwrap()[0].tags, vec!["kick"]);
		assert_eq!(roots(&db).unwrap()[0].status, "partial");
		assert!(search(&db, "\" OR *", 10, 0).unwrap().is_empty());
		assert!(search(&db, "%", 10, 0).unwrap().is_empty());
	}
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AnalysisRecord {
	pub analyzer_ver: i64,
	pub duration_ms: i64,
	pub sample_rate: u32,
	pub channels: usize,
	pub lufs: Option<f64>,
	pub peak_dbfs: Option<f64>,
	pub is_loop: Option<bool>,
	pub bpm: Option<f64>,
	pub bpm_source: Option<String>,
	pub key_root: Option<u8>,
	pub key_mode: Option<String>,
	pub key_source: Option<String>,
}
pub fn analysis(db: &Connection, id: i64) -> Result<Option<AnalysisRecord>> {
	Ok(db.query_row("SELECT analyzer_ver,duration_ms,sample_rate,channels,lufs,peak_dbfs,is_loop,bpm,bpm_source,key_root,key_mode,key_source FROM analysis WHERE sample_id=?1",[id],|r|Ok(AnalysisRecord {analyzer_ver:r.get(0)?,duration_ms:r.get(1)?,sample_rate:r.get(2)?,channels:r.get::<_,u32>(3)? as usize,lufs:r.get(4)?,peak_dbfs:r.get(5)?,is_loop:r.get(6)?,bpm:r.get(7)?,bpm_source:r.get(8)?,key_root:r.get(9)?,key_mode:r.get(10)?,key_source:r.get(11)?})).optional()?)
}
pub fn store_analysis(db: &Transaction<'_>, id: i64, a: &AnalysisRecord) -> Result<()> {
	db.execute("INSERT INTO analysis(sample_id,analyzer_ver,duration_ms,sample_rate,channels,lufs,peak_dbfs,is_loop,bpm,bpm_source,key_root,key_mode,key_source) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13) ON CONFLICT(sample_id) DO UPDATE SET analyzer_ver=excluded.analyzer_ver,duration_ms=excluded.duration_ms,sample_rate=excluded.sample_rate,channels=excluded.channels,lufs=excluded.lufs,peak_dbfs=excluded.peak_dbfs,is_loop=excluded.is_loop,bpm=CASE WHEN analysis.bpm_source='manual' THEN analysis.bpm ELSE excluded.bpm END,bpm_source=CASE WHEN analysis.bpm_source='manual' THEN 'manual' ELSE excluded.bpm_source END,key_root=CASE WHEN analysis.key_source='manual' THEN analysis.key_root ELSE excluded.key_root END,key_mode=CASE WHEN analysis.key_source='manual' THEN analysis.key_mode ELSE excluded.key_mode END,key_source=CASE WHEN analysis.key_source='manual' THEN 'manual' ELSE excluded.key_source END",params![id,a.analyzer_ver,a.duration_ms,a.sample_rate,a.channels as i64,a.lufs,a.peak_dbfs,a.is_loop,a.bpm,a.bpm_source,a.key_root,a.key_mode,a.key_source])?;
	Ok(())
}
pub fn set_manual(db: &Transaction<'_>, id: i64, bpm: Option<f64>, key: Option<u8>, mode: Option<&str>) -> Result<()> {
	if bpm.is_some_and(|v| !v.is_finite() || !(20.0..=400.0).contains(&v))
		|| key.is_some_and(|v| v > 11)
		|| mode.is_some_and(|v| !matches!(v, "major" | "minor"))
	{
		return Err(Error::Invalid("invalid BPM or key".into()));
	}
	if analysis(db, id)?.is_none() {
		return Err(Error::Invalid("sample has not been analyzed".into()));
	}
	if let Some(bpm) = bpm {
		db.execute(
			"UPDATE analysis SET bpm=?1,bpm_source='manual' WHERE sample_id=?2",
			params![bpm, id],
		)?;
	}
	if let Some(key) = key {
		db.execute(
			"UPDATE analysis SET key_root=?1,key_mode=?2,key_source='manual' WHERE sample_id=?3",
			params![key, mode, id],
		)?;
	}
	Ok(())
}
#[derive(Debug, Clone)]
pub struct Job {
	pub sample_id: i64,
	pub kind: String,
}
pub fn claim_embedding_job(db: &Transaction<'_>) -> Result<Option<i64>> {
	let id = db
		.query_row(
			"SELECT sample_id FROM jobs WHERE kind='embed' AND state='pending' ORDER BY priority DESC,sample_id LIMIT 1",
			[],
			|r| r.get::<_, i64>(0),
		)
		.optional()?;
	if let Some(id) = id {
		db.execute(
			"UPDATE jobs SET state='running',attempts=attempts+1 WHERE sample_id=?1 AND kind='embed'",
			[id],
		)?;
	}
	Ok(id)
}
pub fn claim_analysis_job(db: &Transaction<'_>) -> Result<Option<Job>> {
	let job=db.query_row("SELECT sample_id,kind FROM jobs j WHERE state='pending' AND (kind='analyze' OR (kind='peaks' AND NOT EXISTS(SELECT 1 FROM jobs a WHERE a.sample_id=j.sample_id AND a.kind='analyze' AND a.state IN ('pending','running')))) ORDER BY priority DESC,sample_id LIMIT 1",[],|r|Ok(Job {sample_id:r.get(0)?,kind:r.get(1)?})).optional()?;
	if let Some(job) = &job {
		db.execute(
			"UPDATE jobs SET state='running',attempts=attempts+1 WHERE sample_id=?1 AND kind=?2",
			params![job.sample_id, job.kind],
		)?;
	}
	Ok(job)
}
pub fn finish_job(db: &Transaction<'_>, job: &Job, error: Option<&str>) -> Result<()> {
	db.execute(
		"UPDATE jobs SET state=?1,error=?2 WHERE sample_id=?3 AND kind=?4",
		params![
			if error.is_some() { "failed" } else { "done" },
			error,
			job.sample_id,
			job.kind
		],
	)?;
	Ok(())
}
pub fn requeue_old_analysis(db: &Transaction<'_>, version: i64) -> Result<()> {
	db.execute("UPDATE jobs SET state='pending',error=NULL WHERE kind='analyze' AND sample_id IN (SELECT sample_id FROM analysis WHERE analyzer_ver<?1)",[version])?;
	Ok(())
}

#[derive(Debug, Serialize)]
pub struct TagSummary {
	pub id: i64,
	pub parent_id: Option<i64>,
	pub name: String,
	pub count: i64,
}
pub fn tags(db: &Connection) -> Result<Vec<TagSummary>> {
	let mut stmt=db.prepare("SELECT t.id,t.parent_id,t.name,COUNT(st.sample_id) FROM tags t LEFT JOIN sample_tags st ON st.tag_id=t.id GROUP BY t.id ORDER BY t.name COLLATE NOCASE")?;
	Ok(stmt
		.query_map([], |r| {
			Ok(TagSummary {
				id: r.get(0)?,
				parent_id: r.get(1)?,
				name: r.get(2)?,
				count: r.get(3)?,
			})
		})?
		.collect::<std::result::Result<_, _>>()?)
}
pub fn remove_tag(db: &Transaction<'_>, id: i64, name: &str) -> Result<()> {
	db.execute(
		"WITH RECURSIVE paths(id,path) AS (SELECT id,name FROM tags WHERE parent_id IS NULL UNION ALL SELECT t.id,p.path || '/' || t.name FROM tags t JOIN paths p ON t.parent_id=p.id) DELETE FROM sample_tags WHERE sample_id=?1 AND tag_id IN (SELECT id FROM paths WHERE path=?2)",
		params![id, name],
	)?;
	rebuild_search(db, id)
}
