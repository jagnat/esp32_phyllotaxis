use core::f32::consts::{PI, TAU};

use crate::api::{self, Frame, Input, Rgb, LED_COUNT};

const BASE_SPEED: f32 = 1.35;
const SPEED_PER_HIT: f32 = 0.38;
const HIT_WINDOW: f32 = 0.20;
const TARGET_WIDTH: f32 = 0.28;
const MARKER_WIDTH: f32 = 0.23;
const RESULT_SECONDS: f32 = 3.8;

#[derive(Clone, Copy, PartialEq)]
enum Screen {
    Playing,
    Won,
    Lost,
}

pub struct Game {
    initialized: bool,
    marker: f32,
    target: f32,
    speed: f32,
    hits: u8,
    misses: u8,
    button_attempts: u8,
    feedback_seconds: f32,
    feedback_good: bool,
    result_seconds: f32,
    return_after_result: bool,
    screen: Screen,
}

impl Game {
    pub const fn new() -> Self {
        Self {
            initialized: false,
            marker: 0.0,
            target: PI,
            speed: BASE_SPEED,
            hits: 0,
            misses: 0,
            button_attempts: 0,
            feedback_seconds: 0.0,
            feedback_good: false,
            result_seconds: 0.0,
            return_after_result: false,
            screen: Screen::Playing,
        }
    }

    fn reset(&mut self) {
        self.marker = 0.0;
        self.target = next_target(self.marker);
        self.speed = BASE_SPEED;
        self.hits = 0;
        self.misses = 0;
        self.button_attempts = 0;
        self.feedback_seconds = 0.0;
        self.result_seconds = 0.0;
        self.return_after_result = false;
        self.screen = Screen::Playing;
        self.initialized = true;
    }

    fn attempt(&mut self, good: bool) {
        self.feedback_seconds = 0.24;
        self.feedback_good = good;
        if good {
            self.hits += 1;
            if self.hits >= 5 {
                self.screen = Screen::Won;
                self.result_seconds = 0.0;
                return;
            }
            self.speed = BASE_SPEED + SPEED_PER_HIT * self.hits as f32;
        } else {
            self.misses += 1;
            if self.misses >= 3 {
                self.screen = Screen::Lost;
                self.result_seconds = 0.0;
                self.return_after_result = self.button_attempts == 0;
                return;
            }
        }
        self.target = next_target(self.marker);
    }

    pub fn render(&mut self, input: &Input<'_>, frame: &mut Frame<'_>) {
        if !self.initialized {
            self.reset();
        }
        let dt = input.dt_seconds().clamp(0.0, 0.05);

        match self.screen {
            Screen::Won => {
                self.result_seconds += dt;
                render_win(input, frame);
                if self.result_seconds >= RESULT_SECONDS {
                    self.reset();
                }
                return;
            }
            Screen::Lost => {
                self.result_seconds += dt;
                render_loss(input, frame);
                if self.result_seconds >= RESULT_SECONDS {
                    if self.return_after_result {
                        api::return_to_main();
                    }
                    self.reset();
                }
                return;
            }
            Screen::Playing => {}
        }

        let before = clockwise_distance(self.marker, self.target);
        self.marker = wrap(self.marker + self.speed * dt);
        let after = clockwise_distance(self.marker, self.target);
        let passed_target = after > before;
        let on_target = angular_distance(self.marker, self.target) <= HIT_WINDOW;

        if input.button_released() {
            self.button_attempts = self.button_attempts.saturating_add(1);
            self.attempt(on_target);
        } else if passed_target {
            self.attempt(false);
        }

        self.feedback_seconds = (self.feedback_seconds - dt).max(0.0);
        render_playfield(
            frame,
            self.marker,
            self.target,
            self.hits,
            self.misses,
            self.feedback_seconds,
            self.feedback_good,
        );
    }
}

pub fn preview(input: &Input<'_>, frame: &mut Frame<'_>) {
    let marker = input.phase(4_000_000) * TAU;
    render_playfield(frame, marker, PI * 0.72, 0, 0, 0.0, false);
}

