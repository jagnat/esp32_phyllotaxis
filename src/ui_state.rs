extern crate alloc;

use alloc::vec::Vec;

pub const EXIT_HOLD_US: u64 = 5_000_000;
pub const MENU_TIMEOUT_US: u64 = 15_000_000;
pub const ATTRACT_INTERVAL_US: u64 = 60 * 1_000_000;
pub const AUTO_RETURN_US: u64 = 5 * 60 * 1_000_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiMode {
    Running,
    Menu,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct UiInput {
    pub detents: i32,
    pub pressed: bool,
    pub released: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UiOutput {
    pub mode: UiMode,
    pub render_slot: usize,
    pub running_slot: usize,
    pub menu_cursor: Option<usize>,
    /// Unwrapped carousel position. Unlike `menu_cursor`, this advances on
    /// every detent even when the sketch list wraps.
    pub menu_position: Option<i64>,
    pub sketch_input: UiInput,
    pub visual_changed: bool,
}

pub struct UiState {
    menu_slots: Vec<usize>,
    persistent_slots: Vec<usize>,
    attract_slots: Vec<usize>,
    slot_count: usize,
    default_slot: usize,
    mode: UiMode,
    running_slot: usize,
    menu_position: i64,
    menu_entered_us: u64,
    last_menu_input_us: u64,
    last_activity_us: u64,
    last_attract_us: u64,
    press_started_us: Option<u64>,
    suppress_release: bool,
}

impl UiState {
    pub fn new(menu_slots: &[usize], slot_count: usize, default_slot: usize) -> Self {
        assert!(!menu_slots.is_empty());
        assert!(default_slot < slot_count);
        assert!(menu_slots.iter().all(|slot| *slot < slot_count));
        assert!(menu_slots.contains(&default_slot));
        Self {
            menu_slots: menu_slots.to_vec(),
            persistent_slots: alloc::vec![default_slot],
            attract_slots: alloc::vec![default_slot],
            slot_count,
            default_slot,
            mode: UiMode::Running,
            running_slot: default_slot,
            menu_position: menu_slots
                .iter()
                .position(|slot| *slot == default_slot)
                .unwrap_or(0) as i64,
            menu_entered_us: 0,
            last_menu_input_us: 0,
            last_activity_us: 0,
            last_attract_us: 0,
            press_started_us: None,
            suppress_release: false,
        }
    }

    pub fn set_menu_slots(&mut self, menu_slots: &[usize]) {
        assert!(!menu_slots.is_empty());
        assert!(menu_slots.iter().all(|slot| *slot < self.slot_count));
        assert!(menu_slots.contains(&self.default_slot));
        let visible_slot = self.render_slot();
        self.menu_slots.clear();
        self.menu_slots.extend_from_slice(menu_slots);
        let installed_slots = &self.menu_slots;
        self.persistent_slots
            .retain(|slot| installed_slots.contains(slot));
        if !self.persistent_slots.contains(&self.default_slot) {
            self.persistent_slots.insert(0, self.default_slot);
        }
        self.attract_slots
            .retain(|slot| installed_slots.contains(slot));
        if !self.attract_slots.contains(&self.default_slot) {
            self.attract_slots.insert(0, self.default_slot);
        }
        if !self.menu_slots.contains(&self.running_slot) {
            self.running_slot = self.menu_slots[0];
            self.mode = UiMode::Running;
        }
        let anchor = self
            .menu_slots
            .iter()
            .position(|slot| *slot == visible_slot)
            .or_else(|| {
                self.menu_slots
                    .iter()
                    .position(|slot| *slot == self.running_slot)
            })
            .unwrap_or(0);
        self.align_position_to(anchor);
    }

    /// Set the unattended carousel. It must contain main and may only contain
    /// currently installed menu entries.
    pub fn set_attract_slots(&mut self, attract_slots: &[usize]) {
        assert!(attract_slots.contains(&self.default_slot));
        assert!(
            attract_slots
                .iter()
                .all(|slot| self.menu_slots.contains(slot))
        );
        self.attract_slots.clear();
        self.attract_slots.extend_from_slice(attract_slots);
    }

    /// Set menu entries that should not automatically return to the default
    /// after inactivity. Unlike attract entries, these are never auto-cycled.
    pub fn set_persistent_slots(&mut self, persistent_slots: &[usize]) {
        assert!(persistent_slots.contains(&self.default_slot));
        assert!(
            persistent_slots
                .iter()
                .all(|slot| self.menu_slots.contains(slot))
        );
        self.persistent_slots.clear();
        self.persistent_slots.extend_from_slice(persistent_slots);
    }

    pub fn force_select(&mut self, slot: usize) {
        if self.menu_slots.contains(&slot) {
            self.running_slot = slot;
            self.mode = UiMode::Running;
            self.suppress_release = false;
        }
    }

    fn enter_menu(&mut self, now_us: u64, initial_delta: i32) {
        self.mode = UiMode::Menu;
        let running_cursor = self
            .menu_slots
            .iter()
            .position(|slot| *slot == self.running_slot)
            .unwrap_or(0);
        self.align_position_to(running_cursor);
        self.step_menu(initial_delta);
        self.menu_entered_us = now_us;
        self.last_menu_input_us = now_us;
    }

    fn step_menu(&mut self, delta: i32) {
        self.menu_position = self.menu_position.saturating_add(delta as i64);
    }

    fn menu_cursor(&self) -> usize {
        self.menu_position.rem_euclid(self.menu_slots.len() as i64) as usize
    }

    /// Move to the nearest occurrence of a sketch without throwing away the
    /// carousel's accumulated rotation.
    fn align_position_to(&mut self, cursor: usize) {
        let len = self.menu_slots.len() as i64;
        let current = self.menu_position.rem_euclid(len);
        let mut delta = cursor as i64 - current;
        if delta > len / 2 {
            delta -= len;
        } else if delta < -(len / 2) {
            delta += len;
        }
        self.menu_position = self.menu_position.saturating_add(delta);
    }

    pub fn update(
        &mut self,
        now_us: u64,
        input: UiInput,
        sketch_claims_input: bool,
        remote_select: Option<usize>,
    ) -> UiOutput {
        let old_mode = self.mode;
        let old_render_slot = self.render_slot();
        let old_position = self.menu_position;
        let input_started = input.pressed && self.press_started_us.is_none();
        let input_activity = input.detents != 0 || input.released || input_started;

        if input_activity {
            self.last_activity_us = now_us;
            self.last_attract_us = now_us;
        }

        if input_started {
            self.press_started_us = Some(now_us);
        }

        let mut sketch_input = UiInput::default();

        if let Some(slot) = remote_select.filter(|slot| self.menu_slots.contains(slot)) {
            self.running_slot = slot;
            self.mode = UiMode::Running;
            self.suppress_release = input.pressed || input.released;
            self.last_activity_us = now_us;
            self.last_attract_us = now_us;
        } else {
            match self.mode {
                UiMode::Running if sketch_claims_input => {
                    let held_long_enough = input.pressed
                        && self
                            .press_started_us
                            .is_some_and(|started| now_us.saturating_sub(started) >= EXIT_HOLD_US);
                    if held_long_enough {
                        self.enter_menu(now_us, 0);
                        self.suppress_release = true;
                    } else {
                        sketch_input = input;
                    }
                }
                UiMode::Running => {
                    if self.menu_slots.len() == 1 {
                        // The permanent main vibe does not need a one-item menu.
                    } else if input.detents != 0 {
                        self.enter_menu(now_us, input.detents);
                    } else if input.released {
                        self.enter_menu(now_us, 0);
                    }
                }
                UiMode::Menu => {
                    if input.detents != 0 {
                        self.step_menu(input.detents);
                        self.last_menu_input_us = now_us;
                    }
                    if input.pressed {
                        self.last_menu_input_us = now_us;
                    }
                    if input.released {
                        self.last_menu_input_us = now_us;
                        if self.suppress_release {
                            self.suppress_release = false;
                        } else {
                            self.running_slot = self.menu_slots[self.menu_cursor()];
                            self.mode = UiMode::Running;
                        }
                    } else if !input.pressed
                        && now_us.saturating_sub(self.last_menu_input_us) >= MENU_TIMEOUT_US
                    {
                        self.mode = UiMode::Running;
                    }
                }
            }
        }

        if self.mode == UiMode::Running
            && self.running_slot != self.default_slot
            && !self.persistent_slots.contains(&self.running_slot)
            && !self.attract_slots.contains(&self.running_slot)
            && now_us.saturating_sub(self.last_activity_us) >= AUTO_RETURN_US
        {
            self.running_slot = self.default_slot;
            self.suppress_release = false;
            self.last_attract_us = now_us;
        }

        if self.mode == UiMode::Running
            && self.attract_slots.len() > 1
            && self.attract_slots.contains(&self.running_slot)
            && now_us.saturating_sub(self.last_attract_us) >= ATTRACT_INTERVAL_US
        {
            let cursor = self
                .attract_slots
                .iter()
                .position(|slot| *slot == self.running_slot)
                .unwrap_or(0);
            self.running_slot = self.attract_slots[(cursor + 1) % self.attract_slots.len()];
            self.last_attract_us = now_us;
        }

        if !input.pressed {
            self.press_started_us = None;
        }

        let render_slot = self.render_slot();
        UiOutput {
            mode: self.mode,
            render_slot,
            running_slot: self.running_slot,
            menu_cursor: (self.mode == UiMode::Menu).then(|| self.menu_cursor()),
            menu_position: (self.mode == UiMode::Menu).then_some(self.menu_position),
            sketch_input,
            visual_changed: old_mode != self.mode
                || old_render_slot != render_slot
                || (self.mode == UiMode::Menu && old_position != self.menu_position),
        }
    }

    fn render_slot(&self) -> usize {
        match self.mode {
            UiMode::Running => self.running_slot,
            UiMode::Menu => self.menu_slots[self.menu_cursor()],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static MENU: &[usize] = &[0, 3, 6];

    fn state() -> UiState {
        UiState::new(MENU, 7, 0)
    }

    #[test]
    fn turning_opens_menu_and_previews_next_public_sketch() {
        let out = state().update(
            100,
            UiInput {
                detents: 1,
                ..UiInput::default()
            },
            false,
            None,
        );
        assert_eq!(out.mode, UiMode::Menu);
        assert_eq!(out.menu_cursor, Some(1));
        assert_eq!(out.render_slot, 3);
        assert_eq!(out.sketch_input.detents, 0);
    }

    #[test]
    fn menu_click_confirms_preview() {
        let mut ui = state();
        ui.update(
            100,
            UiInput {
                detents: -1,
                ..UiInput::default()
            },
            false,
            None,
        );
        let out = ui.update(
            200,
            UiInput {
                released: true,
                ..UiInput::default()
            },
            false,
            None,
        );
        assert_eq!(out.mode, UiMode::Running);
        assert_eq!(out.running_slot, 6);
        assert!(out.visual_changed);
    }

    #[test]
    fn menu_timeout_cancels_selection() {
        let mut ui = state();
        ui.update(
            100,
            UiInput {
                detents: 1,
                ..UiInput::default()
            },
            false,
            None,
        );
        let out = ui.update(100 + MENU_TIMEOUT_US, UiInput::default(), false, None);
        assert_eq!(out.mode, UiMode::Running);
        assert_eq!(out.running_slot, 0);
    }

    #[test]
    fn claimed_sketch_receives_short_input() {
        let mut ui = UiState::new(MENU, 7, 6);
        let out = ui.update(
            100,
            UiInput {
                detents: 2,
                released: true,
                ..UiInput::default()
            },
            true,
            None,
        );
        assert_eq!(out.mode, UiMode::Running);
        assert_eq!(out.sketch_input.detents, 2);
        assert!(out.sketch_input.released);
    }

    #[test]
    fn long_hold_escapes_claimed_sketch_without_confirming_on_release() {
        let mut ui = UiState::new(MENU, 7, 6);
        ui.update(
            100,
            UiInput {
                pressed: true,
                ..UiInput::default()
            },
            true,
            None,
        );
        let escaped = ui.update(
            100 + EXIT_HOLD_US,
            UiInput {
                pressed: true,
                ..UiInput::default()
            },
            true,
            None,
        );
        assert_eq!(escaped.mode, UiMode::Menu);
        let released = ui.update(
            200 + EXIT_HOLD_US,
            UiInput {
                released: true,
                ..UiInput::default()
            },
            false,
            None,
        );
        assert_eq!(released.mode, UiMode::Menu);
    }

    #[test]
    fn remote_selection_runs_immediately_and_rejects_invalid_slots() {
        let mut ui = state();
        let selected = ui.update(100, UiInput::default(), false, Some(6));
        assert_eq!(selected.mode, UiMode::Running);
        assert_eq!(selected.running_slot, 6);
        let ignored = ui.update(200, UiInput::default(), false, Some(5));
        assert_eq!(ignored.running_slot, 6);
    }

    #[test]
    fn gallery_sketch_returns_to_main_after_five_inactive_minutes() {
        let mut ui = state();
        ui.update(10, UiInput::default(), false, Some(6));
        let before = ui.update(10 + AUTO_RETURN_US - 1, UiInput::default(), false, None);
        assert_eq!(before.running_slot, 6);
        let returned = ui.update(10 + AUTO_RETURN_US, UiInput::default(), false, None);
        assert_eq!(returned.mode, UiMode::Running);
        assert_eq!(returned.running_slot, 0);
        assert_eq!(returned.render_slot, 0);
    }

    #[test]
    fn physical_input_postpones_automatic_return() {
        let mut ui = state();
        ui.update(10, UiInput::default(), false, Some(6));
        ui.update(
            AUTO_RETURN_US - 10,
            UiInput {
                detents: 1,
                ..UiInput::default()
            },
            true,
            None,
        );
        let active = ui.update(AUTO_RETURN_US + 20, UiInput::default(), true, None);
        assert_eq!(active.running_slot, 6);
    }

    #[test]
    fn idle_attract_mode_cycles_main_and_passive_sketches() {
        let mut ui = state();
        ui.set_attract_slots(&[0, 3]);
        let before = ui.update(ATTRACT_INTERVAL_US - 1, UiInput::default(), false, None);
        assert_eq!(before.running_slot, 0);
        let passive = ui.update(ATTRACT_INTERVAL_US, UiInput::default(), false, None);
        assert_eq!(passive.running_slot, 3);
        let main = ui.update(2 * ATTRACT_INTERVAL_US, UiInput::default(), false, None);
        assert_eq!(main.running_slot, 0);
    }

    #[test]
    fn input_sketches_are_not_in_the_attract_cycle() {
        let mut ui = state();
        ui.set_attract_slots(&[0, 3]);
        ui.update(1, UiInput::default(), true, Some(6));
        let active = ui.update(ATTRACT_INTERVAL_US * 2, UiInput::default(), true, None);
        assert_eq!(active.running_slot, 6);
    }

    #[test]
    fn physical_input_postpones_the_next_attract_change() {
        let mut ui = state();
        ui.set_attract_slots(&[0, 3]);
        ui.update(
            ATTRACT_INTERVAL_US - 10,
            UiInput {
                released: true,
                ..UiInput::default()
            },
            false,
            None,
        );
        let still_main = ui.update(ATTRACT_INTERVAL_US + 10, UiInput::default(), false, None);
        assert_eq!(still_main.running_slot, 0);
    }

    #[test]
    fn deleting_the_running_gallery_item_falls_back_to_main() {
        let mut ui = UiState::new(&[0, 4, 8], 101, 0);
        ui.update(100, UiInput::default(), false, Some(8));
        ui.set_menu_slots(&[0, 4]);
        let out = ui.update(200, UiInput::default(), false, None);
        assert_eq!(out.mode, UiMode::Running);
        assert_eq!(out.running_slot, 0);
        assert_eq!(out.render_slot, 0);
    }

    #[test]
    fn persistent_system_mode_does_not_auto_return_or_attract_cycle() {
        let mut ui = state();
        ui.set_persistent_slots(&[0, 3]);
        ui.update(10, UiInput::default(), false, Some(3));
        let out = ui.update(AUTO_RETURN_US * 2, UiInput::default(), false, None);
        assert_eq!(out.running_slot, 3);
        assert_eq!(out.render_slot, 3);
    }

    #[test]
    fn adding_a_gallery_item_preserves_the_running_item() {
        let mut ui = UiState::new(&[0, 4], 101, 0);
        ui.update(100, UiInput::default(), false, Some(4));
        ui.set_menu_slots(&[0, 2, 4]);
        let out = ui.update(200, UiInput::default(), false, None);
        assert_eq!(out.running_slot, 4);
        assert_eq!(out.render_slot, 4);
    }

    #[test]
    fn main_vibe_alone_does_not_open_a_one_item_menu() {
        let mut ui = UiState::new(&[0], 101, 0);
        let out = ui.update(
            100,
            UiInput {
                detents: 1,
                released: true,
                ..UiInput::default()
            },
            false,
            None,
        );
        assert_eq!(out.mode, UiMode::Running);
        assert_eq!(out.render_slot, 0);
    }

    #[test]
    fn cursor_scrolls_lists_larger_than_the_five_position_viewport() {
        static MANY: &[usize] = &[0, 1, 2, 3, 4, 5, 6];
        let mut ui = UiState::new(MANY, 7, 0);
        let forward = ui.update(
            100,
            UiInput {
                detents: 5,
                ..UiInput::default()
            },
            false,
            None,
        );
        assert_eq!(forward.menu_cursor, Some(5));
        assert_eq!(forward.menu_position, Some(5));
        assert_eq!(forward.render_slot, 5);
        let wrapped = ui.update(
            200,
            UiInput {
                detents: 3,
                ..UiInput::default()
            },
            false,
            None,
        );
        assert_eq!(wrapped.menu_cursor, Some(1));
        assert_eq!(wrapped.menu_position, Some(8));
        assert_eq!(wrapped.render_slot, 1);
    }

    #[test]
    fn carousel_advances_across_the_sketch_list_seam() {
        static SIX: &[usize] = &[0, 1, 2, 3, 4, 5];
        let mut ui = UiState::new(SIX, 6, 0);
        for expected_position in 1..=7 {
            let out = ui.update(
                expected_position as u64 * 100,
                UiInput {
                    detents: 1,
                    ..UiInput::default()
                },
                false,
                None,
            );
            assert_eq!(out.menu_position, Some(expected_position));
            assert_eq!(
                out.menu_cursor,
                Some(expected_position as usize % SIX.len())
            );
        }
    }
}
