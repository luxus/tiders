//! Real FFT spectrum analyser (cava-style gravity + peak hold).
//!
//! Incoming PCM **or** precomputed linear bins (from the playback vis tap) are
//! windowed / folded into log-spaced bands from ~20 Hz to Nyquist. Bars only
//! rise on **fresh** energy; silence, pause, or a stale tap lets cava-style
//! gravity pull them down instead of inventing a synth fallback.

use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};
use std::f32::consts::PI;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Number of FFT bins (power of two). 2048 @ 44.1 kHz ≈ 23 ms of audio.
pub const FFT_SIZE: usize = 2048;
/// Default log-spaced bar count shown in the TUI.
pub const DEFAULT_BARS: usize = 48;
/// Assumed sample rate of the PCM tap / synth buffer.
pub const SAMPLE_RATE: f32 = 44_100.0;

const GRAVITY: f32 = 28.0;
const PEAK_HOLD: f32 = 0.16;
const PEAK_FALL: f32 = 2.2;
const SMOOTH: f32 = 1.35;
/// RMS below this is treated as silence so leftover FFT windows don't freeze.
const SILENCE_RMS: f32 = 0.012;
/// Drop the last PCM window if the tap goes quiet for this long.
const PCM_STALE: Duration = Duration::from_millis(180);

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
            EqTheme::Mono => [
                (55, 55, 55),
                (105, 105, 105),
                (160, 160, 160),
                (215, 215, 215),
            ],
            EqTheme::Neon => [
                (150, 0, 195),
                (215, 0, 175),
                (250, 75, 195),
                (250, 195, 235),
            ],
            EqTheme::Tide => [
                (13, 90, 110),
                (45, 212, 191),
                (125, 207, 255),
                (158, 206, 106),
            ],
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
    /// True once real PCM has been pushed.
    has_pcm: bool,
    /// True when [`Self::mags`] was filled by [`Self::feed_mags`] (skip FFT).
    mags_direct: bool,
    last_pcm: Option<Instant>,
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
            mags_direct: false,
            last_pcm: None,
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

    /// Reset analyser state when the playing track changes.
    pub fn set_seed(&mut self, _seed: u64) {
        self.has_pcm = false;
        self.mags_direct = false;
        self.last_pcm = None;
        self.pcm_len = 0;
        self.pcm.fill(0.0);
        self.levels.fill(0.0);
        self.bars.fill(0.0);
        self.peaks.fill(0.0);
        self.vel.fill(0.0);
        self.peak_age.fill(0.0);
    }

    /// Push interleaved-or-mono `f32` samples in `-1.0..=1.0`.
    ///
    /// Keeps the last [`FFT_SIZE`] samples with a single copy. The previous
    /// per-sample `copy_within` was O(n²) and froze the analyser a few seconds
    /// into a track once the PCM tap filled up.
    pub fn feed(&mut self, samples: &[f32]) {
        if samples.is_empty() {
            return;
        }
        self.has_pcm = true;
        self.mags_direct = false;
        self.last_pcm = Some(Instant::now());
        if samples.len() >= FFT_SIZE {
            self.pcm
                .copy_from_slice(&samples[samples.len() - FFT_SIZE..]);
            self.pcm_len = FFT_SIZE;
            return;
        }
        let n = samples.len();
        if self.pcm_len + n <= FFT_SIZE {
            self.pcm[self.pcm_len..self.pcm_len + n].copy_from_slice(samples);
            self.pcm_len += n;
            return;
        }
        let keep = FFT_SIZE - n;
        if self.pcm_len > keep {
            self.pcm.copy_within(self.pcm_len - keep..self.pcm_len, 0);
        }
        self.pcm[keep..FFT_SIZE].copy_from_slice(samples);
        self.pcm_len = FFT_SIZE;
    }

    /// Push linear frequency-bin levels already in `0..=1` (from the playback
    /// vis tap). Skips the FFT so bars stay on the same clock as the speakers.
    pub fn feed_mags(&mut self, mags: &[f32]) {
        if mags.is_empty() {
            return;
        }
        if self.mags.len() != mags.len() {
            self.mags.resize(mags.len(), 0.0);
        }
        self.mags.copy_from_slice(mags);
        self.has_pcm = true;
        self.mags_direct = true;
        self.last_pcm = Some(Instant::now());
        self.pcm_len = FFT_SIZE;
    }

    /// Advance by `dt` seconds. Bars only pick up energy from a fresh PCM
    /// window with audible RMS; otherwise gravity falls to zero.
    pub fn tick(
        &mut self,
        dt: f32,
        playing: bool,
        volume: f32,
        _bpm: f32,
        _position: f64,
    ) -> SpectrumFrame {
        let pcm_fresh = self.has_pcm && self.last_pcm.is_some_and(|t| t.elapsed() < PCM_STALE);
        let energy = if self.mags_direct {
            self.mags.iter().copied().fold(0.0f32, f32::max) > 0.04
        } else {
            self.pcm_rms() >= SILENCE_RMS
        };
        let audible = playing && volume > 0.01 && pcm_fresh && energy;
        if audible {
            if !self.mags_direct {
                self.transform();
            }
            self.fold_bands();
        } else {
            self.levels.fill(0.0);
            if !pcm_fresh {
                self.pcm.fill(0.0);
                self.pcm_len = 0;
                self.mags_direct = false;
            }
        }
        self.apply_gravity(dt, audible);
        SpectrumFrame {
            bars: self.bars.clone(),
            peaks: self.peaks.clone(),
        }
    }

    fn pcm_rms(&self) -> f32 {
        if self.pcm_len == 0 {
            return 0.0;
        }
        let n = self.pcm_len.min(self.pcm.len());
        let sum: f32 = self.pcm[..n].iter().map(|s| s * s).sum();
        (sum / n as f32).sqrt()
    }

    /// Last computed frame, without advancing time.
    pub fn frame(&self) -> SpectrumFrame {
        SpectrumFrame {
            bars: self.bars.clone(),
            peaks: self.peaks.clone(),
        }
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

/// Render `height` rows of block characters for `frame`, top-down.
/// Each cell is `(glyph, t)` where `t` is 0 at the bottom of the bar.
pub fn render_rows(frame: &SpectrumFrame, height: u16) -> Vec<Vec<(char, f32)>> {
    const GLYPHS: [char; 9] = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let h = height.max(1) as usize;
    let mut rows = vec![vec![(' ', 0.0); frame.bars.len()]; h];
    for (x, (&bar, &peak)) in frame.bars.iter().zip(frame.peaks.iter()).enumerate() {
        let filled = bar * h as f32;
        for (y, row) in rows.iter_mut().enumerate() {
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
            row[x] = (glyph, t);
        }
        // Peak cap: a `•` on the row matching the peak.
        let peak_row = h
            .saturating_sub(1)
            .saturating_sub((peak * h as f32).floor() as usize);
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
            (4..24).contains(&idx),
            "440 Hz peak at unexpected band {idx}: {:?}",
            frame.bars
        );
        assert!(
            frame.bars[idx] > 0.15,
            "peak too quiet: {}",
            frame.bars[idx]
        );
    }

    #[test]
    fn feed_keeps_last_window() {
        let mut spec = Spectrum::new(16);
        let mut samples = vec![0.0; FFT_SIZE * 3];
        let n = samples.len();
        samples[n - 1] = 1.0;
        samples[n - 2] = 0.5;
        spec.feed(&samples);
        assert_eq!(spec.pcm_len, FFT_SIZE);
        assert_eq!(spec.pcm[FFT_SIZE - 1], 1.0);
        assert_eq!(spec.pcm[FFT_SIZE - 2], 0.5);
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
            spec.pcm_len = FFT_SIZE;
            let _ = spec.tick(0.016, true, 1.0, 120.0, 0.0);
        }
        let after: f32 = spec.bars.iter().sum();
        assert!(after < before, "bars did not fall ({after} vs {before})");
        assert!(after < 0.5, "silence should collapse bars, got {after}");
    }

    #[test]
    fn stale_pcm_does_not_hold_the_last_frame() {
        let mut spec = Spectrum::new(16);
        spec.feed(&[0.8; FFT_SIZE]);
        let _ = spec.tick(0.008, true, 1.0, 120.0, 0.0);
        let before: f32 = spec.bars.iter().sum();
        assert!(before > 0.5, "expected energy from loud PCM, got {before}");
        spec.last_pcm = Some(Instant::now() - Duration::from_millis(500));
        for _ in 0..50 {
            let _ = spec.tick(0.016, true, 1.0, 120.0, 1.0);
        }
        let after: f32 = spec.bars.iter().sum();
        assert!(
            after < before * 0.2,
            "stale tap should decay, before={before} after={after}"
        );
    }

    #[test]
    fn zero_volume_collapses_bars() {
        let mut spec = Spectrum::new(16);
        spec.feed(&[0.9; FFT_SIZE]);
        let _ = spec.tick(0.008, true, 1.0, 120.0, 0.0);
        let before: f32 = spec.bars.iter().sum();
        for _ in 0..40 {
            spec.feed(&[0.9; 64]);
            let _ = spec.tick(0.016, true, 0.0, 120.0, 0.0);
        }
        let after: f32 = spec.bars.iter().sum();
        assert!(
            after < before,
            "volume 0 should drop bars ({after} vs {before})"
        );
    }

    #[test]
    fn feed_mags_440_peaks_near_a4() {
        let mut spec = Spectrum::new(32);
        let n = 512;
        let mut mags = vec![0.0f32; n];
        // 440 Hz at 44.1 kHz Nyquist mapping: 440 / 22050 * 512 ≈ 10.2
        let bin = (440.0 / 22_050.0 * n as f32).round() as usize;
        mags[bin] = 1.0;
        mags[bin.saturating_sub(1)] = 0.45;
        mags[(bin + 1).min(n - 1)] = 0.45;
        spec.feed_mags(&mags);
        let frame = spec.tick(1.0 / 120.0, true, 1.0, 120.0, 1.0);
        let (idx, _) = frame
            .bars
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap();
        assert!(
            (4..24).contains(&idx),
            "440 Hz mag peak at unexpected band {idx}: {:?}",
            frame.bars
        );
        assert!(frame.bars[idx] > 0.15, "peak too quiet: {}", frame.bars[idx]);
    }
}
