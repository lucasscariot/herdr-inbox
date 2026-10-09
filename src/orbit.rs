//! The orbit: a small armillary sphere drawn in Braille dots, and the
//! inbox's logo. The core is the inbox, each ring is a machine, and the beads
//! riding a ring are that machine's threads, one per thread in its status.
//!
//! Everything here is pure: a fleet and a time in, lit cells out. The whole
//! sculpture turns once a minute and every motion in it repeats within that
//! minute, so a frame is a function of the time modulo [`TURN_MS`].

use std::f64::consts::TAU;

use crate::herdr::types::AgentStatus;

/// One full turn of the sculpture; every animation divides it.
pub const TURN_MS: u64 = 60_000;
/// How long a bead takes to go once around its ring.
const LAP_MS: u64 = 30_000;
/// The period of a working bead's pulse.
const PULSE_MS: u64 = 1_000;
/// The period of the core's breath while a thread needs input.
const BREATH_MS: u64 = 2_000;
/// Dashes around a ring that is not live.
const DASHES: f64 = 24.0;
/// The camera looks down on the sculpture a little.
const TILT: f64 = 0.38;
/// The core's radius, with rings of radius about 1.
const CORE: f64 = 0.2;
/// The radius of a ring of radius 1, as a fraction of the canvas's smaller side.
const SCALE: f64 = 0.42;
/// Rows the orbit never grows past, however much room there is.
pub const MAX_ROWS: u16 = 20;
/// Rows under which the orbit is not drawn at all.
pub const MIN_ROWS: u16 = 8;

/// How a machine's ring is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Link {
    /// Solid, with its threads riding it.
    Live,
    /// Dashes marching around it.
    Connecting,
    /// Sparse dashes, no beads.
    Down,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ring {
    pub link: Link,
    /// One bead per thread, in a stable order. Only live rings show them.
    pub beads: Vec<AgentStatus>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Fleet {
    pub rings: Vec<Ring>,
}

impl Fleet {
    /// The core breathes while any thread on a live machine needs input.
    pub fn needs_input(&self) -> bool {
        self.rings.iter().any(|ring| ring.link == Link::Live && ring.beads.contains(&AgentStatus::Blocked))
    }
}

/// What a lit cell shows, for the caller to colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ink {
    Core,
    /// `front` is the half of the ring nearer the viewer.
    Ring {
        link: Link,
        front: bool,
    },
    Bead(AgentStatus),
}

