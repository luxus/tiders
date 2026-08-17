//! Same-process spectrum tap: lavfi `asplit` → speakers + `showfreqs`.
//!
//! A second `--ao=pcm` mpv cannot stay locked to the audible clock (untimed
//! dump, extra network decode, FIFO slack). Splitting inside the player that
//! owns the speakers keeps bars on `video-sync=audio`.

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Linear frequency bins in the `showfreqs` frame (0 Hz → Nyquist).
pub const VIS_WIDTH: u32 = 512;
/// Bar height in pixels; column fill / height is the bin level.
pub const VIS_HEIGHT: u32 = 40;

/// lavfi-complex graph: native audio to `[ao]`, resampled mono tap to bars.
pub fn lavfi_complex() -> String {
    format!(
        "[aid1]asplit[ao][tap];[tap]aresample=44100,aformat=channel_layouts=mono,showfreqs=mode=bar:fscale=lin:ascale=cbrt:win_size=2048:win_func=hanning:s={VIS_WIDTH}x{VIS_HEIGHT}:rate=30,format=rgb24[vo]"
    )
}

/// Decode one PNG frame into packed RGB8 (`width * height * 3`).
pub fn decode_rgb_png(bytes: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let decoder = png::Decoder::new(Cursor::new(bytes));
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    buf.truncate(info.buffer_size());
    let (w, h) = (info.width, info.height);
    match info.color_type {
        png::ColorType::Rgb => Some((w, h, buf)),
        png::ColorType::Rgba => {
            let mut rgb = Vec::with_capacity((w * h * 3) as usize);
            for px in buf.chunks_exact(4) {
                rgb.extend_from_slice(&px[..3]);
            }
            Some((w, h, rgb))
        }
        png::ColorType::Grayscale => {
            let mut rgb = Vec::with_capacity((w * h * 3) as usize);
            for g in buf {
                rgb.extend_from_slice(&[g, g, g]);
            }
            Some((w, h, rgb))
        }
        png::ColorType::GrayscaleAlpha => {
            let mut rgb = Vec::with_capacity((w * h * 3) as usize);
            for px in buf.chunks_exact(2) {
                rgb.extend_from_slice(&[px[0], px[0], px[0]]);
            }
            Some((w, h, rgb))
        }
        _ => None,
    }
}

/// Map an RGB8 `showfreqs` bar chart to `0..=1` levels, one per column.
///
/// Solid bars grow from the bottom. A two-pixel floor is subtracted so the
/// anti-aliased baseline does not look like audible energy in every bin.
pub fn column_levels(rgb: &[u8], width: usize, height: usize) -> Vec<f32> {
    if width == 0 || height == 0 || rgb.len() < width * height * 3 {
        return Vec::new();
    }
    const THRESH: u16 = 40;
    const FLOOR: usize = 2;
    let denom = height.saturating_sub(FLOOR).max(1) as f32;
    let mut out = vec![0.0f32; width];
    for (x, slot) in out.iter_mut().enumerate() {
        let mut fill = 0usize;
        for y in 0..height {
            let i = (y * width + x) * 3;
            let s = rgb[i] as u16 + rgb[i + 1] as u16 + rgb[i + 2] as u16;
            if s > THRESH {
                fill += 1;
            }
        }
        *slot = fill.saturating_sub(FLOOR) as f32 / denom;
    }
    out
}

pub fn spawn_frame_reader(
    dir: PathBuf,
    bins: Arc<Mutex<Vec<f32>>>,
    gen: Arc<AtomicU64>,
    mine: u64,
) {
    std::thread::Builder::new()
        .name("tiders-vis".into())
        .spawn(move || frame_loop(dir, bins, gen, mine))
        .ok();
}

fn frame_loop(dir: PathBuf, bins: Arc<Mutex<Vec<f32>>>, gen: Arc<AtomicU64>, mine: u64) {
    while gen.load(Ordering::Relaxed) == mine {
        if let Some((keep, rgb, w, h)) = latest_rgb_frame(&dir) {
            let levels = column_levels(&rgb, w as usize, h as usize);
            if !levels.is_empty() {
                if let Ok(mut guard) = bins.lock() {
                    *guard = levels;
                }
            }
            sweep_pngs(&dir, &keep);
        }
        std::thread::sleep(Duration::from_millis(8));
    }
}

/// Newest PNG that decodes. The file mpv is still writing usually fails.
fn latest_rgb_frame(dir: &Path) -> Option<(PathBuf, Vec<u8>, u32, u32)> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "png"))
        .collect();
    files.sort();
    for path in files.into_iter().rev() {
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        if let Some((w, h, rgb)) = decode_rgb_png(&bytes) {
            if w > 0 && h > 0 {
                return Some((path, rgb, w, h));
            }
        }
    }
    None
}