fn next_target(marker: f32) -> f32 {
    // Always place it far enough ahead that the player sees it arrive.
    wrap(marker + 1.25 + api::random() * 3.65)
}

fn wrap(angle: f32) -> f32 {
    let mut wrapped = angle - libm::floorf(angle / TAU) * TAU;
    if wrapped < 0.0 {
        wrapped += TAU;
    }
    wrapped
}

fn clockwise_distance(from: f32, to: f32) -> f32 {
    wrap(to - from)
}

fn angular_distance(a: f32, b: f32) -> f32 {
    let delta = wrap(a - b);
    delta.min(TAU - delta)
}

fn add(a: Rgb, b: Rgb) -> Rgb {
    Rgb::new(
        a.r.saturating_add(b.r),
        a.g.saturating_add(b.g),
        a.b.saturating_add(b.b),
    )
}

fn render_playfield(
    frame: &mut Frame<'_>,
    marker: f32,
    target: f32,
    hits: u8,
    misses: u8,
    feedback_seconds: f32,
    feedback_good: bool,
) {
    frame.fill(Rgb::BLACK);
    for index in 0..LED_COUNT {
        let point = api::position(index);
        let radius = libm::sqrtf(point.x * point.x + point.y * point.y);
        let angle = wrap(libm::atan2f(point.y, point.x));
        let mut color = Rgb::new(2, 3, 8);

        if radius > 0.63 {
            color = add(color, Rgb::new(7, 7, 15));
            let target_glow =
                (1.0 - angular_distance(angle, target) / TARGET_WIDTH).clamp(0.0, 1.0);
            color = add(color, Rgb::new(20, 255, 55).scale(target_glow));
            let marker_glow =
                (1.0 - angular_distance(angle, marker) / MARKER_WIDTH).clamp(0.0, 1.0);
            color = add(color, Rgb::new(255, 210, 120).scale(marker_glow));
        }

        // Five blue progress sparks and three red life sparks live inside.
        if radius < 0.60 {
            for progress in 0..hits {
                let spark = -2.2 + progress as f32 * 0.34;
                let glow = (1.0 - angular_distance(angle, wrap(spark)) / 0.13).clamp(0.0, 1.0)
                    * (1.0 - (radius - 0.34).abs() / 0.18).clamp(0.0, 1.0);
                color = add(color, Rgb::new(30, 100, 255).scale(glow));
            }
            for miss in 0..misses {
                let spark = -0.1 + miss as f32 * 0.38;
                let glow = (1.0 - angular_distance(angle, wrap(spark)) / 0.14).clamp(0.0, 1.0)
                    * (1.0 - (radius - 0.34).abs() / 0.18).clamp(0.0, 1.0);
                color = add(color, Rgb::new(255, 15, 5).scale(glow));
            }
        }

        if feedback_seconds > 0.0 {
            let pulse = feedback_seconds / 0.24;
            let feedback = if feedback_good {
                Rgb::new(20, 180, 70)
            } else {
                Rgb::new(220, 5, 0)
            };
            color = add(color, feedback.scale(pulse * 0.45));
        }
        frame.set(index, color);
    }
}

fn render_win(input: &Input<'_>, frame: &mut Frame<'_>) {
    let phase = input.phase(2_800_000);
    for index in 0..LED_COUNT {
        let point = api::position(index);
        let radius = libm::sqrtf(point.x * point.x + point.y * point.y);
        let angle = libm::atan2f(point.y, point.x) / TAU;
        let wave = 0.58 + 0.34 * libm::sinf(TAU * (phase * 2.0 - radius * 1.6));
        frame.set(index, api::hsv(phase + angle + radius * 0.45, 0.82, wave));
    }
}

fn render_loss(input: &Input<'_>, frame: &mut Frame<'_>) {
    let pulse = 0.20 + 0.48 * (0.5 + 0.5 * libm::sinf(input.phase(900_000) * TAU));
    for index in 0..LED_COUNT {
        let point = api::position(index);
        let radius = libm::sqrtf(point.x * point.x + point.y * point.y);
        frame.set(
            index,
            Rgb::new(255, 2, 0).scale(pulse * (1.0 - radius * 0.35)),
        );
    }
}
