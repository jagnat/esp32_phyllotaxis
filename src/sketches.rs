use crate::util::*;
use crate::audio::*;
use core::f32::consts::TAU;
use libm::{atan2f, fmaxf, fmodf, powf, sinf, sqrtf};

pub struct SketchParams {
	pub dt: f32,
	pub t: f32,
	pub audio: AudioFeatures,
	pub released: bool, // button released this frame (always false until button task)
}

pub trait Sketch {
	fn render(&mut self, leds: &mut [Rgb; NUM_LEDS], p: &SketchParams);
}

// xorshift32 stand-in for C rand()
struct Rng { state: u32 }
impl Rng {
	fn new(seed: u32) -> Self { Self { state: seed } }
	fn next_u32(&mut self) -> u32 {
		let mut x = self.state;
		x ^= x << 13;
		x ^= x >> 17;
		x ^= x << 5;
		self.state = x;
		x
	}
	fn below(&mut self, n: u32) -> u32 { self.next_u32() % n }
}

#[derive(Clone, Copy)]
struct Palette { c1: Rgb, c2: Rgb, c3: Rgb }

const NUM_PALETTES: usize = 4;
const PALETTES: [Palette; NUM_PALETTES] = [
	Palette { c1: Rgb::new(220, 20, 0),  c2: Rgb::new(255, 120, 20), c3: Rgb::new(255, 210, 60) },
	Palette { c1: Rgb::new(255, 0, 75),  c2: Rgb::new(200, 140, 0),  c3: Rgb::new(220, 220, 100) },
	Palette { c1: Rgb::new(255, 90, 0),  c2: Rgb::new(200, 255, 0),  c3: Rgb::new(100, 200, 140) },
	Palette { c1: Rgb::new(205, 0, 90),  c2: Rgb::new(255, 140, 0),  c3: Rgb::new(220, 120, 180) },
];

const NUM_GLINT_COLORS: usize = 9;
const GLINT_COLORS: [Rgb; NUM_GLINT_COLORS] = [
	Rgb::new(40, 200, 90),
	Rgb::new(0, 99, 190),
	Rgb::new(0, 140, 140),
	Rgb::new(140, 140, 0),
	Rgb::new(110, 140, 110),
	Rgb::new(200, 150, 255),
	Rgb::new(150, 230, 255),
	Rgb::new(255, 200, 150),
	Rgb::new(255, 255, 200),
];

fn get_palette(angle: f32, p: &Palette) -> Rgb {
	let mut t = fmodf(angle, TAU);
	if t < 0.0 { t += TAU; }
	t /= TAU;
	let zone = t * 3.0;
	if zone < 1.0 {
		p.c1.lerp(p.c2, zone)
	} else if zone < 2.0 {
		p.c2.lerp(p.c3, zone - 1.0)
	} else {
		p.c3.lerp(p.c1, zone - 2.0)
	}
}

fn next_palette(rng: &mut Rng, current: usize) -> usize {
	let roll = rng.below(100);
	// bias toward the two warm palettes
	if roll < 10 {
		loop { let n = rng.below(2) as usize; if n != current { return n; } }
	} else {
		loop { let n = (rng.below((NUM_PALETTES - 2) as u32) + 2) as usize; if n != current { return n; } }
	}
}

pub struct OrganicVibe {
	pulse_phase: f32,
	rot_dir: f32,
	color_phase: f32,
	circle_direction_timer: f32,
	glint_color_idx: usize,
	glint_seed: f32,
	glint_level: f32,
	glint_trigger_level: f32,
	ambient_phase: f32,
	current_palette: Palette,
	current_palette_idx: usize,
	target_palette_idx: usize,
	palette_transition: f32,
	palette_timer: f32,
	pulse_dir_timer: f32,
	pulse_dir: f32,
	show_spiral: f32,
	show_spiral_accum: f32,
	rng: Rng,
}

impl Default for OrganicVibe {
	fn default() -> Self {
		Self {
			pulse_phase: 0.0,
			rot_dir: 1.0,
			color_phase: 0.0,
			circle_direction_timer: 0.0,
			glint_color_idx: 0,
			glint_seed: 0.0,
			glint_level: 0.0,
			glint_trigger_level: 0.0,
			ambient_phase: 0.0,
			current_palette: PALETTES[0],
			current_palette_idx: 0,
			target_palette_idx: 0,
			palette_transition: 1.0,
			palette_timer: 0.0,
			pulse_dir_timer: 0.0,
			pulse_dir: 1.0,
			show_spiral: 0.0,
			show_spiral_accum: 0.0003,
			rng: Rng::new(0x1337_beef),
		}
	}
}

