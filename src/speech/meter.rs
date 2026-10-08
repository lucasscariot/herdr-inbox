//! Live microphone levels while dictating.
//!
//! The recorder streams its WAV to disk, so the meter tails that file instead
//! of opening the microphone twice. Each tick it reads what was written since
//! the last one, measures twelve frequency bands over the newest samples with
//! a small FFT, and smooths them like a hardware equalizer: instant attack,
//! slow decay. The constants match the legacy plugin's meter.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub const SAMPLE_RATE: f64 = 16_000.0;
pub const BANDS: usize = 12;
/// Samples analysed per tick: 64 ms at 16 kHz.
const WINDOW: usize = 1024;
/// Per tick, when a band gets quieter.
const DECAY: f32 = 0.72;
/// Per tick: the noise floor creeps back up within a few seconds.
const FLOOR_RISE: f32 = 0.004;
/// A band at -54 dBFS is empty; at -8 dBFS it is full.
const FLOOR_DB: f64 = -54.0;
const RANGE_DB: f64 = 46.0;
/// Below this peak (about -46 dBFS) the microphone counts as silent.
const QUIET_PEAK: f32 = 0.005;
/// Seconds of silence before the UI mentions it.
const QUIET_AFTER: Duration = Duration::from_secs(3);
const HEADER_LIMIT: usize = 4096;

