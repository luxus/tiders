//! Real FFT spectrum analyser (cava-style gravity + peak hold).
//!
//! Incoming PCM is windowed (Hann), transformed with [`rustfft`], then folded
//! into log-spaced bands from ~20 Hz to Nyquist. When no PCM has been fed yet
//! the analyser synthesises a short buffer driven by playback time / BPM /
//! volume and runs the **same** FFT path — so the visualiser is never a
//! hand-drawn sine of bar indices.

use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};
use std::f32::consts::PI;
use std::sync::Arc;

/// Number of FFT bins (power of two). 2048 @ 44.1 kHz ≈ 23 ms of audio.
pub const FFT_SIZE: usize = 2048;
/// Default log-spaced bar count shown in the TUI.
pub const DEFAULT_BARS: usize = 48;
/// Assumed sample rate of the PCM tap / synth buffer.
pub const SAMPLE_RATE: f32 = 44_100.0;

const GRAVITY: f32 = 18.0;
const PEAK_HOLD: f32 = 0.22;
const PEAK_FALL: f32 = 1.4;
const SMOOTH: f32 = 1.35;

/// Colour theme for the analyser bars (bottom → top gradient stops).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EqTheme {
    Classic,
    Fire,
    Ice,
    #[default]
    Mono,
    Neon,
    Tide,
}

impl EqTheme {
    pub fn label(self) -> &'static str {
        match self {
            EqTheme::Classic => "classic",
            EqTheme::Fire => "fire",
            EqTheme::Ice => "ice",
            EqTheme::Mono => "mono",
            EqTheme::Neon => "neon",
            EqTheme::Tide => "tide",
        }
    }

    pub fn cycle(self) -> Self {
        match self {
            EqTheme::Tide => EqTheme::Classic,
            EqTheme::Classic => EqTheme::Fire,
            EqTheme::Fire => EqTheme::Ice,
            EqTheme::Ice => EqTheme::Mono,
            EqTheme::Mono => EqTheme::Neon,
            EqTheme::Neon => EqTheme::Tide,
        }
    }

    /// Four RGB stops, bottom → top.
    pub fn stops(self) -> [(u8, u8, u8); 4] {
        match self {
            EqTheme::Classic => [(0, 130, 0), (0, 210, 0), (210, 210, 0), (210, 55, 55)],
            EqTheme::Fire => [(170, 35, 0), (215, 95, 0), (230, 175, 0), (250, 235, 90)],
            EqTheme::Ice => [(0, 55, 150), (0, 135, 215), (55, 205, 225), (195, 235, 255)],
            EqTheme::Mono => [(55, 55, 55), (105, 105, 105), (160, 160, 160), (215, 215, 215)],
            EqTheme::Neon => [(150, 0, 195), (215, 0, 175), (250, 75, 195), (250, 195, 235)],
            EqTheme::Tide => [(13, 90, 110), (45, 212, 191), (125, 207, 255), (158, 206, 106)],
        }
    }
}

impl std::str::FromStr for EqTheme {
    type Err = crate::error::Error;
    fn from_str(s: &str) -> crate::error::Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "classic" => Ok(EqTheme::Classic),
            "fire" => Ok(EqTheme::Fire),
            "ice" => Ok(EqTheme::Ice),
            "mono" => Ok(EqTheme::Mono),
            "neon" => Ok(EqTheme::Neon),
            "tide" => Ok(EqTheme::Tide),
            other => Err(crate::error::Error::other(format!(
                "unknown eq theme: {other}"
            ))),
        }
    }
}

/// One analyser frame: bar heights and peak-hold caps in `0.0..=1.0`.
#[derive(Debug, Clone)]
pub struct SpectrumFrame {
    pub bars: Vec<f32>,
    pub peaks: Vec<f32>,
}

