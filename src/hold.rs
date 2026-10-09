//! Telling a held space bar from typed spaces, from key timing alone.
//!
//! Terminals report key presses, not releases, so a held key shows up as the
//! keyboard's auto-repeat: the press, a pause (the repeat delay, often 250 to
//! 600 ms), then a run of presses at a steady, fast interval. People cannot
//! press one key that fast and that evenly, so two such intervals in a row
//! mean the space bar is held. When the presses stop, it was let go.
//!
//! Until it is clear which it is, spaces wait here instead of being typed. Any
//! other key types them at once, ahead of itself, so typing "a b" is never
//! delayed. A space with nothing after it is typed once the repeat delay has
//! passed without a repeat.

use std::time::{Duration, SystemTime};

/// Repeats come faster than this; people pressing the same key, slower.
const FAST: Duration = Duration::from_millis(110);
/// Repeats are this even; people are not.
const JITTER: Duration = Duration::from_millis(20);
/// How long a space waits for a repeat before it is typed, until a hold has
/// shown the actual repeat delay. Long enough for common defaults (X11 waits
/// 660 ms before repeating, GNOME 500, macOS about 375).
const SETTLE: Duration = Duration::from_millis(700);
/// What a measured repeat delay is allowed to set the wait to.
const SETTLE_MIN: Duration = Duration::from_millis(150);
const SETTLE_MAX: Duration = Duration::from_millis(1_200);
/// Margin over the measured repeat delay.
const SETTLE_MARGIN: Duration = Duration::from_millis(100);
/// The shortest silence that means the key was released: long enough that a
/// busy machine delivering repeats late does not end a recording early, and
/// short enough to feel immediate (it also keeps the end of the last word).
const RELEASE_MIN: Duration = Duration::from_millis(150);

/// What the caller should do now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Nothing yet.
    Wait,
    /// Type this many spaces: they were typed, not held.
    Type(usize),
    /// The space bar is held: type this many earlier spaces, then start
    /// recording.
    Hold(usize),
    /// The held space bar was let go.
    Release,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpaceHold {
    /// Spaces waiting to be told apart, by arrival.
    waiting: Vec<SystemTime>,
    /// While held: the last repeat and the repeat interval.
    held: Option<(SystemTime, Duration)>,
    /// How long a lone space waits for a repeat.
    settle: Duration,
}

impl Default for SpaceHold {
    fn default() -> Self {
        SpaceHold { waiting: Vec::new(), held: None, settle: SETTLE }
    }
}

impl SpaceHold {
    /// Whether spaces are waiting or the bar is held, so the caller should
    /// tick soon.
    pub fn busy(&self) -> bool {
        !self.waiting.is_empty() || self.held.is_some()
    }

    pub fn held(&self) -> bool {
        self.held.is_some()
    }

    /// A space was pressed (or repeated).
    pub fn space(&mut self, now: SystemTime) -> Step {
        if let Some((last, interval)) = self.held.as_mut() {
            // Learn the interval from the run itself, so a slow repeat rate
            // is not taken for a release.
            *interval = (*interval).max(gap(*last, now).min(FAST));
            *last = now;
            return Step::Wait;
        }
        self.waiting.push(now);
        let n = self.waiting.len();
        if n < 4 {
            return Step::Wait;
        }
        // The press, the pause, then two even, fast repeats.
        let (press, first, second, third) = (self.waiting[n - 4], self.waiting[n - 3], self.waiting[n - 2], now);
        let (a, b) = (gap(first, second), gap(second, third));
        if a >= FAST || b >= FAST || a.abs_diff(b) > JITTER {
            return Step::Wait;
        }
        let delay = gap(press, first);
        self.settle = if delay >= FAST {
            (delay + SETTLE_MARGIN).clamp(SETTLE_MIN, SETTLE_MAX)
        } else {
            // The pause came before a space already typed: the repeat delay is
            // longer than the wait. Wait longer next time.
            (self.settle * 3 / 2).min(SETTLE_MAX)
        };
        self.held = Some((now, a.max(b)));
        let typed = n - 4;
        self.waiting.clear();
        Step::Hold(typed)
    }

    /// Another key: the waiting spaces were typed, and go ahead of it. A
    /// held bar stays held: Enter or Esc while recording are for dictation.
    pub fn interrupt(&mut self) -> usize {
        std::mem::take(&mut self.waiting).len()
    }

    /// Time passed with no space.
    pub fn tick(&mut self, now: SystemTime) -> Step {
        if let Some((last, interval)) = self.held {
            if gap(last, now) > (interval * 3).max(RELEASE_MIN) {
                self.held = None;
                return Step::Release;
            }
            return Step::Wait;
        }
        match self.waiting.last() {
            Some(&last) if gap(last, now) > self.settle => Step::Type(self.interrupt()),
            _ => Step::Wait,
        }
    }

    /// Forgets a hold without reporting its release, when what it started
    /// ended some other way.
    pub fn forget(&mut self) {
        self.held = None;
    }
}

fn gap(earlier: SystemTime, later: SystemTime) -> Duration {
    later.duration_since(earlier).unwrap_or_default()
}

#[cfg(test)]
mod tests;