fn sweep_pngs(dir: &Path, keep: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let p = entry.path();
        if p != *keep && p.extension().is_some_and(|ext| ext == "png") {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// Drop leftover frames so a seek / new track cannot show the previous FFT.
pub fn clear_frames(dir: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let p = entry.path();
        if p.extension().is_some_and(|ext| ext == "png") {
            let _ = std::fs::remove_file(p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgb_bar_frame(width: usize, height: usize, peak: usize) -> Vec<u8> {
        let mut rgb = vec![0u8; width * height * 3];
        for x in 0..width {
            let fill = if x == peak {
                height
            } else if x.abs_diff(peak) == 1 {
                height / 3
            } else {
                2
            };
            for y in (height - fill)..height {
                let i = (y * width + x) * 3;
                rgb[i] = 255;
            }
        }
        rgb
    }

    #[test]
    fn column_levels_peak_at_expected_bin() {
        let w = 64;
        let h = 40;
        let peak = 10;
        let levels = column_levels(&rgb_bar_frame(w, h, peak), w, h);
        assert_eq!(levels.len(), w);
        let (idx, _) = levels
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap();
        assert_eq!(idx, peak, "levels={levels:?}");
        assert!(levels[peak] > 0.8, "peak height {}", levels[peak]);
        assert!(levels[0] < 0.1, "floor leaked: {}", levels[0]);
    }

    #[test]
    fn png_roundtrip_keeps_peak() {
        let w = 32u32;
        let h = 16u32;
        let peak = 7usize;
        let rgb = rgb_bar_frame(w as usize, h as usize, peak);
        let mut encoded = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut encoded, w, h);
            encoder.set_color(png::ColorType::Rgb);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().expect("hdr");
            writer.write_image_data(&rgb).expect("idat");
        }
        let (dw, dh, got) = decode_rgb_png(&encoded).expect("decode");
        assert_eq!((dw, dh), (w, h));
        let levels = column_levels(&got, w as usize, h as usize);
        let (idx, _) = levels
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap();
        assert_eq!(idx, peak);
    }

    #[test]
    fn lavfi_graph_splits_audio_to_showfreqs() {
        assert!(lavfi_complex().contains("asplit[ao][tap]"));
        assert!(lavfi_complex().contains("showfreqs="));
        assert!(lavfi_complex().contains("format=rgb24[vo]"));
        assert!(!lavfi_complex().contains("averaging=0"));
    }
}

#[cfg(all(test, unix))]
mod mpv_tap {
    use super::*;
    use std::process::{Command, Stdio};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn mpv_available() -> bool {
        Command::new("mpv").arg("--version").output().is_ok()
    }

    #[test]
    fn showfreqs_sine_peaks_near_440() {
        if !mpv_available() {
            return;
        }
        let dir = std::env::temp_dir().join(format!(
            "tiders-vis-itest-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("vis dir");
        let status = Command::new("mpv")
            .args([
                "--no-terminal",
                "--really-quiet",
                "--force-window=no",
                "--audio-display=no",
                "--osd-level=0",
                "--osc=no",
                "--hwdec=no",
                "--ao=null",
                "--ao-null-untimed=no",
                "--video-sync=audio",
                "--vo=image",
                "--vo-image-format=png",
                "--vo-image-png-compression=0",
            ])
            .arg(format!("--vo-image-outdir={}", dir.display()))
            .arg(format!("--lavfi-complex={}", lavfi_complex()))
            .arg("av://lavfi:sine=frequency=440:duration=0.9")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("spawn mpv");
        assert!(status.success(), "mpv vis tap exited {status}");
        let mut pngs: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|ext| ext == "png"))
            .collect();
        pngs.sort();
        assert!(pngs.len() >= 4, "expected paced frames, got {}", pngs.len());
        let bytes = std::fs::read(&pngs[pngs.len() / 2]).expect("read frame");
        let (w, h, rgb) = decode_rgb_png(&bytes).expect("decode frame");
        let levels = column_levels(&rgb, w as usize, h as usize);
        let (idx, &peak) = levels
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap();
        let expected = (440.0 / 22_050.0 * w as f32).round() as usize;
        assert!(
            idx.abs_diff(expected) <= 3,
            "440 Hz peak at {idx}, expected ~{expected}, peak={peak}, n={}",
            levels.len()
        );
        assert!(peak > 0.2, "peak too quiet: {peak}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