/// FFT spectrum analyser with cava-style gravity.
pub struct Spectrum {
    fft: Arc<dyn Fft<f32>>,
    scratch: Vec<Complex<f32>>,
    /// Reused FFT input/output so `tick` does not allocate at 120 Hz.
    fft_buf: Vec<Complex<f32>>,
    window: Vec<f32>,
    pcm: Vec<f32>,
    pcm_len: usize,
    mags: Vec<f32>,
    /// Instantaneous FFT band energy (target for gravity).
    levels: Vec<f32>,
    /// Scratch for the horizontal smear pass.
    smear: Vec<f32>,
    bars: Vec<f32>,
    peaks: Vec<f32>,
    peak_age: Vec<f32>,
    vel: Vec<f32>,
    n_bars: usize,
    /// True once real PCM has been pushed (synth fallback is skipped).
    has_pcm: bool,
    seed: u64,
}

impl Spectrum {
    pub fn new(n_bars: usize) -> Self {
        let n_bars = n_bars.max(8);
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(FFT_SIZE);
        let scratch = vec![Complex::new(0.0, 0.0); fft.get_inplace_scratch_len().max(FFT_SIZE)];
        let window: Vec<f32> = (0..FFT_SIZE)
            .map(|i| 0.5 - 0.5 * (2.0 * PI * i as f32 / (FFT_SIZE as f32 - 1.0)).cos())
            .collect();
        Self {
            fft,
            scratch,
            fft_buf: vec![Complex::new(0.0, 0.0); FFT_SIZE],
            window,
            pcm: vec![0.0; FFT_SIZE],
            pcm_len: 0,
            mags: vec![0.0; FFT_SIZE / 2],
            levels: vec![0.0; n_bars],
            smear: vec![0.0; n_bars],
            bars: vec![0.0; n_bars],
            peaks: vec![0.0; n_bars],
            peak_age: vec![0.0; n_bars],
            vel: vec![0.0; n_bars],
            n_bars,
            has_pcm: false,
            seed: 0xC0FFEE,
        }
    }

    pub fn set_bars(&mut self, n_bars: usize) {
        let n_bars = n_bars.max(8);
        if n_bars == self.n_bars {
            return;
        }
        self.n_bars = n_bars;
        self.levels.resize(n_bars, 0.0);
        self.smear.resize(n_bars, 0.0);
        self.bars.resize(n_bars, 0.0);
        self.peaks.resize(n_bars, 0.0);
        self.peak_age.resize(n_bars, 0.0);
        self.vel.resize(n_bars, 0.0);
    }

    /// Seed the synth fallback so different tracks look distinct.
    pub fn set_seed(&mut self, seed: u64) {
        self.seed = seed.max(1);
        self.has_pcm = false;
        self.pcm_len = 0;
    }

    /// Push interleaved-or-mono `f32` samples in `-1.0..=1.0`.
    pub fn feed(&mut self, samples: &[f32]) {
        if samples.is_empty() {
            return;
        }
        self.has_pcm = true;
        for &s in samples {
            if self.pcm_len < FFT_SIZE {
                self.pcm[self.pcm_len] = s;
                self.pcm_len += 1;
            } else {
                self.pcm.copy_within(1.., 0);
                self.pcm[FFT_SIZE - 1] = s;
            }
        }
    }

    /// Advance by `dt` seconds. `playing` / `volume` / `bpm` / `position` drive
    /// the synth fallback when no PCM has been fed.
    pub fn tick(
        &mut self,
        dt: f32,
        playing: bool,
        volume: f32,
        bpm: f32,
        position: f64,
    ) -> SpectrumFrame {
        if playing {
            if !self.has_pcm {
                self.synthesize(position, bpm.max(60.0), volume.clamp(0.0, 1.0));
            }
            self.transform();
            self.fold_bands();
        }
        self.apply_gravity(dt, playing);
        SpectrumFrame {
            bars: self.bars.clone(),
            peaks: self.peaks.clone(),
        }
    }

    /// Last computed frame, without advancing time.
    pub fn frame(&self) -> SpectrumFrame {
        SpectrumFrame {
            bars: self.bars.clone(),
            peaks: self.peaks.clone(),
        }
    }

