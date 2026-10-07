use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use sampler_engine::Engine;
use std::{
	path::PathBuf,
	time::{Duration, Instant},
};

#[derive(Parser)]
#[command(version, about = "Read-only audio library indexer")]
struct Cli {
	#[arg(long, global = true)]
	db: Option<PathBuf>,
	#[command(subcommand)]
	command: Command,
}
#[derive(Clone, Copy, ValueEnum)]
enum Storage {
	Local,
	External,
	Network,
}
#[derive(Subcommand)]
enum Command {
	Analyze {
		#[arg(long,default_value_t=usize::MAX)]
		limit: usize,
	},
	Metadata {
		sample: i64,
	},
	SetMetadata {
		sample: i64,
		#[arg(long)]
		bpm: Option<f64>,
		#[arg(long)]
		key: Option<u8>,
		#[arg(long)]
		mode: Option<String>,
	},
	RetryFailed,
	Init,
	AddRoot {
		path: PathBuf,
		#[arg(long)]
		label: Option<String>,
		#[arg(long, value_enum, default_value = "local")]
		storage: Storage,
	},
	Roots,
	RelocateRoot {
		id: i64,
		path: PathBuf,
	},
	Scan {
		root: Option<i64>,
	},
	Search {
		#[arg(default_value = "")]
		text: String,
		#[arg(long, default_value_t = 100)]
		limit: usize,
		#[arg(long, default_value_t = 0)]
		offset: usize,
	},
	Tag {
		sample: i64,
		name: String,
	},
	Verify {
		#[arg(long,default_value_t=usize::MAX)]
		limit: usize,
	},
	Jobs,
	Watch {
		root: i64,
		#[arg(long, default_value_t = 30)]
		poll_seconds: u64,
	},
}
fn main() -> Result<()> {
	let cli = Cli::parse();
	let path = cli.db.map(Ok).unwrap_or_else(|| {
		directories::ProjectDirs::from("studio", "poti", "sampler")
			.map(|dirs| dirs.data_local_dir().join("library.sqlite3"))
			.context("No application data directory; use --db")
	})?;
	let engine = Engine::open(&path)?;
	match cli.command {
		Command::Analyze { limit } => println!("Processed {} analysis jobs", engine.analyze_pending(limit)?),
		Command::Metadata { sample } => println!("{}", serde_json::to_string_pretty(&engine.analysis(sample)?)?),
		Command::SetMetadata { sample, bpm, key, mode } => engine.set_manual(sample, bpm, key, mode)?,
		Command::RetryFailed => engine.retry_failed()?,
		Command::Init => println!("{}", path.display()),
		Command::AddRoot { path, label, storage } => {
			let label = label.unwrap_or_else(|| path.file_name().unwrap_or_default().to_string_lossy().into_owned());
			let storage = match storage {
				Storage::Local => sampler_db::Storage::Local,
				Storage::External => sampler_db::Storage::External,
				Storage::Network => sampler_db::Storage::Network,
			};
			println!("{}", engine.add_root(&path, &label, storage)?);
		}
		Command::Roots => println!("{}", serde_json::to_string_pretty(&engine.roots()?)?),
		Command::RelocateRoot { id, path } => engine.relocate_root(id, &path)?,
		Command::Scan { root } => {
			let ids = root
				.map(|id| vec![id])
				.unwrap_or(engine.roots()?.iter().filter(|r| r.enabled).map(|r| r.id).collect());
			for id in ids {
				println!("{}", serde_json::to_string(&engine.scan(id)?)?);
			}
		}
		Command::Search { text, limit, offset } => println!(
			"{}",
			serde_json::to_string_pretty(&engine.search(&text, limit, offset)?)?
		),
		Command::Tag { sample, name } => engine.tag(sample, &name)?,
		Command::Verify { limit } => println!("Verified {} jobs", engine.verify(limit)?),
		Command::Jobs => println!("{}", serde_json::to_string_pretty(&engine.jobs()?)?),
		Command::Watch { root, poll_seconds } => {
			let mut watch = engine.watch(root).ok();
			let mut last = Instant::now();
			println!("{}", serde_json::to_string(&engine.scan(root)?)?);
			loop {
				std::thread::sleep(Duration::from_millis(500));
				if watch.as_ref().is_some_and(|w| w.take_dirty())
					|| last.elapsed() >= Duration::from_secs(poll_seconds.max(1))
				{
					println!("{}", serde_json::to_string(&engine.scan(root)?)?);
					if watch.is_none() {
						watch = engine.watch(root).ok();
					}
					last = Instant::now();
				}
			}
		}
	}
	Ok(())
}
