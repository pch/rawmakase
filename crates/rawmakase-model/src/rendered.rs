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

/// Bins per stop in an [`HdrHistogram`]'s section above graphics white.
pub const HDR_BINS_PER_STOP: usize = 32;
/// How many stops above graphics white an [`HdrHistogram`] holds: the largest HDR
/// limit ([`crate::model::hdr::LIMIT_RANGE`]).
pub const HDR_HISTOGRAM_STOPS: usize = 8;
const HDR_BINS: usize = HDR_BINS_PER_STOP * HDR_HISTOGRAM_STOPS;
/// The most a channel of graphics white reaches through a primaries conversion's
/// rounding: values up to it count as white, not as light above it.
const WHITE_ROUNDING: f32 = 1.001;

/// An HDR master's histogram, as Lightroom divides it: channels at or below graphics
/// white by their sRGB-encoded value, as [`Histogram`] bins the SDR rendition, and
/// channels above it by stops above white. Channels are the master's in linear sRGB
/// primaries, as the HDR preview shows them; negative values count as 0. Nothing in
/// it depends on a display: headroom is applied by whoever draws it
/// ([`HdrHistogram::above`]).
#[derive(Clone, Debug, PartialEq)]
pub struct HdrHistogram {
    /// 256 bins per channel from 0 to graphics white, by sRGB-encoded value.
    pub sdr: [[u32; 256]; 3],
    /// [`HDR_BINS_PER_STOP`] bins per stop above graphics white, up to
    /// [`HDR_HISTOGRAM_STOPS`]: bin `i` holds values from 2^(i/32) up to 2^((i+1)/32).
    pub hdr: [[u32; HDR_BINS]; 3],
    /// Per channel, the pixels whose encoded value is at or below [`SHADOW_CLIP`], as
    /// [`Clipped::shadows`].
    pub shadows: [u32; 3],
    /// Per ProPhoto channel, the pixels at the HDR limit: at or above
    /// [`HIGHLIGHT_CLIP`] of [`HdrImage::peak`], where its roll-off flattens them.
    pub at_limit: [u32; 3],
    /// The master's HDR limit, in stops above graphics white.
    pub limit: f32,
}
impl HdrHistogram {
    /// The number of pixels counted.
    pub fn total(&self) -> u32 {
        self.sdr[1].iter().sum::<u32>() + self.hdr[1].iter().sum::<u32>()
    }
    /// Per channel, the pixels above `stops` over graphics white, to a bin: what a
    /// display with 2^`stops` of headroom can't show. At 0 stops or less, every pixel
    /// above white.
    pub fn above(&self, stops: f32) -> [u32; 3] {
        let from = (stops.max(0.) * HDR_BINS_PER_STOP as f32).ceil() as usize;
        self.hdr
            .map(|bins| bins.get(from..).map_or(0, |b| b.iter().sum()))
    }
}

impl HdrImage {
    /// The master's histogram ([`HdrHistogram`]).
    pub fn histogram(&self) -> HdrHistogram {
        let to_srgb = crate::color::hdr_output::HdrGamut::Rec709.prophoto_to_rgb();
        let mut h = HdrHistogram {
            sdr: [[0; 256]; 3],
            hdr: [[0; HDR_BINS]; 3],
            shadows: [0; 3],
            at_limit: [0; 3],
            limit: self.peak.log2(),
        };
        let top = HIGHLIGHT_CLIP * self.peak;
        for p in &self.pixels {
            for (c, &v) in p.iter().enumerate() {
                h.at_limit[c] += (v >= top) as u32;
            }
            for (c, v) in crate::color::mul(to_srgb, *p).into_iter().enumerate() {
                let v = if v.is_finite() { v.max(0.) } else { 0. };
                // Graphics white converts to sRGB primaries a rounding above 1; it
                // stays white.
                if v <= WHITE_ROUNDING {
                    let encoded = crate::color::srgb_encode(v.min(1.));
                    h.sdr[c][(encoded * 255.) as usize] += 1;
                    h.shadows[c] += (encoded <= SHADOW_CLIP) as u32;
                } else {
                    let bin = (v.log2() * HDR_BINS_PER_STOP as f32) as usize;
                    h.hdr[c][bin.min(HDR_BINS - 1)] += 1;
                }
            }
        }
        h
    }
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

    /// Below white the master is binned as the SDR histogram bins its rendition; above
    /// it by stops, so a display's headroom picks out what it can't show.
    #[test]
    fn an_hdr_histogram_bins_light_above_white_in_stops() {
        let master = HdrImage {
            width: 5,
            height: 1,
            // White, 1 and 2.5 stops above it, the limit's top, and black.
            pixels: vec![[1.; 3], [2.; 3], [2f32.powf(2.5); 3], [15.99; 3], [0.; 3]],
            peak: 16.,
        };
        let h = master.histogram();
        assert_eq!(h.total(), 5);
        assert!((h.limit - 4.).abs() < 1e-6);
        // Neutrals stay neutral in sRGB primaries, to rounding.
        for c in 0..3 {
            assert_eq!(h.sdr[c][255] + h.sdr[c][254], 1, "white, channel {c}");
            assert_eq!(h.sdr[c][0], 1);
            assert_eq!(h.shadows[c], 1);
            assert_eq!(h.hdr[c].iter().sum::<u32>(), 3);
        }
        assert_eq!(h.at_limit, [1; 3]);
        // A display with 2 stops of headroom can't show the two brightest; with 3
        // stops only the one at the limit; with none, all three above white.
        assert_eq!(h.above(2.), [2; 3]);
        assert_eq!(h.above(3.), [1; 3]);
        assert_eq!(h.above(0.), [3; 3]);
        assert_eq!(h.above(-1.), [3; 3]);
        assert_eq!(h.above(8.), [0; 3]);
        // Light beyond the bins' 8 stops lands in the last; a pixel that isn't a
        // number counts as black.
        let bright = HdrImage {
            width: 2,
            height: 1,
            pixels: vec![[1e6; 3], [f32::NAN, 0., -2.]],
            peak: 256.,
        }
        .histogram();
        assert_eq!(bright.total(), 2);
        assert_eq!(bright.above(7.9), [1; 3]);
        assert_eq!(bright.sdr.map(|c| c[0]), [1; 3]);
    }
}
