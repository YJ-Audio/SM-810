use crate::Error;
use rusty_link::{AblLink, SessionState};
use std::sync::{Arc, Mutex};

pub struct LinkControl {
	link: Arc<AblLink>,
	// App-session capture is thread-unsafe; the callback uses a separate preallocated state.
	state: Mutex<SessionState>,
}
#[derive(Debug, Clone, Copy)]
pub struct LinkStatus {
	pub enabled: bool,
	pub tempo: f64,
	pub peers: u64,
}
impl LinkControl {
	pub fn new(tempo: f64) -> Arc<Self> {
		Arc::new(Self {
			link: Arc::new(AblLink::new(tempo)),
			state: Mutex::new(SessionState::new()),
		})
	}
	pub fn configure(&self, enabled: bool, tempo: Option<f64>) -> Result<(), Error> {
		if let Some(tempo) = tempo {
			if !tempo.is_finite() || !(20.0..=999.0).contains(&tempo) {
				return Err(Error::InvalidBuffer);
			}
			let mut state = self
				.state
				.lock()
				.map_err(|_| Error::Device("Link control lock poisoned".into()))?;
			self.link.capture_app_session_state(&mut state);
			state.set_tempo(tempo, self.link.clock_micros());
			self.link.commit_app_session_state(&state);
		}
		self.link.enable(enabled);
		Ok(())
	}
	pub fn status(&self) -> LinkStatus {
		let mut state = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
		self.link.capture_app_session_state(&mut state);
		LinkStatus {
			enabled: self.link.is_enabled(),
			tempo: state.tempo(),
			peers: self.link.num_peers(),
		}
	}
}
pub(crate) struct LinkClock {
	link: Arc<AblLink>,
	state: SessionState,
}
impl LinkClock {
	pub fn new(control: &LinkControl) -> Self {
		Self {
			link: control.link.clone(),
			state: SessionState::new(),
		}
	}
	pub fn snapshot(&mut self, delivery_micros: i64) -> Option<i64> {
		if !self.link.is_enabled() {
			return None;
		}
		self.link.capture_audio_session_state(&mut self.state);
		Some(self.link.clock_micros().saturating_add(delivery_micros))
	}
	pub fn next_bar(&self, now: i64) -> f64 {
		next_bar(self.state.beat_at_time(now, 4.0))
	}
	pub fn time_at_beat(&self, beat: f64) -> i64 {
		self.state.time_at_beat(beat, 4.0)
	}
}
fn next_bar(beat: f64) -> f64 {
	(beat / 4.0).ceil() * 4.0
}
pub(crate) fn launch_frame(target_micros: i64, output_micros: i64, rate: u32) -> usize {
	let delta = target_micros.saturating_sub(output_micros).max(0) as u64;
	(delta.saturating_mul(rate as u64).div_ceil(1_000_000)).min(usize::MAX as u64) as usize
}
#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn bar_and_frame_quantization_handle_negative_beats_and_output_latency() {
		assert_eq!(next_bar(-3.8), 0.0);
		assert_eq!(next_bar(4.0), 4.0);
		assert_eq!(next_bar(4.001), 8.0);
		assert_eq!(launch_frame(2_000_000, 1_990_000, 48000), 480);
		assert_eq!(launch_frame(2_000_000, 1_999_999, 48000), 1);
		assert_eq!(launch_frame(2_000_000, 2_010_000, 48000), 0);
		let control = LinkControl::new(120.0);
		assert!((control.status().tempo - 120.0).abs() < 1e-9);
		assert!(control.configure(false, Some(f64::NAN)).is_err());
		control.configure(false, Some(124.0)).unwrap();
		assert!((control.status().tempo - 124.0).abs() < 1e-9);
	}
}

#[cfg(test)]
mod network_tests {
	use super::*;
	#[test]
	#[ignore = "requires local multicast networking; run explicitly"]
	fn two_link_peers_discover_each_other_and_share_tempo() {
		let a = LinkControl::new(120.0);
		let b = LinkControl::new(120.0);
		a.configure(true, None).unwrap();
		b.configure(true, None).unwrap();
		let start = std::time::Instant::now();
		while (a.status().peers == 0 || b.status().peers == 0) && start.elapsed().as_secs() < 5 {
			std::thread::sleep(std::time::Duration::from_millis(25));
		}
		assert!(
			a.status().peers > 0 && b.status().peers > 0,
			"Multicast peers were not discovered"
		);
		b.configure(true, Some(138.0)).unwrap();
		let start = std::time::Instant::now();
		while (a.status().tempo - 138.0).abs() > 0.001 && start.elapsed().as_secs() < 5 {
			std::thread::sleep(std::time::Duration::from_millis(25));
		}
		assert!((a.status().tempo - 138.0).abs() < 0.001);
		a.configure(false, None).unwrap();
		b.configure(false, None).unwrap();
	}
}