pub const BLOCKS: [char; 9] = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// `count + 1` log-spaced band edges across the voice range.
pub fn band_edges(count: usize, low: f64, high: f64) -> Vec<f64> {
    let ratio = (high / low).powf(1.0 / count as f64);
    (0..=count).map(|i| low * ratio.powi(i as i32)).collect()
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Complex {
    re: f64,
    im: f64,
}

impl Complex {
    fn mul(self, o: Complex) -> Complex {
        Complex { re: self.re * o.re - self.im * o.im, im: self.re * o.im + self.im * o.re }
    }
    fn add(self, o: Complex) -> Complex {
        Complex { re: self.re + o.re, im: self.im + o.im }
    }
    fn sub(self, o: Complex) -> Complex {
        Complex { re: self.re - o.re, im: self.im - o.im }
    }
    fn norm_sqr(self) -> f64 {
        self.re * self.re + self.im * self.im
    }
}

/// An iterative radix-2 FFT over a power-of-two length.
fn fft(values: &[f64]) -> Vec<Complex> {
    let n = values.len();
    let bits = n.trailing_zeros();
    let mut data: Vec<Complex> = (0..n)
        .map(|i| {
            let j = if bits == 0 { 0 } else { i.reverse_bits() >> (usize::BITS - bits) };
            Complex { re: values[j], im: 0.0 }
        })
        .collect();
    let mut span = 2;
    while span <= n {
        let half = span / 2;
        let step = -2.0 * std::f64::consts::PI / span as f64;
        for start in (0..n).step_by(span) {
            for k in 0..half {
                let twiddle = Complex { re: (step * k as f64).cos(), im: (step * k as f64).sin() };
                let t = twiddle.mul(data[start + k + half]);
                data[start + k + half] = data[start + k].sub(t);
                data[start + k] = data[start + k].add(t);
            }
        }
        span *= 2;
    }
    data
}

/// Band levels (0..1) for a WAV file that is still being written.
pub struct Meter {
    path: PathBuf,
    pub levels: Vec<f32>,
    /// The quietest recent level per band: room noise, subtracted from the display.
    floors: Vec<f32>,
    /// Byte offset of the next unread sample; `None` until the data chunk is found.
    offset: Option<u64>,
    tail: Vec<u8>,
    loud_at: Instant,
    window: Vec<f64>,
    bins: Vec<(usize, usize)>,
    full_scale: f64,
}

impl Meter {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        let edges = band_edges(BANDS, 80.0, 4000.0);
        let bins: Vec<usize> =
            edges.iter().map(|e| ((e * WINDOW as f64 / SAMPLE_RATE).round() as usize).max(1)).collect();
        Self {
            path: path.into(),
            levels: vec![0.0; BANDS],
            floors: vec![1.0; BANDS],
            offset: None,
            tail: Vec::new(),
            loud_at: Instant::now(),
            window: (0..WINDOW)
                .map(|i| 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / (WINDOW - 1) as f64).cos())
                .collect(),
            bins: bins.windows(2).map(|w| (w[0], w[1].max(w[0] + 1))).collect(),
            // Power in the main bin of a full-scale sine under a Hann window.
            full_scale: (32768.0 * WINDOW as f64 / 4.0).powi(2),
        }
    }

    /// Walks the RIFF chunks once to find where samples begin.
    fn locate_data(head: &[u8]) -> Option<u64> {
        if head.len() < 12 || &head[..4] != b"RIFF" || &head[8..12] != b"WAVE" {
            return None;
        }
        let mut position = 12;
        while position + 8 <= head.len() {
            let chunk = &head[position..position + 4];
            let size = u32::from_le_bytes(head[position + 4..position + 8].try_into().ok()?) as usize;
            if chunk == b"data" {
                return Some((position + 8) as u64);
            }
            position += 8 + size + (size & 1);
        }
        None
    }

    fn read_new(&mut self) -> Vec<u8> {
        let Ok(mut file) = File::open(&self.path) else {
            return Vec::new();
        };
        if self.offset.is_none() {
            let mut head = vec![0; HEADER_LIMIT];
            let read = file.read(&mut head).unwrap_or(0);
            self.offset = Self::locate_data(&head[..read]);
        }
        let Some(offset) = self.offset else {
            return Vec::new();
        };
        let mut data = Vec::new();
        if file.seek(SeekFrom::Start(offset)).is_ok() && file.read_to_end(&mut data).is_ok() {
            // Whole samples only; a half-written one is read next time.
            data.truncate(data.len() & !1);
            self.offset = Some(offset + data.len() as u64);
        }
        data
    }

    /// Folds in whatever the recorder wrote since the last call.
    pub fn update(&mut self) -> &[f32] {
        self.update_at(Instant::now())
    }

    pub fn update_at(&mut self, now: Instant) -> &[f32] {
        let data = self.read_new();
        if data.is_empty() {
            for level in &mut self.levels {
                *level *= DECAY;
            }
            return &self.levels;
        }
        let peak = data.as_chunks::<2>().0.iter().map(|b| i16::from_le_bytes(*b).unsigned_abs()).max().unwrap_or(0);
        if f32::from(peak) / 32768.0 > QUIET_PEAK {
            self.loud_at = now;
        }
        self.tail.extend_from_slice(&data);
        if self.tail.len() > WINDOW * 2 {
            self.tail.drain(..self.tail.len() - WINDOW * 2);
        }
        if self.tail.len() < WINDOW * 2 {
            return &self.levels;
        }
        let samples: Vec<f64> = self
            .tail
            .as_chunks::<2>()
            .0
            .iter()
            .zip(&self.window)
            .map(|(b, w)| f64::from(i16::from_le_bytes(*b)) * w)
            .collect();
        let spectrum = fft(&samples);
        for (index, (start, stop)) in self.bins.clone().into_iter().enumerate() {
            let power: f64 = spectrum[start..stop.min(spectrum.len())].iter().map(|c| c.norm_sqr()).sum();
            let decibels = if power > 0.0 { 10.0 * (power / self.full_scale).log10() } else { -120.0 };
            let raw = ((decibels - FLOOR_DB) / RANGE_DB).clamp(0.0, 1.0) as f32;
            // Room noise would otherwise keep every bar half lit: the floor
            // drops at once and creeps back up.
            let floor = (self.floors[index] + FLOOR_RISE).min(raw);
            self.floors[index] = floor;
            let level = if floor < 1.0 { (raw - floor) / (1.0 - floor) } else { 0.0 };
            self.levels[index] = if level >= self.levels[index] { level } else { self.levels[index] * DECAY };
        }
        &self.levels
    }

    /// Whether nothing audible arrived for a while: a muted or missing microphone.
    pub fn quiet(&self) -> bool {
        self.quiet_at(Instant::now())
    }

    pub fn quiet_at(&self, now: Instant) -> bool {
        now.duration_since(self.loud_at) > QUIET_AFTER
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// One cell per band: the block for its level, at least a sliver so the
/// meter always reads as a meter.
pub fn line(levels: &[f32]) -> String {
    levels.iter().map(|level| BLOCKS[((level.clamp(0.0, 1.0) * 8.0).round() as usize).max(1)]).collect()
}

/// 0 low, 1 mid, 2 hot: the color a bar takes.
pub fn shade(level: f32) -> u8 {
    if level > 0.85 {
        2
    } else if level > 0.6 {
        1
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn wav_header(data_len: u32) -> Vec<u8> {
        let mut h = Vec::new();
        h.extend_from_slice(b"RIFF");
        h.extend_from_slice(&(36 + data_len).to_le_bytes());
        h.extend_from_slice(b"WAVEfmt ");
        h.extend_from_slice(&16u32.to_le_bytes());
        h.extend_from_slice(&1u16.to_le_bytes());
        h.extend_from_slice(&1u16.to_le_bytes());
        h.extend_from_slice(&16_000u32.to_le_bytes());
        h.extend_from_slice(&32_000u32.to_le_bytes());
        h.extend_from_slice(&2u16.to_le_bytes());
        h.extend_from_slice(&16u16.to_le_bytes());
        h.extend_from_slice(b"data");
        h.extend_from_slice(&data_len.to_le_bytes());
        h
    }

    fn sine(frequency: f64, amplitude: f64, samples: usize) -> Vec<u8> {
        (0..samples)
            .flat_map(|i| {
                let v = (amplitude * 32767.0 * (2.0 * std::f64::consts::PI * frequency * i as f64 / SAMPLE_RATE).sin())
                    as i16;
                v.to_le_bytes()
            })
            .collect()
    }

    #[test]
    fn the_fft_finds_a_pure_tone_in_its_bin() {
        let n = 64;
        let tone: Vec<f64> = (0..n).map(|i| (2.0 * std::f64::consts::PI * 5.0 * i as f64 / n as f64).cos()).collect();
        let spectrum = fft(&tone);
        let peak = (0..n / 2).max_by(|a, b| spectrum[*a].norm_sqr().total_cmp(&spectrum[*b].norm_sqr())).unwrap();
        assert_eq!(peak, 5);
        assert!((spectrum[5].norm_sqr().sqrt() - n as f64 / 2.0).abs() < 1e-6);
    }

    #[test]
    fn band_edges_span_the_voice_range_logarithmically() {
        let edges = band_edges(12, 80.0, 4000.0);
        assert_eq!(edges.len(), 13);
        assert!((edges[0] - 80.0).abs() < 1e-9 && (edges[12] - 4000.0).abs() < 1e-6);
        let ratios: Vec<f64> = edges.windows(2).map(|w| w[1] / w[0]).collect();
        assert!(ratios.iter().all(|r| (r - ratios[0]).abs() < 1e-9), "constant ratio");
    }

    /// A recording that starts with room silence, as real ones do: the
    /// noise floor settles low before the voice arrives.
    fn recording_with_silence(dir: &Path) -> (PathBuf, File, Meter) {
        let path = dir.join("rec.wav");
        let mut file = File::create(&path).unwrap();
        file.write_all(&wav_header(0)).unwrap();
        file.write_all(&vec![0u8; WINDOW * 2]).unwrap();
        file.flush().unwrap();
        let mut meter = Meter::new(&path);
        meter.update();
        (path, file, meter)
    }

    #[test]
    fn a_steady_tone_from_the_first_sample_reads_as_room_noise() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rec.wav");
        let mut data = wav_header(0);
        data.extend(sine(500.0, 0.5, WINDOW * 2));
        std::fs::write(&path, data).unwrap();
        let mut meter = Meter::new(&path);
        assert!(meter.update().iter().all(|l| *l == 0.0), "a hum is not speech");
    }

    #[test]
    fn a_loud_tone_lights_its_own_band_most() {
        let dir = tempfile::tempdir().unwrap();
        let (_path, mut file, mut meter) = recording_with_silence(dir.path());
        let edges = band_edges(BANDS, 80.0, 4000.0);
        let center = (edges[6] * edges[7]).sqrt();
        file.write_all(&sine(center, 0.5, WINDOW * 2)).unwrap();
        file.flush().unwrap();
        let levels = meter.update().to_vec();
        let loudest = (0..BANDS).max_by(|a, b| levels[*a].total_cmp(&levels[*b])).unwrap();
        assert_eq!(loudest, 6, "{levels:?}");
        assert!(levels[6] > 0.5, "{levels:?}");
        assert!(!meter.quiet());
    }

    #[test]
    fn the_file_is_tailed_and_levels_decay_without_new_samples() {
        let dir = tempfile::tempdir().unwrap();
        let (_path, mut file, mut meter) = recording_with_silence(dir.path());
        file.write_all(&sine(500.0, 0.5, WINDOW * 2)).unwrap();
        file.flush().unwrap();
        let first = meter.update().to_vec();
        let peak: f32 = first.iter().copied().fold(0.0, f32::max);
        let after = meter.update().to_vec();
        let decayed: f32 = after.iter().copied().fold(0.0, f32::max);
        assert!((decayed - peak * DECAY).abs() < 1e-6, "no new data decays by DECAY");
        file.write_all(&sine(500.0, 0.5, WINDOW)).unwrap();
        file.flush().unwrap();
        let again: f32 = meter.update().iter().copied().fold(0.0, f32::max);
        assert!(again > decayed, "new samples lift the level back");
    }

    #[test]
    fn silence_is_noticed_after_three_seconds() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rec.wav");
        let mut file = File::create(&path).unwrap();
        file.write_all(&wav_header(0)).unwrap();
        file.write_all(&vec![0u8; WINDOW * 4]).unwrap();
        drop(file);
        let start = Instant::now();
        let mut meter = Meter::new(&path);
        meter.update_at(start);
        assert!(!meter.quiet_at(start + Duration::from_secs(2)));
        assert!(meter.quiet_at(start + Duration::from_secs(4)));
        assert!(meter.levels.iter().all(|l| *l == 0.0), "silence lights nothing");
    }

    #[test]
    fn a_header_that_is_still_being_written_waits() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rec.wav");
        std::fs::write(&path, b"RIFF\0\0").unwrap();
        let mut meter = Meter::new(&path);
        assert!(meter.update().iter().all(|l| *l == 0.0));
        let mut full = wav_header(0);
        full.extend(vec![0u8; WINDOW * 2]);
        std::fs::write(&path, &full).unwrap();
        meter.update();
        full.extend(sine(300.0, 0.5, WINDOW * 2));
        std::fs::write(&path, full).unwrap();
        assert!(meter.update().iter().any(|l| *l > 0.0), "found the data chunk on a later tick");
    }

    #[test]
    fn extra_chunks_before_data_are_skipped() {
        let mut head = b"RIFF\0\0\0\0WAVE".to_vec();
        head.extend_from_slice(b"LIST");
        head.extend_from_slice(&3u32.to_le_bytes());
        head.extend_from_slice(b"abc\0");
        head.extend_from_slice(b"data");
        head.extend_from_slice(&0u32.to_le_bytes());
        assert_eq!(Meter::locate_data(&head), Some(head.len() as u64), "odd sizes are padded");
        assert_eq!(Meter::locate_data(b"not a wav file"), None);
    }

    #[test]
    fn a_missing_file_reads_as_silence() {
        let mut meter = Meter::new("/nonexistent/rec.wav");
        assert_eq!(meter.update().len(), BANDS);
    }

    #[test]
    fn lines_and_shades() {
        assert_eq!(line(&[0.0, 0.5, 1.0, 2.0]), "▁▄██");
        assert_eq!((shade(0.5), shade(0.7), shade(0.9)), (0, 1, 2));
    }
}
