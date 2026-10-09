use super::*;

use AgentStatus::{Blocked, Done, Idle, Working};

fn live(beads: &[AgentStatus]) -> Ring {
    Ring { link: Link::Live, beads: beads.to_vec() }
}

fn fleet(rings: Vec<Ring>) -> Fleet {
    Fleet { rings }
}

/// The dots of one frame, for questions about the drawing itself.
fn canvas(fleet: &Fleet, millis: u64, cols: usize, rows: usize) -> Canvas {
    let mut canvas = Canvas::new(cols * 2, rows * 4);
    paint(&mut canvas, fleet, millis);
    canvas
}

fn lit(canvas: &Canvas, keep: impl Fn(Ink) -> bool) -> usize {
    (0..canvas.height)
        .flat_map(|y| (0..canvas.width).map(move |x| (x, y)))
        .filter(|&(x, y)| canvas.ink(x, y).is_some_and(|(_, ink)| keep(ink)))
        .count()
}

/// Every ink seen in a frame, over a few moments of a turn.
fn inks_over_a_turn(fleet: &Fleet, cols: u16, rows: u16) -> Vec<Ink> {
    let mut seen = Vec::new();
    for step in 0..12 {
        for cell in render(fleet, step * 5_000, cols, rows).into_iter().flatten() {
            if !seen.contains(&cell.ink) {
                seen.push(cell.ink);
            }
        }
    }
    seen
}

#[test]
fn braille_bits_follow_the_unicode_dot_numbering() {
    // Dots 1-2-3 go down the left column, 4-5-6 down the right, 7 and 8 last.
    assert_eq!(braille_bit(0, 0), 0x01);
    assert_eq!(braille_bit(0, 1), 0x02);
    assert_eq!(braille_bit(0, 2), 0x04);
    assert_eq!(braille_bit(1, 0), 0x08);
    assert_eq!(braille_bit(1, 1), 0x10);
    assert_eq!(braille_bit(1, 2), 0x20);
    assert_eq!(braille_bit(0, 3), 0x40);
    assert_eq!(braille_bit(1, 3), 0x80);
    let mut canvas = Canvas::new(2, 4);
    canvas.plot(0.0, 0.0, 0.0, Some(Ink::Core));
    canvas.plot(1.0, 3.0, 0.0, Some(Ink::Core));
    assert_eq!(canvas.cells(1, 1), vec![Some(Cell { glyph: '⢁', ink: Ink::Core })]);
}

#[test]
fn a_frame_has_one_entry_per_cell_and_any_size_is_safe() {
    let fleet = fleet(vec![live(&[Blocked, Working]), live(&[Idle]), Ring { link: Link::Down, beads: vec![] }]);
    for cols in 0..24 {
        for rows in 0..9 {
            let frame = render(&fleet, 12_345, cols, rows);
            assert_eq!(frame.len(), cols as usize * rows as usize, "{cols}x{rows}");
        }
    }
    assert!(render(&fleet, 0, 0, 0).is_empty());
}

#[test]
fn blank_cells_are_none_and_lit_ones_carry_braille() {
    let frame = render(&fleet(vec![live(&[Done])]), 0, 40, 20);
    let lit: Vec<_> = frame.iter().flatten().collect();
    assert!(!lit.is_empty());
    assert!(frame.iter().any(Option::is_none), "the corners stay blank");
    for cell in lit {
        assert!(('\u{2801}'..='\u{28FF}').contains(&cell.glyph), "{:?}", cell.glyph);
    }
}

#[test]
fn it_fits_twice_as_wide_as_tall_within_bounds() {
    assert_eq!(fit(200, 60), Some((MAX_ROWS * 2, MAX_ROWS)), "never larger than MAX_ROWS");
    assert_eq!(fit(30, 60), Some((30, 15)), "narrow areas set the size");
    assert_eq!(fit(80, 10), Some((20, 10)));
    assert_eq!(fit(80, MIN_ROWS), Some((MIN_ROWS * 2, MIN_ROWS)));
    assert_eq!(fit(80, MIN_ROWS - 1), None, "too short");
    assert_eq!(fit(MIN_ROWS * 2 - 1, 40), None, "too narrow");
    assert_eq!(fit(0, 0), None);
}

