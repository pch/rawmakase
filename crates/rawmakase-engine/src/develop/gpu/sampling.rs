//! The sampling pass's parameter header (`sp` in `local.wgsl`): its named slots,
//! their ranges for the Rust side, and the `S_*` offsets generated from them for
//! the shader, so the two sides cannot drift apart. The radial lens tables follow
//! the header, at [`HEADER`].
use std::ops::Range;

/// Declares each slot as a `Range` that starts where the previous one ends, the
/// header's length as `HEADER`, and every slot by name as `ALL`.
macro_rules! slots {
    ($($name:ident: $len:expr),* $(,)?) => {
        slots!(@at 0; $($name: $len,)*);
        /// Every slot, in order, by the name the shader gives it.
        pub(crate) const ALL: &[(&str, Range<usize>)] = &[$((stringify!($name), $name)),*];
    };
    (@at $at:expr;) => {
        /// Parameters before the radial tables.
        pub(crate) const HEADER: usize = $at;
    };
    (@at $at:expr; $name:ident: $len:expr, $($rest:tt)*) => {
        pub(crate) const $name: Range<usize> = $at..$at + $len;
        slots!(@at $name.end; $($rest)*);
    };
}

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
    // Luma, chroma, luma detail, chroma detail, luma contrast, chroma smoothness.
    NOISE: 6,
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
    // Reduced width, height.
    REDUCED: 2,
    // Workgroups per row of the dispatch.
    COUNT: 1,
    // `ManualDistortion`: k (0 when off) and the frame's axes.
    MANUAL: 3,
}

/// `S_*` offset constants for `local.wgsl`.
pub(crate) fn wgsl_prelude() -> String {
    ALL.iter()
        .map(|(name, range)| format!("const S_{name}: u32 = {}u;\n", range.start))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The offsets the shader used when they were written out by hand, less the
    /// local-tone gain's, which is no longer sampled.
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
                ("NOISE", 35),
                ("LENS", 41),
                ("CENTER", 42),
                ("HALF", 44),
                ("FILL", 45),
                ("AMOUNT", 46),
                ("DISTORTION", 47),
                ("RED", 49),
                ("BLUE", 51),
                ("VIGNETTING", 53),
                ("VIGNETTING_AMOUNT", 55),
                ("REDUCED", 56),
                ("COUNT", 58),
                ("MANUAL", 59),
            ]
        );
        assert_eq!(HEADER, 62);
    }
}
