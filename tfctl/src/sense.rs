//! More from the light sensor (light.rs) than "the room is dark": read 10 times a second, it
//! sees a hand held over the hole (a wave), a shadow passing (motion, when the room is not so
//! bright that the 6-bit reading sits at its top), and sets the backlight from the room light.
//!
//! Upstream measured 60..62 in a lit room, 7..14 with the hole partly covered and 0..2 covered
//! (ormarch E064). A wave is a deep, short dip from a lit baseline; motion is a shallow change
//! that wave and dark do not explain. Thresholds are first guesses [U]: tune with the frame logs.
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Samples kept for the baseline.
const HISTORY: Duration = Duration::from_secs(3);
/// The newest samples are left out of the baseline, so a dip does not drag it down.
const FRESH: Duration = Duration::from_millis(500);
/// A wave needs a lit room: below this baseline a hand changes too little.
pub const WAVE_BASE_MIN: u8 = 12;
/// A wave is covered at least this long (a flicker is not a hand) and at most this long
/// (longer is a hand resting there, or night: light.rs handles that).
const WAVE_MIN: Duration = Duration::from_millis(150);
const WAVE_MAX: Duration = Duration::from_millis(1500);
/// Motion: the median of the last few samples moved this far from the baseline (counts, and
/// at least MOTION_PCT per cent of it), with the baseline in this range (a reading at 58+ is
/// near the converter's top, where a shadow changes nothing).
const MOTION_MIN: u8 = 3;
const MOTION_PCT: u32 = 8;
const MOTION_BASE: std::ops::RangeInclusive<u8> = 8..=57;
const MOTION_GAP: Duration = Duration::from_secs(10);

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Gesture {
    Wave,
    Motion,
}

#[derive(Debug, PartialEq)]
enum State {
    Idle,
    /// Covered since, from this baseline.
    Down(Instant, u8),
    /// Covered too long; waiting for the light to come back.
    Held(u8),
}

pub struct Sense {
    hist: VecDeque<(Instant, u8)>,
    state: State,
    last_motion: Option<Instant>,
}

impl Default for Sense {
    fn default() -> Self {
        Self::new()
    }
}

fn median(mut v: Vec<u8>) -> Option<u8> {
    if v.is_empty() {
        return None;
    }
    v.sort_unstable();
    Some(v[v.len() / 2])
}

impl Sense {
    pub fn new() -> Sense {
        Sense {
            hist: VecDeque::new(),
            state: State::Idle,
            last_motion: None,
        }
    }

    /// The room's light: median of the samples older than FRESH.
    pub fn base(&self, now: Instant) -> Option<u8> {
        median(
            self.hist
                .iter()
                .filter(|(t, _)| now.saturating_duration_since(*t) >= FRESH)
                .map(|(_, v)| *v)
                .collect(),
        )
    }

    /// Feed one reading; a gesture when one just ended.
    pub fn step(&mut self, v: u8, now: Instant) -> Option<Gesture> {
        while self
            .hist
            .front()
            .is_some_and(|(t, _)| now.saturating_duration_since(*t) > HISTORY)
        {
            self.hist.pop_front();
        }
        let base = self.base(now);
        let mut out = None;
        match self.state {
            State::Idle => {
                if let Some(b) = base.filter(|b| *b >= WAVE_BASE_MIN) {
                    if v <= (b / 4).max(3) {
                        self.state = State::Down(now, b);
                    }
                }
                if self.state == State::Idle {
                    out = self.motion(v, base, now);
                }
            }
            State::Down(since, b) => {
                let held = now.saturating_duration_since(since);
                if v >= b / 2 {
                    self.state = State::Idle;
                    if held >= WAVE_MIN {
                        out = Some(Gesture::Wave);
                        // The dip must not count as motion when it leaves the baseline.
                        self.last_motion = Some(now);
                    }
                } else if held > WAVE_MAX {
                    self.state = State::Held(b);
                }
            }
            State::Held(b) => {
                if v >= b / 2 {
                    self.state = State::Idle;
                    self.hist.clear(); // a new room light, maybe
                }
            }
        }
        // Covered samples stay out of the baseline.
        if self.state == State::Idle {
            self.hist.push_back((now, v));
        }
        out
    }