#[test]
fn with_no_machines_only_the_core_is_drawn() {
    let inks = inks_over_a_turn(&Fleet::default(), 40, 20);
    assert_eq!(inks, vec![Ink::Core]);
}

#[test]
fn the_core_sits_in_the_middle_and_only_nearer_things_cover_it() {
    let alone = canvas(&Fleet::default(), 0, 40, 20);
    let middle = (alone.width / 2, alone.height / 2);
    assert_eq!(alone.ink(middle.0, middle.1).map(|(_, ink)| ink), Some(Ink::Core));
    let fleet = fleet(vec![live(&[Blocked; 6]), live(&[Done; 6]), live(&[Working; 6])]);
    let mut covered = 0;
    for step in 0..120 {
        let canvas = canvas(&fleet, step * 500, 40, 20);
        let (z, ink) = canvas.ink(middle.0, middle.1).expect("the middle is always lit");
        if ink != Ink::Core {
            covered += 1;
            assert!(z > CORE, "{ink:?} at depth {z} covers the core at {}ms", step * 500);
        }
    }
    assert!(covered < 60, "the core shows most of the time, covered {covered} of 120");
}

#[test]
fn the_core_is_shaded_lit_side_up_and_left() {
    let canvas = canvas(&Fleet::default(), 0, 40, 20);
    let (cx, cy) = (canvas.width / 2, canvas.height / 2);
    let reach = (CORE * SCALE * canvas.height as f64) as usize - 1;
    let count = |xs: std::ops::Range<usize>, ys: std::ops::Range<usize>| {
        ys.flat_map(|y| xs.clone().map(move |x| (x, y))).filter(|&(x, y)| canvas.ink(x, y).is_some()).count()
    };
    let upper_left = count(cx - reach..cx, cy - reach..cy);
    let lower_right = count(cx + 1..cx + reach + 1, cy + 1..cy + reach + 1);
    assert!(upper_left > lower_right * 2, "lit {upper_left} against shadow {lower_right}");
    assert!(lower_right > 0, "the shadow side is dithered, not empty");
}

#[test]
fn the_core_hides_the_ring_behind_it() {
    let fleet = fleet(vec![live(&[])]);
    // Wherever the core is, no dot shows the back of a ring.
    for step in 0..60 {
        let canvas = canvas(&fleet, step * 1_000, 40, 20);
        let (cx, cy) = (canvas.width as f64 / 2.0, canvas.height as f64 / 2.0);
        let inner = CORE * SCALE * canvas.height as f64 - 1.5;
        for y in 0..canvas.height {
            for x in 0..canvas.width {
                let near = ((x as f64 - cx).powi(2) + (y as f64 - cy).powi(2)).sqrt() < inner;
                if let (true, Some((_, Ink::Ring { front: false, .. }))) = (near, canvas.ink(x, y)) {
                    panic!("back of the ring shows through the core at ({x}, {y}) at {step}s");
                }
            }
        }
    }
}

#[test]
fn there_is_one_ring_per_machine_with_a_front_and_a_back() {
    let rings = |count: usize| canvas(&fleet((0..count).map(|_| live(&[])).collect()), 7_000, 60, 30);
    let three = rings(3);
    let front = lit(&three, |ink| ink == Ink::Ring { link: Link::Live, front: true });
    let back = lit(&three, |ink| ink == Ink::Ring { link: Link::Live, front: false });
    assert!(front > 50 && back > 50, "front {front}, back {back}");
    // Each extra ring adds roughly a ring's worth of dots.
    let one = lit(&rings(1), |ink| matches!(ink, Ink::Ring { .. }));
    let all = lit(&three, |ink| matches!(ink, Ink::Ring { .. }));
    assert!(all as f64 > one as f64 * 2.2, "one ring {one}, three rings {all}");
}

