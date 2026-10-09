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
    // 1 when the local-tone gains apply.
    GAIN: 1,
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
    // Exposure, Clarity, Texture, texture blur.
    SLIDERS: 4,
    // x, y, width, height of the pixels `gains` holds.
    BOX: 4,
    // Workgroups per row of `region_gain`.
    BOX_COUNT: 2,
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
    /// local gain's Shadows and Highlights, which render per pixel.
    #[test]
    fn slots_keep_the_offsets_the_shader_was_written_with() {
        let offsets: Vec<(&str, usize)> = ALL.iter().map(|(n, r)| (*n, r.start)).collect();
        assert_eq!(
            offsets,
            [
                ("WIDTH", 0),
                ("HEIGHT", 1),
                ("GAIN", 2),
                ("OUT", 3),
                ("REGION", 5),
                ("SPREAD", 9),
                ("CROP", 10),
                ("ORIENTED", 14),
                ("ZOOM", 16),
                ("SIN", 17),
                ("COS", 18),
                ("TURNS", 19),
                ("FLIP", 20),
                ("INSET", 22),
                ("TRANSFORM", 26),
                ("HOMOGRAPHY", 27),
                ("NOISE", 36),
                ("LENS", 42),
                ("CENTER", 43),
                ("HALF", 45),
                ("FILL", 46),
                ("AMOUNT", 47),
                ("DISTORTION", 48),
                ("RED", 50),
                ("BLUE", 52),
                ("VIGNETTING", 54),
                ("VIGNETTING_AMOUNT", 56),
                ("REDUCED", 57),
                ("COUNT", 59),
                ("SLIDERS", 60),
                ("BOX", 64),
                ("BOX_COUNT", 68),
                ("MANUAL", 70),
            ]
        );
        assert_eq!(HEADER, 73);
    }
}
