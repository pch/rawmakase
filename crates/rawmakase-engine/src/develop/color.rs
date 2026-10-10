//! The colour wheels' hues, which the masks' tint shares with the Color Grading panel.

/// The fully saturated RGB of a colour wheel's `hue` (0–1, red at 0).
pub fn hue_rgb(hue: f32) -> [f32; 3] {
    let h = hue.rem_euclid(1.) * 6.;
    let x = 1. - (h.rem_euclid(2.) - 1.).abs();
    match h as u32 {
        0 => [1., x, 0.],
        1 => [x, 1., 0.],
        2 => [0., 1., x],
        3 => [0., x, 1.],
        4 => [x, 0., 1.],
        _ => [1., 0., x],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wheel_matches_rgb_hues_and_wraps() {
        assert_eq!(hue_rgb(0.), [1., 0., 0.]);
        assert_eq!(hue_rgb(1. / 3.), [0., 1., 0.]);
        assert_eq!(hue_rgb(2. / 3.), [0., 0., 1.]);
        assert_eq!(hue_rgb(1.), hue_rgb(0.));
    }
}
