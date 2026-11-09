//! The sampling pass's parameter header (`sp` in `local.wgsl`): its named slots,
//! their ranges for the Rust side, and the `S_*` offsets generated from them for
//! the shader, so the two sides cannot drift apart. The radial lens tables follow
//! the header, at [`HEADER`].
use crate::develop::slots::slots;

slots! {
    WIDTH: 1,
    HEIGHT: 1,
    // Output width, height.
    OUT: 2,
    // x, y, width, height.
    REGION: 4,
    SPREAD: 1,
    CROP: 4,
    ORIENTED: 2,
    ZOOM: 1,
    SIN: 1,
    COS: 1,
    TURNS: 1,
    FLIP: 2,
    INSET: 4,
    TRANSFORM: 1,
    HOMOGRAPHY: 9,
    LENS: 1,
    CENTER: 2,
    HALF: 1,
    FILL: 1,
    AMOUNT: 1,
    // Offset (or -1) and length of each radial table.
    DISTORTION: 2,
    RED: 2,
    BLUE: 2,
    VIGNETTING: 2,
    VIGNETTING_AMOUNT: 1,
    // Offset (or -1) of the GainMap table (`gpu_params::lens_gpu_params`).
    GAIN_MAP: 1,
    // Workgroups per row of the dispatch.
    COUNT: 1,
    // `ManualDistortion`: k (0 when off) and the frame's axes.
    MANUAL: 3,
    // Protect reconstructed X-Trans stripe colour during CA remapping.
    XTRANS: 1,
    // The transparency mask's width and height (`Metadata::coverage`), 0 without one.
    COVERAGE: 2,
}

/// `S_*` offset constants for `local.wgsl`, and `RADIAL_STEPS`, the halvings its
/// table search needs to reach any knot of the largest table `optics::Radial` allows.
pub(crate) fn wgsl_prelude() -> String {
    ALL.iter()
        .map(|(name, range)| format!("const S_{name}: u32 = {}u;\n", range.start))
        .chain([format!("const RADIAL_STEPS: u32 = {}u;\n", radial_steps())])
        .collect()
}

/// The halvings of `local.wgsl`'s `table_eval` that narrow a window of
/// `optics::Radial::MAX_KNOTS` knots to none.
pub(crate) fn radial_steps() -> u32 {
    (crate::optics::Radial::MAX_KNOTS + 1)
        .next_power_of_two()
        .trailing_zeros()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The offsets the shader used when they were written out by hand, less the
    /// local-tone gain's, which is no longer sampled, and the reduced size's, which
    /// nothing reduces on the device any more, and noise reduction, now prepared
    /// on the full camera image.
    #[test]
    fn slots_keep_the_offsets_the_shader_was_written_with() {
        let offsets: Vec<(&str, usize)> = ALL.iter().map(|(n, r)| (*n, r.start)).collect();
        assert_eq!(
            offsets,
            [
                ("WIDTH", 0),
                ("HEIGHT", 1),
                ("OUT", 2),
                ("REGION", 4),
                ("SPREAD", 8),
                ("CROP", 9),
                ("ORIENTED", 13),
                ("ZOOM", 15),
                ("SIN", 16),
                ("COS", 17),
                ("TURNS", 18),
                ("FLIP", 19),
                ("INSET", 21),
                ("TRANSFORM", 25),
                ("HOMOGRAPHY", 26),
                ("LENS", 35),
                ("CENTER", 36),
                ("HALF", 38),
                ("FILL", 39),
                ("AMOUNT", 40),
                ("DISTORTION", 41),
                ("RED", 43),
                ("BLUE", 45),
                ("VIGNETTING", 47),
                ("VIGNETTING_AMOUNT", 49),
                ("GAIN_MAP", 50),
                ("COUNT", 51),
                ("MANUAL", 52),
                ("XTRANS", 55),
                ("COVERAGE", 56),
            ]
        );
        assert_eq!(HEADER, 58);
    }
}