    fn synthesize(&mut self, position: f64, bpm: f32, volume: f32) {
        let t0 = position as f32;
        let beat = (bpm / 60.0).max(0.5);
        let kick = {
            let phase = (t0 * beat).fract();
            (-phase * 18.0).exp()
        };
        for i in 0..FFT_SIZE {
            let t = t0 + i as f32 / SAMPLE_RATE;
            let mut s = 0.0f32;
            // A handful of inharmonic partials unique to the track seed.
            for p in 0..7u32 {
                let f = 55.0 * (p + 1) as f32 * (1.0 + ((self.seed >> p) & 7) as f32 * 0.07);
                let env = 0.35 / (p + 1) as f32;
                s += env * (2.0 * PI * f * t).sin();
            }
            // Filtered noise (hash) for high-band shimmer.
            let n = hash01(self.seed.wrapping_add(i as u64).wrapping_add((t0 * 100.0) as u64));
            s += (n - 0.5) * 0.18;
            // Kick thump.
            s += kick * (2.0 * PI * 55.0 * t).sin() * 0.7;
            // Hi-hat-ish on off-beats.
            let hat = {
                let phase = (t * beat * 2.0).fract();
                if phase < 0.06 {
                    (n - 0.5) * (1.0 - phase / 0.06)
                } else {
                    0.0
                }
            };
            s += hat * 0.45;
            self.pcm[i] = (s * volume * 0.55).clamp(-1.0, 1.0);
        }
        self.pcm_len = FFT_SIZE;
    }

    fn transform(&mut self) {
        if self.pcm_len < FFT_SIZE / 4 {
            return;
        }
        for i in 0..FFT_SIZE {
            self.fft_buf[i] = Complex::new(self.pcm[i] * self.window[i], 0.0);
        }
        self.fft
            .process_with_scratch(&mut self.fft_buf, &mut self.scratch);
        let scale = 2.0 / FFT_SIZE as f32;
        for i in 0..self.mags.len() {
            let re = self.fft_buf[i].re * scale;
            let im = self.fft_buf[i].im * scale;
            // Magnitude → dB-ish with a floor, then 0..1.
            let mag = (re * re + im * im).sqrt();
            let db = 20.0 * (mag + 1e-9).log10();
            self.mags[i] = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
        }
    }

    fn fold_bands(&mut self) {
        let nyquist = SAMPLE_RATE / 2.0;
        let f_min = 20.0f32;
        let f_max = nyquist.min(20_000.0);
        let n = self.n_bars as f32;
        for b in 0..self.n_bars {
            let t0 = b as f32 / n;
            let t1 = (b + 1) as f32 / n;
            let lo = f_min * (f_max / f_min).powf(t0);
            let hi = f_min * (f_max / f_min).powf(t1);
            let i0 = ((lo / nyquist) * self.mags.len() as f32).floor() as usize;
            let i1 = ((hi / nyquist) * self.mags.len() as f32).ceil() as usize;
            let i0 = i0.min(self.mags.len().saturating_sub(1));
            let i1 = i1.max(i0 + 1).min(self.mags.len());
            let mut acc = 0.0f32;
            let mut wsum = 0.0f32;
            for (k, mag) in self.mags[i0..i1].iter().enumerate() {
                // Slight high-frequency lift so hats read.
                let lift = 1.0 + (i0 + k) as f32 / self.mags.len() as f32;
                acc += mag * lift;
                wsum += lift;
            }
            let v = if wsum > 0.0 { acc / wsum } else { 0.0 };
            self.levels[b] = v.clamp(0.0, 1.0);
        }
        // Monstercat-style horizontal smear.
        if self.n_bars > 2 {
            for i in 0..self.n_bars {
                let mut v = self.levels[i];
                if i > 0 {
                    v = v.max(self.levels[i - 1] / SMOOTH);
                }
                if i + 1 < self.n_bars {
                    v = v.max(self.levels[i + 1] / SMOOTH);
                }
                self.smear[i] = v;
            }
            self.levels.copy_from_slice(&self.smear);
        }
    }