#[test]
fn rings_lean_differently_and_shrink_a_little() {
    let planes: Vec<_> = (0..4).map(|index| ring_plane(index, 4)).collect();
    for (index, (u, v, radius)) in planes.iter().enumerate() {
        assert!((u.length() - 1.0).abs() < 1e-9 && (v.length() - 1.0).abs() < 1e-9, "unit basis for ring {index}");
        let dot = u.x * v.x + u.y * v.y + u.z * v.z;
        assert!(dot.abs() < 1e-9, "orthogonal basis for ring {index}");
        assert!((0.7..=1.0).contains(radius));
    }
    let normals: Vec<V3> = planes.iter().map(|(u, v, _)| u.cross(*v)).collect();
    for a in 0..normals.len() {
        for b in a + 1..normals.len() {
            let same = (normals[a].x * normals[b].x + normals[a].y * normals[b].y + normals[a].z * normals[b].z).abs();
            assert!(same < 0.99, "rings {a} and {b} lie in the same plane");
        }
    }
    assert!(planes.windows(2).all(|w| w[1].2 < w[0].2));
    assert_eq!(ring_plane(40, 41).2, 0.7, "a crowded fleet keeps a minimum radius");
}

#[test]
fn every_bead_shows_in_its_own_status() {
    let fleet = fleet(vec![live(&[Blocked, Done, Working, Idle])]);
    let inks = inks_over_a_turn(&fleet, 60, 30);
    for status in [Blocked, Done, Working, Idle] {
        assert!(inks.contains(&Ink::Bead(status)), "no {status:?} bead in {inks:?}");
    }
}

#[test]
fn a_bead_is_drawn_per_thread() {
    // Spread out on a big canvas, each bead is its own blob of dots.
    for count in [1, 2, 5] {
        let fleet = fleet(vec![live(&vec![Done; count])]);
        let canvas = canvas(&fleet, 3_000, 80, 40);
        assert_eq!(blobs(&canvas, Ink::Bead(Done)), count, "{count} beads");
    }
}

/// Connected groups of dots of one ink.
fn blobs(canvas: &Canvas, ink: Ink) -> usize {
    let mut seen = vec![false; canvas.width * canvas.height];
    let mut count = 0;
    for start in 0..seen.len() {
        let is = |i: usize| canvas.ink(i % canvas.width, i / canvas.width).is_some_and(|(_, k)| k == ink);
        if seen[start] || !is(start) {
            continue;
        }
        count += 1;
        let mut stack = vec![start];
        seen[start] = true;
        while let Some(i) = stack.pop() {
            let (x, y) = ((i % canvas.width) as i64, (i / canvas.width) as i64);
            for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1), (-1, -1), (1, 1), (-1, 1), (1, -1)] {
                let (nx, ny) = (x + dx, y + dy);
                if nx < 0 || ny < 0 || nx >= canvas.width as i64 || ny >= canvas.height as i64 {
                    continue;
                }
                let n = ny as usize * canvas.width + nx as usize;
                if !seen[n] && is(n) {
                    seen[n] = true;
                    stack.push(n);
                }
            }
        }
    }
    count
}

#[test]
fn beads_ride_live_rings_only() {
    for link in [Link::Connecting, Link::Down] {
        let fleet = fleet(vec![Ring { link, beads: vec![Blocked, Done, Working] }]);
        let inks = inks_over_a_turn(&fleet, 60, 30);
        assert!(inks.iter().all(|ink| !matches!(ink, Ink::Bead(_))), "{link:?}: {inks:?}");
        assert!(inks.contains(&Ink::Ring { link, front: true }), "{link:?} ring is still drawn");
    }
}

