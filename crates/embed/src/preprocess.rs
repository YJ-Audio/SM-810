//! CLAP unfused, pinned repeatpad + Slaney log-mel preprocessing.
use rustfft::{Fft, FftPlanner, num_complex::Complex};
use std::sync::Arc;

pub const RATE: u32 = 48000;
pub const SAMPLES: usize = 480000;
pub const FRAMES: usize = 1001;
pub const MELS: usize = 64;
const FFT: usize = 1024;
const HOP: usize = 480;
fn mel(hz: f64) -> f64 {
	if hz < 1000.0 {
		hz / (200.0 / 3.0)
	} else {
		15.0 + (hz / 1000.0).ln() / (6.4f64.ln() / 27.0)
	}
}
fn hz(mel: f64) -> f64 {
	if mel < 15.0 {
		mel * (200.0 / 3.0)
	} else {
		1000.0 * ((mel - 15.0) * (6.4f64.ln() / 27.0)).exp()
	}
}
pub struct Preprocessor {
	fft: Arc<dyn Fft<f64>>,
	window: Vec<f64>,
	filters: Vec<Vec<(usize, f64)>>,
}
impl Default for Preprocessor {
	fn default() -> Self {
		let fft = FftPlanner::new().plan_fft_forward(FFT);
		let window = (0..FFT)
			.map(|i| 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / FFT as f64).cos())
			.collect();
		let points: Vec<_> = (0..MELS + 2)
			.map(|i| hz(mel(50.0) + (mel(14000.0) - mel(50.0)) * i as f64 / (MELS + 1) as f64))
			.collect();
		let filters = (0..MELS)
			.map(|m| {
				(0..FFT / 2 + 1)
					.filter_map(|bin| {
						let f = bin as f64 * RATE as f64 / FFT as f64;
						let weight = ((f - points[m]) / (points[m + 1] - points[m]))
							.min((points[m + 2] - f) / (points[m + 2] - points[m + 1]))
							.max(0.0) * 2.0 / (points[m + 2] - points[m]);
						(weight > 0.0).then_some((bin, weight))
					})
					.collect()
			})
			.collect();
		Self { fft, window, filters }
	}
}
impl Preprocessor {
	pub fn features(&self, mono: &[f32]) -> Result<Vec<f32>, &'static str> {
		if mono.is_empty() || mono.iter().any(|s| !s.is_finite()) {
			return Err("CLAP requires a finite, nonempty waveform");
		}
		let length = mono.len().min(SAMPLES);
		let repeated = SAMPLES / length * length;
		let value = |index: isize| {
			// numpy.pad(mode="reflect") excludes the boundary value itself.
			let index = if index < 0 {
				-index
			} else if index >= SAMPLES as isize {
				2 * SAMPLES as isize - 2 - index
			} else {
				index
			} as usize;
			if index < repeated {
				mono[index % length] as f64
			} else {
				0.0
			}
		};
		let mut output = Vec::with_capacity(FRAMES * MELS);
		let mut buffer = vec![Complex::new(0.0, 0.0); FFT];
		let mut scratch = vec![Complex::new(0.0, 0.0); self.fft.get_inplace_scratch_len()];
		for frame in 0..FRAMES {
			for (i, sample) in buffer.iter_mut().enumerate() {
				*sample = Complex::new(
					value((frame * HOP + i) as isize - (FFT / 2) as isize) * self.window[i],
					0.0,
				);
			}
			self.fft.process_with_scratch(&mut buffer, &mut scratch);
			for filter in &self.filters {
				let power = filter
					.iter()
					.map(|(bin, weight)| buffer[*bin].norm_sqr() * weight)
					.sum::<f64>();
				output.push((10.0 * power.max(1e-10).log10()) as f32);
			}
		}
		Ok(output)
	}
}