    fn apply_gravity(&mut self, dt: f32, playing: bool) {
        let dt = dt.clamp(0.0, 0.05);
        for i in 0..self.n_bars {
            let target = if playing { self.levels[i] } else { 0.0 };
            if target >= self.bars[i] {
                self.bars[i] = target;
                self.vel[i] = 0.0;
            } else {
                self.vel[i] += GRAVITY * dt;
                self.bars[i] = (self.bars[i] - self.vel[i] * dt).max(target).max(0.0);
            }
            if self.bars[i] >= self.peaks[i] {
                self.peaks[i] = self.bars[i];
                self.peak_age[i] = 0.0;
            } else {
                self.peak_age[i] += dt;
                if self.peak_age[i] > PEAK_HOLD {
                    self.peaks[i] = (self.peaks[i] - PEAK_FALL * dt).max(self.bars[i]);
                }
            }
        }
    }
}

fn hash01(x: u64) -> f32 {
    let mut z = x.wrapping_add(0x9E3779B97F4A7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
    z ^= z >> 31;
    (z as f32) / (u64::MAX as f32)
}

/// Render `height` rows of block characters for `frame`, top-down.
/// Each cell is `(glyph, t)` where `t` is 0 at the bottom of the bar.
pub fn render_rows(frame: &SpectrumFrame, height: u16) -> Vec<Vec<(char, f32)>> {
    const GLYPHS: [char; 9] = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let h = height.max(1) as usize;
    let mut rows = vec![vec![(' ', 0.0); frame.bars.len()]; h];
    for (x, (&bar, &peak)) in frame.bars.iter().zip(frame.peaks.iter()).enumerate() {
        let filled = bar * h as f32;
        for y in 0..h {
            // y=0 is the top row.
            let from_bottom = (h - 1 - y) as f32;
            let cell = filled - from_bottom;
            let glyph = if cell >= 1.0 {
                GLYPHS[8]
            } else if cell > 0.0 {
                GLYPHS[(cell * 8.0).round() as usize]
            } else {
                GLYPHS[0]
            };
            let t = ((from_bottom + 0.5) / h as f32).clamp(0.0, 1.0);
            rows[y][x] = (glyph, t);
        }
        // Peak cap: a `•` on the row matching the peak.
        let peak_row = h.saturating_sub(1).saturating_sub((peak * h as f32).floor() as usize);
        if peak > 0.02 && peak_row < h && rows[peak_row][x].0 == ' ' {
            rows[peak_row][x] = ('•', (1.0 - peak_row as f32 / h as f32).clamp(0.0, 1.0));
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sine_440_peaks_near_a4() {
        let mut spec = Spectrum::new(32);
        let freq = 440.0f32;
        let mut samples = Vec::with_capacity(FFT_SIZE);
        for i in 0..FFT_SIZE {
            let t = i as f32 / SAMPLE_RATE;
            samples.push((2.0 * PI * freq * t).sin());
        }
        spec.feed(&samples);
        let frame = spec.tick(1.0 / 120.0, true, 1.0, 120.0, 1.0);
        // 440 Hz sits in the lower-mid log bands, not the first (bass) or last (air).
        let (idx, _) = frame
            .bars
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap();
        assert!(
            idx >= 4 && idx < 24,
            "440 Hz peak at unexpected band {idx}: {:?}",
            frame.bars
        );
        assert!(frame.bars[idx] > 0.15, "peak too quiet: {}", frame.bars[idx]);
    }

    #[test]
    fn gravity_falls_when_silent() {
        let mut spec = Spectrum::new(16);
        spec.feed(&[1.0; FFT_SIZE]);
        let _ = spec.tick(0.008, true, 1.0, 120.0, 0.0);
        let before: f32 = spec.bars.iter().sum();
        for _ in 0..40 {
            spec.has_pcm = true;
            // No new energy.
            spec.pcm.fill(0.0);
            let _ = spec.tick(0.016, false, 0.0, 120.0, 0.0);
        }
        let after: f32 = spec.bars.iter().sum();
        assert!(after < before, "bars did not fall ({after} vs {before})");
    }
}