#[test]
fn rings_that_are_not_live_are_dashed() {
    let solid = lit(&canvas(&fleet(vec![live(&[])]), 0, 60, 30), |ink| matches!(ink, Ink::Ring { .. }));
    let connecting = lit(&canvas(&fleet(vec![Ring { link: Link::Connecting, beads: vec![] }]), 0, 60, 30), |ink| {
        matches!(ink, Ink::Ring { .. })
    });
    let down = lit(&canvas(&fleet(vec![Ring { link: Link::Down, beads: vec![] }]), 0, 60, 30), |ink| {
        matches!(ink, Ink::Ring { .. })
    });
    let ratio = |n: usize| n as f64 / solid as f64;
    assert!((0.35..0.65).contains(&ratio(connecting)), "connecting is half dashes: {}", ratio(connecting));
    assert!((0.2..0.45).contains(&ratio(down)), "down is a third: {}", ratio(down));
}

#[test]
fn the_dashes_of_a_connecting_ring_march() {
    let fleet = fleet(vec![Ring { link: Link::Connecting, beads: vec![] }]);
    // The view turns too; compare two moments the view barely moves between.
    let a = canvas(&fleet, 0, 60, 30);
    let b = canvas(&fleet, 500, 60, 30);
    let moved = (0..a.dots.len())
        .filter(|&i| a.dots[i].and_then(|d| d.1).is_some() != b.dots[i].and_then(|d| d.1).is_some())
        .count();
    let lit_a = lit(&a, |_| true);
    assert!(moved > lit_a / 2, "{moved} of {lit_a} dots changed");
}

#[test]
fn it_turns_once_a_minute_and_repeats_exactly() {
    let fleet = fleet(vec![
        live(&[Blocked, Working]),
        live(&[Done, Idle, Idle]),
        Ring { link: Link::Connecting, beads: vec![] },
    ]);
    for millis in [0, 1_234, 29_999, 45_100] {
        assert_eq!(render(&fleet, millis, 50, 25), render(&fleet, millis + TURN_MS, 50, 25), "at {millis}ms");
        assert_eq!(render(&fleet, millis, 50, 25), render(&fleet, millis + 7 * TURN_MS, 50, 25));
    }
    assert_ne!(render(&fleet, 0, 50, 25), render(&fleet, 5_000, 50, 25), "it moves within the minute");
}

#[test]
fn every_period_divides_the_turn() {
    for period in [LAP_MS, PULSE_MS, BREATH_MS, 500] {
        assert_eq!(TURN_MS % period, 0, "{period}ms");
    }
}

#[test]
fn the_sculpture_is_centred() {
    let fleet = fleet(vec![live(&[Done; 4]), live(&[Blocked; 3]), live(&[Working; 5])]);
    for millis in (0..TURN_MS).step_by(1_500) {
        let canvas = canvas(&fleet, millis, 40, 20);
        let dots: Vec<(usize, usize)> = (0..canvas.height)
            .flat_map(|y| (0..canvas.width).map(move |x| (x, y)))
            .filter(|&(x, y)| canvas.ink(x, y).is_some())
            .collect();
        let (min_x, max_x) = (dots.iter().map(|d| d.0).min().unwrap(), dots.iter().map(|d| d.0).max().unwrap());
        let (min_y, max_y) = (dots.iter().map(|d| d.1).min().unwrap(), dots.iter().map(|d| d.1).max().unwrap());
        let left = min_x;
        let right = canvas.width - 1 - max_x;
        let top = min_y;
        let bottom = canvas.height - 1 - max_y;
        assert!(min_x > 0 && min_y > 0 && right > 0 && bottom > 0, "nothing touches the edge at {millis}ms");
        // Sideways it is symmetric up to the turn; up and down, only within
        // the tilt.
        assert!(left.abs_diff(right) <= canvas.width / 4, "x margins {left} and {right} at {millis}ms");
        assert!(top.abs_diff(bottom) <= canvas.height / 4, "y margins {top} and {bottom} at {millis}ms");
    }
}

