//! Developed pixels as values: an output image, its histogram and clipping, as
//! the renderer, export and watermarks pass them; nothing here renders.
/// A value from 0 to 1 as a byte, rounded; out-of-range values are clamped.
pub fn unit_to_u8(v: f32) -> u8 {
    (v.clamp(0., 1.) * 255. + 0.5) as u8
}
/// A value from 0 to 1 as a 16-bit sample, rounded; out-of-range values are clamped.
pub(crate) fn unit_to_u16(v: f32) -> u16 {
    (v.clamp(0., 1.) * 65535. + 0.5) as u16
}
/// A channel at or above this clips in the highlights, and at or below
/// [`SHADOW_CLIP`] in the shadows. Both are rendered output values (encoded
/// sRGB, 0–1) before any monitor profile, so the display never changes what
/// counts as clipped. The GPU's `present.wgsl` uses the same values.
pub const HIGHLIGHT_CLIP: f32 = 0.999;
/// See [`HIGHLIGHT_CLIP`].
pub const SHADOW_CLIP: f32 = 0.001;

/// Per channel, how many pixels clip: at or above [`HIGHLIGHT_CLIP`], at or
/// below [`SHADOW_CLIP`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Clipped {
    pub shadows: [u32; 3],
    pub highlights: [u32; 3],
}
/// 256 bins per channel of the rendered output, and its clipped pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Histogram {
    pub bins: [[u32; 256]; 3],
    pub clipped: Clipped,
}
impl Histogram {
    pub const EMPTY: Self = Self {
        bins: [[0; 256]; 3],
        clipped: Clipped {
            shadows: [0; 3],
            highlights: [0; 3],
        },
    };
    /// The number of pixels counted.
    pub fn total(&self) -> u32 {
        self.bins[1].iter().sum()
    }
}
impl Default for Histogram {
    fn default() -> Self {
        Self::EMPTY
    }
}

/// Which clipping warnings are painted over the shown photo: clipped
/// highlights red where any channel clips, clipped shadows blue where all
/// three do.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClipOverlay {
    pub shadows: bool,
    pub highlights: bool,
}
impl ClipOverlay {
    pub const NONE: Self = Self {
        shadows: false,
        highlights: false,
    };
    pub const HIGHLIGHT_COLOR: [u8; 3] = [255, 40, 40];
    pub const SHADOW_COLOR: [u8; 3] = [40, 80, 255];
    pub fn any(self) -> bool {
        self.shadows || self.highlights
    }
    /// The warning colour shown instead of this rendered pixel, if any.
    pub fn color(self, pixel: [f32; 3]) -> Option<[u8; 3]> {
        if self.highlights && pixel.iter().any(|v| *v >= HIGHLIGHT_CLIP) {
            Some(Self::HIGHLIGHT_COLOR)
        } else if self.shadows && pixel.iter().all(|v| *v <= SHADOW_CLIP) {
            Some(Self::SHADOW_COLOR)
        } else {
            None
        }
    }
    /// Paints the warnings into `rgb`, the display bytes of `pixels`.
    pub fn paint(self, rgb: &mut [u8], pixels: &[[f32; 3]]) {
        if !self.any() {
            return;
        }
        for (p, orig) in rgb.as_chunks_mut::<3>().0.iter_mut().zip(pixels) {
            if let Some(color) = self.color(*orig) {
                *p = color;
            }
        }
    }
    /// As `present.wgsl` reads it: 1 highlights, 2 shadows.
    pub fn shader_flags(self) -> u32 {
        self.highlights as u32 | (self.shadows as u32) << 1
    }
}

#[derive(Clone)]
pub struct Rendered {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<[f32; 3]>,
}
impl Rendered {
    pub fn rgb8(&self) -> Vec<u8> {
        self.pixels
            .iter()
            .flatten()
            .map(|v| unit_to_u8(*v))
            .collect()
    }
    pub fn rgb16(&self) -> Vec<u16> {
        self.pixels
            .iter()
            .flatten()
            .map(|v| unit_to_u16(*v))
            .collect()
    }
    pub fn histogram(&self) -> Histogram {
        let mut h = Histogram::EMPTY;
        for p in &self.pixels {
            for (c, &v) in p.iter().enumerate() {
                h.bins[c][(v.clamp(0., 1.) * 255.) as usize] += 1;
                h.clipped.highlights[c] += (v >= HIGHLIGHT_CLIP) as u32;
                h.clipped.shadows[c] += (v <= SHADOW_CLIP) as u32;
            }
        }
        h
    }
}

/// An HDR master: linear ProPhoto RGB (D50), 1 = graphics white, nothing clipped
/// above white and no channel above [`HdrImage::peak`]. Conversion to absolute
/// luminance (nits) and to a transfer function belongs to the output that writes it.
#[derive(Clone, Debug)]
pub struct HdrImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<[f32; 3]>,
    /// 2^HDR limit: the brightest any channel may be.
    pub peak: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rendered(pixels: &[[f32; 3]]) -> Rendered {
        Rendered {
            width: pixels.len() as u32,
            height: 1,
            pixels: pixels.to_vec(),
        }
    }

    #[test]
    fn clipping_is_counted_per_channel_at_the_rendered_thresholds() {
        let h = rendered(&[
            // Red clips; 0.998 lands in the top bins but does not clip.
            [1.0, 0.998, 0.5],
            // Beyond the range still clips, at both ends.
            [1.3, -0.2, 0.0],
            // Just above the shadow threshold and below the highlight one.
            [0.0015, 0.9985, 0.0005],
        ])
        .histogram();
        assert_eq!(h.clipped.highlights, [2, 0, 0]);
        assert_eq!(h.clipped.shadows, [0, 1, 2]);
        assert_eq!(h.total(), 3);
        assert_eq!(h.bins[1][254], 2);
    }

    #[test]
    fn each_clipping_overlay_paints_only_its_own_end() {
        let pixels = [[1.0, 0.2, 0.2], [0.0, 0.0, 0.0], [0.0, 0.5, 0.0]];
        let out = rendered(&pixels);
        let painted = |overlay: ClipOverlay| {
            let mut rgb = out.rgb8();
            overlay.paint(&mut rgb, &out.pixels);
            rgb
        };
        let plain = out.rgb8();
        assert_eq!(painted(ClipOverlay::NONE), plain);
        let highlights = painted(ClipOverlay {
            highlights: true,
            ..ClipOverlay::NONE
        });
        assert_eq!(highlights[..3], ClipOverlay::HIGHLIGHT_COLOR);
        assert_eq!(highlights[3..], plain[3..]);
        let shadows = painted(ClipOverlay {
            shadows: true,
            ..ClipOverlay::NONE
        });
        assert_eq!(shadows[..3], plain[..3]);
        // Only where all three channels are black.
        assert_eq!(shadows[3..6], ClipOverlay::SHADOW_COLOR);
        assert_eq!(shadows[6..], plain[6..]);
        assert_eq!(
            ClipOverlay {
                shadows: true,
                highlights: true
            }
            .shader_flags(),
            3
        );
    }
}
