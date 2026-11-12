// SPDX-FileCopyrightText: 2026 Alby Hernández <hola@achetronic.com>
// SPDX-License-Identifier: Apache-2.0

//! Tells a held shortcut apart from one that is never coming back.
//!
//! The portal reports `Activated` on every key repeat and `Deactivated` when the
//! accelerator comes up. The second one cannot be trusted: mutter matches the
//! accelerator against the modifiers still held, so releasing `Control` or
//! `Shift` before the letter means the gesture is never closed, and the recording
//! runs until the safety cap. The user reads that as the shortcut not releasing.
//!
//! The repeats give it away. They stop the moment the key comes up, so a silence
//! longer than the repeat interval means the key is up, and a press that arrives
//! after such a silence is a new gesture whose predecessor was never closed.

use std::time::{Duration, Instant};

/// Gaps at or below this are the same key repeating, not a new press.
const REPEAT_GAP: Duration = Duration::from_millis(250);

/// Silence accepted after a press before any repeat has been seen.
///
/// Repeats are what makes the silence meaningful, so without one the only honest
/// answer is "I do not know yet". Long enough not to cut a hold short on a
/// keyboard with repeat disabled, short enough to rescue a recording that would
/// otherwise run to the safety cap.
const UNKNOWN_SILENCE: Duration = Duration::from_secs(3);

/// A release this soon after the press is the compositor talking, not the user:
/// no hand presses and lets go of a combination in eighty milliseconds.
const MIN_HOLD: Duration = Duration::from_millis(80);

/// Silence after a repeat is this many intervals, within these bounds.
const SILENCE_FACTOR: u32 = 4;
const MIN_SILENCE: Duration = Duration::from_millis(150);
const MAX_SILENCE: Duration = Duration::from_millis(600);

/// What a press turned out to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Press {
    /// A new gesture.
    Fresh,
    /// The key repeating while it is held: already counted.
    Repeat,
}

/// One shortcut being held.
#[derive(Debug)]
pub struct Hold {
    down: bool,
    seen: Instant,
    /// When the gesture opened, to tell a real release from a spurious one.
    since: Instant,
    repeats: u32,
    gap: Duration,
}

impl Hold {
    /// Starts with nothing held.
    pub fn new() -> Self {
        Self {
            down: false,
            seen: Instant::now(),
            since: Instant::now(),
            repeats: 0,
            gap: REPEAT_GAP,
        }
    }

    /// Whether a gesture is open.
    pub fn is_down(&self) -> bool {
        self.down
    }

    /// The portal reported a press.
    ///
    /// While the gesture is open, a press is the key repeating: opening a second
    /// gesture from it would stop a recording the user is still dictating into.
    /// Closing the first one is the silence detector's job, and it runs before
    /// this can be called with the key really up.
    pub fn press(&mut self, now: Instant) -> Press {
        if self.down {
            // Repeat intervals are short, but the first one comes after the
            // initial delay of key repeat: only the short ones teach the cadence.
            let quiet = now.saturating_duration_since(self.seen);
            if !quiet.is_zero() && quiet <= REPEAT_GAP {
                self.gap = quiet;
            }
            self.repeats += 1;
            self.seen = now;
            return Press::Repeat;
        }

        self.down = true;
        self.repeats = 0;
        self.seen = now;
        self.since = now;
        Press::Fresh
    }

    /// The portal reported a release.
    ///
    /// False when nothing was open, or when it came so soon after the press that
    /// it cannot be the user's. The portal emits those pairs on its own, and
    /// taking them at face value stopped every dictation a millisecond after it
    /// began. The gesture then ends by silence, which is the other half of this
    /// module's job.
    pub fn release(&mut self, now: Instant) -> bool {
        if !self.down || now.saturating_duration_since(self.since) < MIN_HOLD {
            return false;
        }

        self.down = false;
        self.repeats = 0;
        true
    }

    /// Whether the silence means the key came up, closing the gesture.
    pub fn expired(&mut self, now: Instant) -> bool {
        if !self.down || now.saturating_duration_since(self.seen) < self.silence() {
            return false;
        }

        self.down = false;
        self.repeats = 0;
        true
    }

    /// How much silence counts as a release right now.
    fn silence(&self) -> Duration {
        if self.repeats == 0 {
            return UNKNOWN_SILENCE;
        }

        (self.gap * SILENCE_FACTOR).clamp(MIN_SILENCE, MAX_SILENCE)
    }
}

impl Default for Hold {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(millis: u64) -> Instant {
        Instant::now() + Duration::from_millis(millis)
    }

    #[test]
    fn a_press_opens_the_gesture_and_its_repeats_do_not() {
        let mut hold = Hold::new();
        assert_eq!(hold.press(at(0)), Press::Fresh);
        assert_eq!(hold.press(at(30)), Press::Repeat);
        assert_eq!(hold.press(at(60)), Press::Repeat);
        assert!(hold.is_down());
    }

    #[test]
    fn a_release_closes_the_gesture_once() {
        let mut hold = Hold::new();
        hold.press(at(0));
        assert!(hold.release(at(500)));
        assert!(!hold.release(at(900)));
    }

    #[test]
    fn a_release_right_after_the_press_is_the_compositor_talking() {
        let mut hold = Hold::new();
        hold.press(at(0));

        assert!(!hold.release(at(1)));
        assert!(hold.is_down());
        assert!(hold.release(at(400)));
    }

    #[test]
    fn silence_after_a_repeat_is_a_release_even_without_one_from_the_portal() {
        let mut hold = Hold::new();
        hold.press(at(0));
        hold.press(at(30));
        hold.press(at(60));

        // Four intervals of thirty milliseconds is a hundred and twenty, which is
        // below the floor: the floor is what has to hold.
        assert!(!hold.expired(at(60 + 100)));
        assert!(hold.expired(at(60 + MIN_SILENCE.as_millis() as u64)));
        assert!(!hold.is_down());
    }

    #[test]
    fn before_any_repeat_nothing_is_assumed() {
        let mut hold = Hold::new();
        hold.press(at(0));

        assert!(!hold.expired(at(500)));
        assert!(hold.expired(at(UNKNOWN_SILENCE.as_millis() as u64)));
    }

    #[test]
    fn the_first_repeat_after_the_initial_delay_is_not_a_new_gesture() {
        let mut hold = Hold::new();
        assert_eq!(hold.press(at(0)), Press::Fresh);
        assert_eq!(hold.press(at(500)), Press::Repeat);
        assert_eq!(hold.press(at(530)), Press::Repeat);
    }

    #[test]
    fn a_press_after_the_gesture_closed_starts_a_new_one() {
        let mut hold = Hold::new();
        hold.press(at(0));
        hold.press(at(500));
        // The first gap is the initial delay of key repeat, not the cadence, so
        // the ceiling applies until a short one arrives.
        assert!(!hold.expired(at(500 + MIN_SILENCE.as_millis() as u64)));
        assert!(hold.expired(at(500 + MAX_SILENCE.as_millis() as u64)));
        assert_eq!(hold.press(at(6000)), Press::Fresh);
    }

    #[test]
    fn nothing_expires_while_no_gesture_is_open() {
        let mut hold = Hold::new();
        assert!(!hold.expired(at(10_000)));
    }
}
