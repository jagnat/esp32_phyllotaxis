use libm::{cosf, sqrtf};
use microfft::real::rfft_512;

pub const NUM_BANDS: usize = 6;
const FFT_SIZE: usize = 512;
const BINS: usize = FFT_SIZE / 2;

// bin edges (62.5 Hz/bin at 32kHz): ~188, 375, 1k, 3k, 8k, 16k
const BAND_EDGES: [usize; NUM_BANDS + 1] = [1, 3, 6, 16, 48, 128, 256];
const MIN_BAND_MAX: [f32; NUM_BANDS] = [0.15, 0.12, 0.08, 0.05, 0.03, 0.02];
const NOISE_FLOOR_RISE: f32 = 0.1 * 0.016;
const NORM_GLOBAL_FACTOR: f32 = 0.4;

#[derive(Clone, Copy, Default)]
pub struct AudioFeatures {
	pub bands: [f32; NUM_BANDS],
	pub transients: [f32; NUM_BANDS],
	pub sustain: [f32; NUM_BANDS],
	pub super_sustain: [f32; NUM_BANDS],
}

#[derive(Clone, Copy)]
enum EnvState { Rising, Holding, Falling }

#[derive(Clone, Copy)]
struct Envelope { hold: u8, state: EnvState }
impl Default for Envelope {
	fn default() -> Self { Self { hold: 0, state: EnvState::Rising } }
}

pub struct AudioProcessor {
	hamming: [f32; FFT_SIZE],
	noise_floor: [f32; NUM_BANDS],
	band_maxes: [f32; NUM_BANDS],
	recent_global_max: f32,
	// persistent envelope outputs + state, per band
	transients: [f32; NUM_BANDS],
	sustain: [f32; NUM_BANDS],
	super_sustain: [f32; NUM_BANDS],
	env_transient: [Envelope; NUM_BANDS],
	env_sustain: [Envelope; NUM_BANDS],
	env_super: [Envelope; NUM_BANDS],
}

impl AudioProcessor {
	pub fn new() -> Self {
		let mut hamming = [0.0; FFT_SIZE];
		for (i, w) in hamming.iter_mut().enumerate() {
			*w = 0.54 - 0.46 * cosf(2.0 * core::f32::consts::PI * i as f32 / (FFT_SIZE as f32 - 1.0));
		}
		Self {
			hamming,
			noise_floor: [0.0; NUM_BANDS],
			band_maxes: [0.001; NUM_BANDS],
			recent_global_max: 0.001,
			transients: [0.0; NUM_BANDS],
			sustain: [0.0; NUM_BANDS],
			super_sustain: [0.0; NUM_BANDS],
			env_transient: [Envelope::default(); NUM_BANDS],
			env_sustain: [Envelope::default(); NUM_BANDS],
			env_super: [Envelope::default(); NUM_BANDS],
		}
	}

	pub fn process(&mut self, sample: impl Fn(usize) -> f32) -> AudioFeatures {
		let mut buf = [0.0f32; FFT_SIZE];
		for (i, b) in buf.iter_mut().enumerate() {
			*b = sample(i) * self.hamming[i];
		}
		let spectrum = rfft_512(&mut buf);

		let mut mag = [0.0f32; BINS];
		for (m, c) in mag.iter_mut().zip(spectrum.iter()) {
			*m = sqrtf(c.re * c.re + c.im * c.im);
		}

		// average bins into log-ish bands
		let mut band = [0.0f32; NUM_BANDS];
		for b in 0..NUM_BANDS {
			let count = (BAND_EDGES[b + 1] - BAND_EDGES[b]) as f32;
			let mut sum = 0.0;
			for bin in BAND_EDGES[b]..BAND_EDGES[b + 1] {
				sum += mag[bin];
			}
			band[b] = (sum / count).min(1.0);
		}

		// noise floor tracking and activity gate
		for b in 0..NUM_BANDS {
			if band[b] < self.noise_floor[b] {
				self.noise_floor[b] = band[b];
			} else if band[b] > self.noise_floor[b] {
				self.noise_floor[b] = (self.noise_floor[b] + NOISE_FLOOR_RISE).min(0.1);
			}
			let lo = self.noise_floor[b];
			let hi = lo * 2.0;
			if band[b] <= lo {
				band[b] = 0.0;
			} else if band[b] < hi {
				band[b] *= smoothstep(lo, hi, band[b]);
			}
		}

		// global max decay
		let mut cur_max = 0.0f32;
		for b in 0..NUM_BANDS {
			if band[b] > cur_max { cur_max = band[b]; }
		}
		if cur_max > self.recent_global_max {
			self.recent_global_max = cur_max;
		} else {
			self.recent_global_max *= 0.995;
		}
		self.recent_global_max = self.recent_global_max.max(0.01);

		// per-band AGC and envelopes
		let mut feats = AudioFeatures::default();
		for b in 0..NUM_BANDS {
			let energy = band[b];
			if energy > self.band_maxes[b] {
				self.band_maxes[b] = energy;
			} else {
				self.band_maxes[b] *= 0.9999;
			}
			self.band_maxes[b] = self.band_maxes[b].max(MIN_BAND_MAX[b]);

			let global_norm = energy / self.recent_global_max;
			let band_norm = energy / self.band_maxes[b];
			let normalized = (NORM_GLOBAL_FACTOR * global_norm
				+ (1.0 - NORM_GLOBAL_FACTOR) * band_norm).min(1.0);

			feats.bands[b] = normalized;
			handle_band(&mut self.transients[b], &mut self.env_transient[b], normalized, 1, 0.8);
			handle_band(&mut self.sustain[b], &mut self.env_sustain[b], normalized, 10, 0.94);
			handle_band(&mut self.super_sustain[b], &mut self.env_super[b], normalized, 16, 0.97);
		}
		feats.transients = self.transients;
		feats.sustain = self.sustain;
		feats.super_sustain = self.super_sustain;
		feats
	}
}

// attack-hold-decay envelope follower
fn handle_band(out: &mut f32, env: &mut Envelope, normalized: f32, max_hold: u8, decay: f32) {
	if normalized > *out {
		env.state = EnvState::Rising;
	}
	match env.state {
		EnvState::Rising => { *out = normalized; env.state = EnvState::Holding; env.hold = 0; }
		EnvState::Holding => { env.hold += 1; if env.hold > max_hold { env.state = EnvState::Falling; } }
		EnvState::Falling => { *out = *out * decay + normalized * (1.0 - decay); }
	}
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
	let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
	t * t * (3.0 - 2.0 * t)
}