impl Ink {
    /// When dots of several inks share a cell, the cell takes the strongest.
    fn rank(self) -> u8 {
        match self {
            Ink::Bead(_) => 3,
            Ink::Core => 2,
            Ink::Ring { front: true, .. } => 1,
            Ink::Ring { front: false, .. } => 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub glyph: char,
    pub ink: Ink,
}

/// A frame of `cols` by `rows` cells, row by row; `None` is a blank cell.
pub type Frame = Vec<Option<Cell>>;

/// The size the orbit takes in a free area, if it fits: as tall as it can be
/// up to [`MAX_ROWS`], and twice as wide as tall so its dots are square.
pub fn fit(cols: u16, rows: u16) -> Option<(u16, u16)> {
    let rows = rows.min(MAX_ROWS).min(cols / 2);
    (rows >= MIN_ROWS).then_some((rows * 2, rows))
}

/// Draws the fleet at `millis`.
pub fn render(fleet: &Fleet, millis: u64, cols: u16, rows: u16) -> Frame {
    let mut canvas = Canvas::new(cols as usize * 2, rows as usize * 4);
    paint(&mut canvas, fleet, millis);
    canvas.cells(cols as usize, rows as usize)
}

/// A fraction of the way through a period that divides [`TURN_MS`].
fn phase(millis: u64, period: u64) -> f64 {
    (millis % period) as f64 / period as f64
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct V3 {
    x: f64,
    y: f64,
    z: f64,
}

impl V3 {
    const fn new(x: f64, y: f64, z: f64) -> Self {
        V3 { x, y, z }
    }
    fn scale(self, k: f64) -> Self {
        V3::new(self.x * k, self.y * k, self.z * k)
    }
    fn add(self, o: V3) -> Self {
        V3::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }
    fn cross(self, o: V3) -> Self {
        V3::new(self.y * o.z - self.z * o.y, self.z * o.x - self.x * o.z, self.x * o.y - self.y * o.x)
    }
    fn length(self) -> f64 {
        (self.x * self.x + self.y * self.y + self.z * self.z).sqrt()
    }
    fn unit(self) -> Self {
        self.scale(1.0 / self.length())
    }
}

/// The plane of ring `index`: two unit vectors spanning it, and its radius.
/// Rings lean at different angles and spread around the vertical axis, like
/// the rings of an armillary sphere; each is a little smaller than the last
/// so beads never sit exactly where two rings cross.
fn ring_plane(index: usize, count: usize) -> (V3, V3, f64) {
    const LEANS: [f64; 6] = [0.42, 1.1, 1.1, 0.75, 1.35, 0.75];
    let lean = LEANS[index % LEANS.len()];
    let around = std::f64::consts::PI * index as f64 / count.max(1) as f64 + 0.3;
    let normal = V3::new(lean.sin() * around.cos(), lean.cos(), lean.sin() * around.sin());
    let u = normal.cross(V3::new(0.0, 1.0, 0.0)).unit();
    let v = normal.cross(u).unit();
    let radius = (1.0 - 0.05 * index as f64).max(0.7);
    (u, v, radius)
}

/// Turns the sculpture about the vertical axis, then tilts it towards the
/// viewer. `z` grows towards the viewer.
struct View {
    yaw: (f64, f64),
    tilt: (f64, f64),
}

impl View {
    fn at(millis: u64) -> Self {
        let yaw = TAU * phase(millis, TURN_MS);
        View { yaw: (yaw.sin(), yaw.cos()), tilt: (TILT.sin(), TILT.cos()) }
    }

    fn apply(&self, p: V3) -> V3 {
        let (s, c) = self.yaw;
        let p = V3::new(p.x * c + p.z * s, p.y, -p.x * s + p.z * c);
        let (s, c) = self.tilt;
        V3::new(p.x, p.y * c - p.z * s, p.y * s + p.z * c)
    }
}

/// A grid of dots, each holding the depth of the nearest thing drawn there
/// and, if that thing is lit, its ink.
struct Canvas {
    width: usize,
    height: usize,
    dots: Vec<Option<(f64, Option<Ink>)>>,
}

impl Canvas {
    fn new(width: usize, height: usize) -> Self {
        Canvas { width, height, dots: vec![None; width * height] }
    }

    /// Lights a dot (or, with no ink, only hides what is behind it) unless
    /// something nearer is already there.
    fn plot(&mut self, x: f64, y: f64, z: f64, ink: Option<Ink>) {
        let (x, y) = (x.round(), y.round());
        if x < 0.0 || y < 0.0 || x >= self.width as f64 || y >= self.height as f64 {
            return;
        }
        let dot = &mut self.dots[y as usize * self.width + x as usize];
        if dot.is_none_or(|(nearest, _)| z > nearest) {
            *dot = Some((z, ink));
        }
    }

    fn ink(&self, x: usize, y: usize) -> Option<(f64, Ink)> {
        self.dots[y * self.width + x].and_then(|(z, ink)| ink.map(|ink| (z, ink)))
    }

    /// Packs the dots into Braille cells, two wide and four tall.
    fn cells(&self, cols: usize, rows: usize) -> Frame {
        let mut frame = Vec::with_capacity(cols * rows);
        for row in 0..rows {
            for col in 0..cols {
                let mut mask = 0u32;
                let mut best: Option<(f64, Ink)> = None;
                for dy in 0..4 {
                    for dx in 0..2 {
                        let Some((z, ink)) = self.ink(col * 2 + dx, row * 4 + dy) else { continue };
                        mask |= braille_bit(dx, dy);
                        if best.is_none_or(|(best_z, best_ink)| (ink.rank(), z) > (best_ink.rank(), best_z)) {
                            best = Some((z, ink));
                        }
                    }
                }
                frame.push(
                    best.map(|(_, ink)| Cell { glyph: char::from_u32(0x2800 + mask).expect("Braille block"), ink }),
                );
            }
        }
        frame
    }
}

/// The bit of a Braille cell for the dot `dx` across and `dy` down.
fn braille_bit(dx: usize, dy: usize) -> u32 {
    const BITS: [[u32; 2]; 4] = [[0x01, 0x08], [0x02, 0x10], [0x04, 0x20], [0x40, 0x80]];
    BITS[dy][dx]
}

fn paint(canvas: &mut Canvas, fleet: &Fleet, millis: u64) {
    if canvas.width == 0 || canvas.height == 0 {
        return;
    }
    let view = View::at(millis);
    // Dots are square, so one scale serves both axes. Rings of radius 1 leave
    // room for a near bead at their edge.
    let scale = SCALE * canvas.width.min(canvas.height) as f64;
    let center = ((canvas.width as f64 - 1.0) / 2.0, (canvas.height as f64 - 1.0) / 2.0);
    let project = |p: V3| (center.0 + p.x * scale, center.1 - p.y * scale, p.z);

    core(canvas, fleet, millis, center, scale);
    let count = fleet.rings.len();
    for (index, ring) in fleet.rings.iter().enumerate() {
        let (u, v, radius) = ring_plane(index, count);
        let at = |angle: f64| view.apply(u.scale(angle.cos() * radius).add(v.scale(angle.sin() * radius)));
        // Enough samples that neighbouring ones land on neighbouring dots.
        let samples = ((TAU * radius * scale * 2.5) as usize).max(48);
        let flow = (millis / 500) as usize;
        for sample in 0..samples {
            let turn = sample as f64 / samples as f64;
            let dash = (turn * DASHES) as usize;
            let lit = match ring.link {
                Link::Live => true,
                Link::Connecting => (dash + flow).is_multiple_of(2),
                Link::Down => dash.is_multiple_of(3),
            };
            let p = at(TAU * turn);
            let (x, y, z) = project(p);
            canvas.plot(x, y, z, lit.then_some(Ink::Ring { link: ring.link, front: p.z > 0.0 }));
        }
        if ring.link != Link::Live || ring.beads.is_empty() {
            continue;
        }
        let direction = if index % 2 == 0 { 1.0 } else { -1.0 };
        let travel = direction * TAU * phase(millis, LAP_MS);
        let pulse = (TAU * phase(millis, PULSE_MS)).sin();
        let base = (scale * 0.07).clamp(0.8, 2.4);
        for (slot, &status) in ring.beads.iter().enumerate() {
            let p = at(TAU * slot as f64 / ring.beads.len() as f64 + travel);
            let size = match status {
                AgentStatus::Working => 1.0 + 0.25 * pulse,
                AgentStatus::Idle | AgentStatus::Unknown => 0.75,
                AgentStatus::Blocked | AgentStatus::Done => 1.15,
            };
            let radius = base * size * (1.0 + 0.25 * p.z);
            let (x, y, z) = project(p);
            disc(canvas, x, y, radius, z + 0.05, Ink::Bead(status));
        }
    }
}

/// A filled disc of dots at one depth.
fn disc(canvas: &mut Canvas, cx: f64, cy: f64, radius: f64, z: f64, ink: Ink) {
    let reach = radius.ceil() as i64;
    for dy in -reach..=reach {
        for dx in -reach..=reach {
            if ((dx * dx + dy * dy) as f64) <= radius * radius + 0.25 {
                canvas.plot(cx + dx as f64, cy + dy as f64, z, Some(ink));
            }
        }
    }
}

/// The core: a lit sphere, dithered darker away from the light. Its unlit
/// dots still hide the rings passing behind it.
fn core(canvas: &mut Canvas, fleet: &Fleet, millis: u64, center: (f64, f64), scale: f64) {
    let breath = if fleet.needs_input() { 1.0 + 0.12 * (TAU * phase(millis, BREATH_MS)).sin() } else { 1.0 };
    let world = CORE * breath;
    let radius = (world * scale).max(1.0);
    let light = V3::new(-0.45, 0.55, 0.7).unit();
    let reach = radius.ceil() as i64;
    for dy in -reach..=reach {
        for dx in -reach..=reach {
            let (nx, ny) = (dx as f64 / radius, -dy as f64 / radius);
            let flat = nx * nx + ny * ny;
            if flat > 1.0 {
                continue;
            }
            let nz = (1.0 - flat).sqrt();
            let shade = nx * light.x + ny * light.y + nz * light.z;
            let (x, y) = (center.0.round() as i64 + dx, center.1.round() as i64 + dy);
            let lit = shade > 0.55 || (shade > 0.2 && (x + y) % 2 == 0) || (shade > -0.2 && x % 2 == 0 && y % 4 == 0);
            canvas.plot(x as f64, y as f64, nz * world, lit.then_some(Ink::Core));
        }
    }
}

#[cfg(test)]
mod tests;