impl Sketch for OrganicVibe {
	fn render(&mut self, leds: &mut [Rgb; NUM_LEDS], p: &SketchParams) {
		let dt = p.dt;
		let a = &p.audio;

		self.circle_direction_timer += dt;
		if self.circle_direction_timer > 30.0 {
			if self.rng.below(10) > 5 { self.rot_dir *= -1.0; }
			self.circle_direction_timer = 0.0;
		}

		self.pulse_dir_timer += dt;
		if self.pulse_dir_timer > 37.0 {
			self.pulse_dir_timer = 0.0;
			self.pulse_dir = if self.rng.below(10) > 9 { -1.0 } else { 1.0 };
		}

		self.palette_timer += dt;
		self.palette_transition += dt * 0.05;
		if self.palette_timer > 100.0 || p.released {
			if p.released { self.pulse_dir = -1.0; }
			self.palette_timer = 0.0;
			self.target_palette_idx = next_palette(&mut self.rng, self.current_palette_idx);
			self.palette_transition = 0.0;
		}

		// blend toward the target palette
		if self.palette_transition < 1.0 {
			let pt = self.palette_transition;
			let s = pt * pt * (3.0 - 2.0 * pt);
			let cur = &PALETTES[self.current_palette_idx];
			let tgt = &PALETTES[self.target_palette_idx];
			self.current_palette.c1 = cur.c1.lerp(tgt.c1, s);
			self.current_palette.c2 = cur.c2.lerp(tgt.c2, s);
			self.current_palette.c3 = cur.c3.lerp(tgt.c3, s);
		} else {
			self.current_palette_idx = self.target_palette_idx;
			self.current_palette = PALETTES[self.current_palette_idx];
		}

		// spiral undulates regardless of audio
		self.show_spiral += self.show_spiral_accum;
		if self.show_spiral > 0.6 { self.show_spiral_accum = -0.001; }
		if self.show_spiral < 0.0 { self.show_spiral_accum = 0.0003; }

		// audio-reactive speeds
		self.pulse_phase += self.pulse_dir * dt * 0.6 * (0.1 + (a.sustain[0] * 2.0 + a.transients[0] * 2.0));
		self.color_phase += dt * 0.2 * self.rot_dir;
		self.color_phase = fmodf(self.color_phase, TAU);
		if self.color_phase < 0.0 { self.color_phase += TAU; }
		self.ambient_phase += dt * 0.04;

		// trigger glints on high-band transients
		let hi_transient = 2.0 * fmaxf(0.6 * a.transients[4], a.transients[5]);
		if hi_transient > 0.1
			&& (hi_transient > self.glint_trigger_level || self.glint_level < 0.1)
			&& hi_transient > self.glint_level
		{
			self.glint_level = hi_transient.clamp(0.5, 1.0);
			self.glint_trigger_level = hi_transient;
			self.glint_color_idx = self.rng.below(NUM_GLINT_COLORS as u32) as usize;
			self.glint_seed = self.rng.below(1000) as f32 * 0.1337;
		} else {
			self.glint_level *= 0.994;
		}
		if self.glint_level < 0.0001 { self.glint_level = 0.0; }

		for (i, led) in leds.iter_mut().enumerate() {
			let x = LED_POSITIONS[i][0];
			let y = LED_POSITIONS[i][1];
			let r = sqrtf(x * x + y * y);
			let theta = atan2f(y, x);

			let mut final_c = get_palette(theta + self.color_phase, &self.current_palette);

			// breathing
			let mut wave = (sinf(r * 12.0 - self.pulse_phase * 2.0) + 1.0) * 0.5;
			wave = powf(wave, 1.4);
			let pulse_factor = powf(fmaxf(a.sustain[1], 0.4 * a.transients[3]), 0.8);
			let pulse_intensity = wave * (0.2 + pulse_factor);
			final_c = Rgb::new(20, 0, 0).lerp(final_c, pulse_intensity);

			// fib-spiral shell
			let theta_rotating = theta + self.ambient_phase;
			let twist_factor = 4.0 * 2.39996;
			let warped_theta = theta_rotating - r * twist_factor;
			let shell_wave = powf((sinf(7.0 * warped_theta) + 1.0) * 0.5, 3.0);
			let mut shell_color = Rgb::new(180, 0, 110).lerp(Rgb::new(0, 80, 20), pulse_intensity);
			shell_color = Rgb::new(0, 0, 0).lerp(shell_color, fmaxf(self.show_spiral, a.super_sustain[2]));
			shell_color = Rgb::new(0, 0, 0).lerp(shell_color, shell_wave);
			final_c = shell_color.add(final_c);

			// high-frequency glints on a sparse pixel mask
			if self.glint_level > 0.04 {
				let noise = sinf(i as f32 * 1.618 + self.glint_seed);
				if noise > 0.96 {
					let masking = powf(1.0 - pulse_intensity, 3.0);
					let brightness = self.glint_level * masking;
					let col = Rgb::new(0, 0, 0).lerp(GLINT_COLORS[self.glint_color_idx], brightness);
					final_c = final_c.add(col);
				}
			}

			*led = final_c;
		}
	}
}

// strip split into bands, hue by band, value by energy
pub struct AudioSketch;
impl Sketch for AudioSketch {
	fn render(&mut self, leds: &mut [Rgb; NUM_LEDS], p: &SketchParams) {
		let leds_per_band = NUM_LEDS / NUM_BANDS;
		for (i, led) in leds.iter_mut().enumerate() {
			let band = (i / leds_per_band).min(NUM_BANDS - 1);
			let energy = p.audio.bands[band].min(1.0);
			let hue = (band as f32 / NUM_BANDS as f32) * 360.0;
			*led = hsv_to_rgb_rainbow(Hsv { h: hue, s: 1.0, v: energy });
		}
	}
}

pub struct FullWhiteSketch;
impl Sketch for FullWhiteSketch {
	fn render(&mut self, leds: &mut [Rgb; NUM_LEDS], p: &SketchParams) {
		for (i, led) in  leds.iter_mut().enumerate() {
			if i == 1 { *led = Rgb{r: 255, g: 255, b: 255} };
		}
	}
}

pub struct SketchBank {
	organic: OrganicVibe,
	audio_sketch: AudioSketch,
	full_white: FullWhiteSketch,
	active: usize,
}
impl SketchBank {
	pub fn new() -> Self {
		Self { organic: OrganicVibe::default(), audio_sketch: AudioSketch, full_white: FullWhiteSketch, active: 0 }
	}

	pub fn render(&mut self, leds: &mut [Rgb; NUM_LEDS], p: &SketchParams) {
		match self.active {
			1 => self.audio_sketch.render(leds, p),
			2 => self.full_white.render(leds, p),
			_ => self.organic.render(leds, p),
		}
	}

	pub fn next(&mut self) { self.active = (self.active + 1) % 2; }
}
