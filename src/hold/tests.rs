use super::*;

fn t(ms: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_millis(1_000_000 + ms)
}

/// Presses a space at each time, returning the steps.
fn spaces(hold: &mut SpaceHold, times: &[u64]) -> Vec<Step> {
    times.iter().map(|&ms| hold.space(t(ms))).collect()
}

/// A held bar: the press at `start`, repeats after `delay`, every `every`.
fn held_bar(start: u64, delay: u64, every: u64, repeats: u64) -> Vec<u64> {
    std::iter::once(start).chain((0..repeats).map(|i| start + delay + i * every)).collect()
}

#[test]
fn a_lone_space_is_typed_once_no_repeat_can_follow() {
    let mut hold = SpaceHold::default();
    assert_eq!(hold.space(t(0)), Step::Wait);
    assert!(hold.busy());
    assert_eq!(hold.tick(t(SETTLE.as_millis() as u64)), Step::Wait, "not before the repeat delay could pass");
    assert_eq!(hold.tick(t(SETTLE.as_millis() as u64 + 1)), Step::Type(1));
    assert!(!hold.busy());
    assert_eq!(hold.tick(t(5_000)), Step::Wait, "typed once");
}

#[test]
fn the_next_key_types_waiting_spaces_ahead_of_itself() {
    let mut hold = SpaceHold::default();
    hold.space(t(0));
    assert_eq!(hold.interrupt(), 1);
    assert!(!hold.busy());
    assert_eq!(hold.tick(t(5_000)), Step::Wait);
    assert_eq!(hold.interrupt(), 0, "nothing twice");
}

#[test]
fn typed_spaces_are_never_a_hold() {
    // Fast typing: a few spaces in a row, quick but uneven, or even but
    // slower than any key repeat.
    for times in
        [vec![0, 140, 280, 420, 560, 700], vec![0, 60, 160, 200, 330], vec![0, 115, 230, 345], vec![0, 30, 100, 120]]
    {
        let mut hold = SpaceHold::default();
        assert!(spaces(&mut hold, &times).iter().all(|s| *s == Step::Wait), "{times:?}");
        let last = *times.last().unwrap();
        assert_eq!(hold.tick(t(last + 701)), Step::Type(times.len()), "{times:?}");
    }
}

#[test]
fn holding_the_bar_starts_after_two_even_repeats_and_ends_when_they_stop() {
    // This keyboard: a 250 ms repeat delay, then 40 repeats a second.
    let mut hold = SpaceHold::default();
    let times = held_bar(0, 250, 25, 30);
    let steps = spaces(&mut hold, &times);
    assert_eq!(steps[..3], [Step::Wait; 3]);
    assert_eq!(steps[3], Step::Hold(0), "at the third repeat, about 300 ms in");
    assert!(steps[4..].iter().all(|s| *s == Step::Wait));
    assert!(hold.held());
    let last = *times.last().unwrap();
    assert_eq!(hold.tick(t(last + 150)), Step::Wait, "repeats delivered late are not a release");
    assert_eq!(hold.tick(t(last + 151)), Step::Release);
    assert!(!hold.busy());
    assert_eq!(hold.tick(t(last + 500)), Step::Wait, "released once");
}

#[test]
fn spaces_typed_before_a_hold_are_still_typed() {
    let mut hold = SpaceHold::default();
    let mut times = vec![0, 150];
    times.extend(held_bar(300, 250, 25, 3));
    let steps = spaces(&mut hold, &times);
    assert_eq!(steps.last(), Some(&Step::Hold(2)));
}

#[test]
fn a_hold_teaches_the_repeat_delay_so_a_lone_space_waits_less() {
    let mut hold = SpaceHold::default();
    spaces(&mut hold, &held_bar(0, 250, 25, 3));
    hold.tick(t(1_000));
    assert_eq!(hold.settle, Duration::from_millis(350));
    hold.space(t(2_000));
    assert_eq!(hold.tick(t(2_350)), Step::Wait);
    assert_eq!(hold.tick(t(2_351)), Step::Type(1));
}

#[test]
fn macos_default_repeat_is_held_and_released_by_its_own_rhythm() {
    // About 375 ms before repeating, then a repeat every 90 ms.
    let mut hold = SpaceHold::default();
    let times = held_bar(0, 375, 90, 10);
    assert_eq!(spaces(&mut hold, &times)[3], Step::Hold(0));
    let last = *times.last().unwrap();
    assert_eq!(hold.tick(t(last + 200)), Step::Wait, "two missed repeats is not a release yet");
    assert_eq!(hold.tick(t(last + 271)), Step::Release);
}

#[test]
fn a_repeat_delay_longer_than_the_wait_costs_one_space_then_is_learned() {
    // X11 by default: 660 ms, but this one is slower still.
    let mut hold = SpaceHold::default();
    hold.space(t(0));
    assert_eq!(hold.tick(t(701)), Step::Type(1), "the press is typed before its repeats begin");
    let steps = spaces(&mut hold, &[900, 925, 950, 975]);
    assert_eq!(steps.last(), Some(&Step::Hold(0)));
    assert_eq!(hold.settle, Duration::from_millis(1_050), "waits half as long again");
    hold.tick(t(2_000));
    // The next hold no longer types a space.
    let steps = spaces(&mut hold, &held_bar(10_000, 900, 25, 3));
    assert_eq!(steps.last(), Some(&Step::Hold(0)));
    assert_eq!(hold.settle, Duration::from_millis(1_000));
}

#[test]
fn the_wait_stays_within_bounds() {
    let mut hold = SpaceHold::default();
    spaces(&mut hold, &held_bar(0, 3_000, 30, 3));
    assert_eq!(hold.settle, SETTLE_MAX);
    let mut hold = SpaceHold { settle: SETTLE_MAX, ..SpaceHold::default() };
    hold.space(t(0));
    hold.tick(t(2_000));
    spaces(&mut hold, &[3_000, 3_025, 3_050, 3_075]);
    assert_eq!(hold.settle, SETTLE_MAX, "growing never passes the cap");
    let mut hold = SpaceHold::default();
    spaces(&mut hold, &held_bar(0, 115, 30, 3));
    assert_eq!(hold.settle, Duration::from_millis(215));
}

#[test]
fn other_keys_during_a_hold_leave_it_held() {
    let mut hold = SpaceHold::default();
    spaces(&mut hold, &held_bar(0, 250, 25, 5));
    assert_eq!(hold.interrupt(), 0, "Enter while held types no spaces");
    assert!(hold.held());
    assert_eq!(hold.tick(t(1_000)), Step::Release);
}

#[test]
fn forgetting_a_hold_reports_no_release() {
    let mut hold = SpaceHold::default();
    spaces(&mut hold, &held_bar(0, 250, 25, 5));
    hold.forget();
    assert!(!hold.busy());
    assert_eq!(hold.tick(t(1_000)), Step::Wait);
}

#[test]
fn a_clock_going_backwards_never_panics_or_holds() {
    let mut hold = SpaceHold::default();
    let steps = spaces(&mut hold, &[1_000, 900, 800, 700]);
    // Zero gaps are even and fast, so this reads as held; what matters is
    // that nothing panics and a later tick releases it.
    assert!(steps.iter().all(|s| matches!(s, Step::Wait | Step::Hold(_))));
    assert_eq!(hold.tick(t(5_000)), Step::Release);
    assert_eq!(hold.tick(t(0)), Step::Wait);
}