    fn motion(&mut self, v: u8, base: Option<u8>, now: Instant) -> Option<Gesture> {
        let b = base.filter(|b| MOTION_BASE.contains(b))?;
        // Enough history for a fair baseline.
        if self
            .hist
            .front()
            .is_none_or(|(t, _)| now.saturating_duration_since(*t) < Duration::from_secs(2))
        {
            return None;
        }
        let recent: Vec<u8> = self
            .hist
            .iter()
            .rev()
            .take(2)
            .map(|(_, x)| *x)
            .chain(std::iter::once(v))
            .collect();
        let m = median(recent)?;
        let need = MOTION_MIN.max(((b as u32 * MOTION_PCT).div_ceil(100)) as u8);
        if m.abs_diff(b) < need {
            return None;
        }
        if self
            .last_motion
            .is_some_and(|t| now.saturating_duration_since(t) < MOTION_GAP)
        {
            return None;
        }
        self.last_motion = Some(now);
        Some(Gesture::Motion)
    }
}

/// Auto-brightness: the share of the configured brightness for a light reading, in the
/// vendor's bands (10/17/24/31/38/45/52/59, ormarch E064): 25 % in a dim room to 100 %.
pub fn auto_pct(light: u8) -> u8 {
    const BANDS: [u8; 8] = [10, 17, 24, 31, 38, 45, 52, 59];
    let level = BANDS.iter().filter(|b| light >= **b).count() as u32;
    (25 + level * 75 / 8) as u8
}

/// One step of the backlight toward `target`, at most 4 points a second (no visible jumps).
pub fn approach(now: u8, target: u8) -> u8 {
    if now < target {
        (now + 4).min(target)
    } else {
        now.saturating_sub(4).max(target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feed `v` every 100 ms for `ms`; return gestures seen.
    fn run(s: &mut Sense, t: &mut Instant, v: u8, ms: u64) -> Vec<Gesture> {
        let mut out = vec![];
        for _ in 0..ms / 100 {
            *t += Duration::from_millis(100);
            out.extend(s.step(v, *t));
        }
        out
    }

    #[test]
    fn a_hand_over_the_hole_is_a_wave() {
        let (mut s, mut t) = (Sense::new(), Instant::now());
        assert!(run(&mut s, &mut t, 61, 3000).is_empty());
        assert!(run(&mut s, &mut t, 1, 500).is_empty());
        assert_eq!(run(&mut s, &mut t, 61, 500), vec![Gesture::Wave]);
        // Partly covered (upstream 7..14) is a wave too.
        assert!(run(&mut s, &mut t, 61, 3000).is_empty());
        run(&mut s, &mut t, 9, 400);
        assert_eq!(run(&mut s, &mut t, 60, 300), vec![Gesture::Wave]);
    }

    #[test]
    fn flicker_rest_and_dark_rooms_are_not_waves() {
        let (mut s, mut t) = (Sense::new(), Instant::now());
        run(&mut s, &mut t, 61, 3000);
        // 100 ms: a flicker.
        assert!(run(&mut s, &mut t, 0, 100).is_empty());
        assert!(run(&mut s, &mut t, 61, 3000).is_empty());
        // 5 s: a hand resting, or the lights went out.
        assert!(run(&mut s, &mut t, 0, 5000).is_empty());
        assert!(run(&mut s, &mut t, 61, 3000).is_empty());
        // A dim room has no room for a dip.
        let (mut s, mut t) = (Sense::new(), Instant::now());
        run(&mut s, &mut t, 6, 3000);
        assert!(run(&mut s, &mut t, 1, 500).is_empty());
        assert!(run(&mut s, &mut t, 6, 500).is_empty());
    }

    #[test]
    fn a_shadow_in_a_lamp_lit_room_is_motion_once() {
        let (mut s, mut t) = (Sense::new(), Instant::now());
        assert!(run(&mut s, &mut t, 40, 3000).is_empty());
        // Noise of a count or two is not motion.
        assert!(run(&mut s, &mut t, 41, 300).is_empty());
        assert!(run(&mut s, &mut t, 39, 300).is_empty());
        let g = run(&mut s, &mut t, 33, 600);
        assert_eq!(g, vec![Gesture::Motion]);
        // Not again within MOTION_GAP.
        assert!(run(&mut s, &mut t, 40, 500).is_empty());
        assert!(run(&mut s, &mut t, 33, 500).is_empty());
    }

    #[test]
    fn a_saturated_room_shows_no_motion() {
        let (mut s, mut t) = (Sense::new(), Instant::now());
        run(&mut s, &mut t, 61, 3000);
        assert!(run(&mut s, &mut t, 56, 600).is_empty());
    }

    #[test]
    fn auto_brightness_bands() {
        assert_eq!(auto_pct(0), 25);
        assert_eq!(auto_pct(9), 25);
        assert_eq!(auto_pct(10), 34);
        assert_eq!(auto_pct(35), 62);
        assert_eq!(auto_pct(61), 100);
        assert_eq!(approach(20, 100), 24);
        assert_eq!(approach(100, 97), 97);
        assert_eq!(approach(50, 50), 50);
    }
}
