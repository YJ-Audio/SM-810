use crate::{Engine, Error, Result, ScanReport};
use std::{
	collections::HashMap,
	path::PathBuf,
	sync::Arc,
	time::{Duration, Instant},
};
struct RootWatch {
	path: PathBuf,
	watch: Option<sampler_scan::Watch>,
	last_poll: Instant,
	pending: bool,
}
pub struct LibraryWatch {
	engine: Arc<Engine>,
	roots: HashMap<i64, RootWatch>,
	poll_interval: Duration,
}
impl LibraryWatch {
	pub fn new(engine: Arc<Engine>, poll_interval: Duration) -> Self {
		Self {
			engine,
			roots: HashMap::new(),
			poll_interval,
		}
	}
	/// Called from a maintenance thread; notifications coalesce and offline roots keep their DB rows.
	pub fn tick(&mut self) -> Result<Vec<ScanReport>> {
		let roots = self.engine.roots()?;
		self.roots
			.retain(|id, _| roots.iter().any(|r| r.id == *id && r.enabled));
		let mut reports = Vec::new();
		for root in roots.into_iter().filter(|r| r.enabled) {
			let entry = self.roots.entry(root.id).or_insert_with(|| RootWatch {
				path: root.path.clone(),
				watch: sampler_scan::Watch::new(&root.path).ok(),
				last_poll: Instant::now(),
				pending: true,
			});
			if entry.path != root.path {
				entry.path = root.path.clone();
				entry.watch = sampler_scan::Watch::new(&root.path).ok();
				entry.pending = true;
			}
			entry.pending |= entry.watch.as_ref().is_some_and(|w| w.take_dirty());
			if entry.last_poll.elapsed() >= self.poll_interval {
				entry.pending = true;
				entry.last_poll = Instant::now();
				// Mount reconnects can invalidate an OS watch without changing its path.
				entry.watch = sampler_scan::Watch::new(&root.path).ok();
			}
			if entry.pending {
				match self.engine.scan(root.id) {
					Ok(report) => {
						entry.pending = false;
						reports.push(report);
					}
					Err(Error::Busy) => {} // Retry after analysis/cache jobs release their scan guard.
					Err(error) => {
						entry.pending = false;
						return Err(error);
					}
				}
			}
		}
		Ok(reports)
	}
}