#[test]
fn the_core_breathes_only_while_a_thread_needs_input() {
    let core_dots = |fleet: &Fleet, millis| lit(&canvas(fleet, millis, 60, 30), |ink| ink == Ink::Core);
    // A quarter breath in, the core is at its largest; three quarters, its
    // smallest. With nothing waiting, it stays the same.
    let calm = fleet(vec![live(&[Done, Working, Idle])]);
    assert!(!calm.needs_input());
    assert_eq!(core_dots(&calm, BREATH_MS / 4), core_dots(&calm, BREATH_MS * 3 / 4));
    let waiting = fleet(vec![live(&[Done, Blocked])]);
    assert!(waiting.needs_input());
    let inhale = core_dots(&waiting, BREATH_MS / 4);
    let exhale = core_dots(&waiting, BREATH_MS * 3 / 4);
    assert!(inhale > exhale, "inhale {inhale}, exhale {exhale}");
}

#[test]
fn a_blocked_thread_on_a_machine_that_is_not_live_does_not_count() {
    let fleet = fleet(vec![Ring { link: Link::Down, beads: vec![Blocked] }]);
    assert!(!fleet.needs_input());
}

#[test]
fn a_working_bead_pulses_and_an_idle_one_is_smaller() {
    // One bead at the same moment and so the same place: only its size
    // depends on its status.
    let size =
        |status, millis| lit(&canvas(&fleet(vec![live(&[status])]), millis, 80, 40), |ink| ink == Ink::Bead(status));
    let (swell, shrink) = (PULSE_MS / 4, PULSE_MS * 3 / 4);
    assert!(size(Working, swell) > size(Done, swell), "a working bead swells past a ready one");
    assert!(size(Working, shrink) < size(Done, shrink), "then shrinks under it");
    for millis in [0, swell, shrink] {
        assert_eq!(size(Blocked, millis), size(Done, millis), "beads that want you are the same size");
        assert!(size(Idle, millis) < size(Done, millis), "idle beads are smaller");
    }
}

#[test]
fn a_crowded_ring_stays_inside_the_frame() {
    let fleet = fleet((0..8).map(|_| live(&[Working; 40])).collect());
    for millis in [0, 10_000, 20_000] {
        let frame = render(&fleet, millis, 12, 6);
        assert_eq!(frame.len(), 72);
    }
}

#[test]
fn a_bead_outranks_the_ring_and_core_in_a_shared_cell() {
    let mut canvas = Canvas::new(2, 4);
    canvas.plot(0.0, 0.0, 0.9, Some(Ink::Ring { link: Link::Live, front: true }));
    canvas.plot(1.0, 0.0, 0.8, Some(Ink::Core));
    canvas.plot(0.0, 1.0, -0.5, Some(Ink::Bead(Blocked)));
    assert_eq!(canvas.cells(1, 1)[0].map(|c| c.ink), Some(Ink::Bead(Blocked)));
    // Between two rings, the nearer one wins.
    let mut canvas = Canvas::new(2, 4);
    canvas.plot(0.0, 0.0, -0.3, Some(Ink::Ring { link: Link::Live, front: false }));
    canvas.plot(1.0, 1.0, 0.2, Some(Ink::Ring { link: Link::Down, front: true }));
    assert_eq!(canvas.cells(1, 1)[0].map(|c| c.ink), Some(Ink::Ring { link: Link::Down, front: true }));
}

#[test]
fn a_nearer_dot_wins_and_an_unlit_one_still_hides() {
    let mut canvas = Canvas::new(2, 4);
    canvas.plot(0.0, 0.0, 0.0, Some(Ink::Ring { link: Link::Live, front: true }));
    canvas.plot(0.0, 0.0, -1.0, Some(Ink::Bead(Done)));
    assert_eq!(canvas.ink(0, 0).map(|d| d.1), Some(Ink::Ring { link: Link::Live, front: true }), "behind: ignored");
    canvas.plot(0.0, 0.0, 1.0, None);
    assert_eq!(canvas.ink(0, 0), None, "an unlit nearer dot hides it");
    canvas.plot(-1.0, 0.0, 5.0, Some(Ink::Core));
    canvas.plot(2.0, 0.0, 5.0, Some(Ink::Core));
    canvas.plot(0.0, 4.0, 5.0, Some(Ink::Core));
    assert_eq!(canvas.cells(1, 1), vec![None], "off-canvas dots are dropped");
}
