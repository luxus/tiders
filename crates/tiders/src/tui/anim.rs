//! Time-based animation helpers. Values are driven by wall-clock seconds so
//! the UI stays smooth at 120 Hz even if a frame is dropped.

/// Cubic ease-out: fast start, gentle landing. `t` in `0..=1`.
pub fn ease_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// Quintic ease-out — slightly snappier than cubic, used for popups.
pub fn ease_out_quint(t: f32) -> f32 {
    let t = 1.0 - t.clamp(0.0, 1.0);
    1.0 - t * t * t * t * t
}

pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t.clamp(0.0, 1.0)
}

/// Approach `target` from `current` with a critically-damped spring feel.
pub fn damp(current: f32, target: f32, dt: f32, speed: f32) -> f32 {
    let k = 1.0 - (-speed * dt).exp();
    current + (target - current) * k.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ease_bounds() {
        assert_eq!(ease_out_cubic(0.0), 0.0);
        assert_eq!(ease_out_cubic(1.0), 1.0);
        assert!(ease_out_cubic(0.5) > 0.5);
    }
}
