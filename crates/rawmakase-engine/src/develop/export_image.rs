//! A render for export together with the color space its pixels are encoded in
//! (docs/color-pipeline.md#export-color-spaces). Only [`crate::develop::render_export`]
//! makes one in a space other than sRGB, and the buffer never leaves it: its pixels
//! are read and drawn on in place, and encoded through [`ExportImage::rgb8`] and
//! [`ExportImage::rgb16`]. So a buffer rendered as sRGB can't be written with another
//! space's profile, nor a wide-gamut one handed to an sRGB encoder as a
//! [`Rendered`].
use crate::color::OutputSpace;
use crate::rendered::{Rendered, unit_to_u8};
use std::borrow::Cow;

/// Pixel format requested for an export render.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ExportFormat {
    /// Floating-point RGB (`[f32; 3]`, 12 bytes/pixel). Required when CPU
    /// post-processing such as floating-point watermark compositing runs on
    /// the rendered pixels.
    #[default]
    F32,
    /// Packed 8-bit unsigned RGB (`3 * width * height` bytes).
    Rgb8,
    /// Packed 16-bit unsigned RGB (`3 * width * height` `u16` samples).
    Rgb16,
}

/// Owned pixel storage of an [`ExportImage`], preserving its [`OutputSpace`].
#[derive(Clone, Debug, PartialEq)]
pub enum ExportPixels {
    F32(Vec<[f32; 3]>),
    Rgb8(Vec<u8>),
    Rgb16(Vec<u16>),
}

/// A value from 0 to 1 as a 16-bit sample, rounded; out-of-range values are clamped.
fn unit_to_u16(v: f32) -> u16 {
    (v.clamp(0., 1.) * 65535. + 0.5) as u16
}

/// Finished export pixels, encoded in [`ExportImage::space`].
#[derive(Clone)]
pub struct ExportImage {
    width: u32,
    height: u32,
    pixels: ExportPixels,
    space: OutputSpace,
}
impl ExportImage {
    /// `image`, encoded sRGB as every render but [`crate::develop::render_export`]'s is.
    pub fn srgb(image: Rendered) -> Self {
        Self::rendered_in(image, OutputSpace::Srgb)
    }
    pub(crate) fn rendered_in(image: Rendered, space: OutputSpace) -> Self {
        Self {
            width: image.width,
            height: image.height,
            pixels: ExportPixels::F32(image.pixels),
            space,
        }
    }
    pub(crate) fn from_pixels(
        width: u32,
        height: u32,
        pixels: ExportPixels,
        space: OutputSpace,
    ) -> Self {
        let count = width as usize * height as usize;
        match &pixels {
            ExportPixels::F32(v) => debug_assert_eq!(v.len(), count),
            ExportPixels::Rgb8(v) => debug_assert_eq!(v.len(), count * 3),
            ExportPixels::Rgb16(v) => debug_assert_eq!(v.len(), count * 3),
        }
        Self {
            width,
            height,
            pixels,
            space,
        }
    }
    /// Converts this image's pixel storage to `format` while preserving [`Self::space`].
    pub(crate) fn into_format(self, format: ExportFormat) -> Self {
        let (width, height, space) = (self.width, self.height, self.space);
        match (format, &self.pixels) {
            (ExportFormat::F32, ExportPixels::F32(_))
            | (ExportFormat::Rgb8, ExportPixels::Rgb8(_))
            | (ExportFormat::Rgb16, ExportPixels::Rgb16(_)) => self,
            (ExportFormat::Rgb8, _) => {
                let bytes = self.into_rgb8();
                Self::from_pixels(width, height, ExportPixels::Rgb8(bytes), space)
            }
            (ExportFormat::Rgb16, _) => {
                let samples = self.into_rgb16();
                Self::from_pixels(width, height, ExportPixels::Rgb16(samples), space)
            }
            (ExportFormat::F32, ExportPixels::Rgb8(bytes)) => {
                let pixels = bytes
                    .as_chunks::<3>()
                    .0
                    .iter()
                    .map(|c| {
                        [
                            f32::from(c[0]) / 255.,
                            f32::from(c[1]) / 255.,
                            f32::from(c[2]) / 255.,
                        ]
                    })
                    .collect();
                Self::from_pixels(width, height, ExportPixels::F32(pixels), space)
            }
            (ExportFormat::F32, ExportPixels::Rgb16(samples)) => {
                let pixels = samples
                    .as_chunks::<3>()
                    .0
                    .iter()
                    .map(|c| {
                        [
                            f32::from(c[0]) / 65535.,
                            f32::from(c[1]) / 65535.,
                            f32::from(c[2]) / 65535.,
                        ]
                    })
                    .collect();
                Self::from_pixels(width, height, ExportPixels::F32(pixels), space)
            }
        }
    }
    pub fn space(&self) -> OutputSpace {
        self.space
    }
    pub fn width(&self) -> u32 {
        self.width
    }
    pub fn height(&self) -> u32 {
        self.height
    }
    /// Storage format of this image's pixels.
    pub fn format(&self) -> ExportFormat {
        match &self.pixels {
            ExportPixels::F32(_) => ExportFormat::F32,
            ExportPixels::Rgb8(_) => ExportFormat::Rgb8,
            ExportPixels::Rgb16(_) => ExportFormat::Rgb16,
        }
    }
    /// Underlying pixel storage.
    pub fn storage(&self) -> &ExportPixels {
        &self.pixels
    }
    /// Consumes this image and returns its underlying pixel storage.
    pub fn into_pixels(self) -> ExportPixels {
        self.pixels
    }
    /// Borrowed floating-point pixels, if stored as [`ExportPixels::F32`].
    pub fn as_f32(&self) -> Option<&[[f32; 3]]> {
        match &self.pixels {
            ExportPixels::F32(v) => Some(v),
            _ => None,
        }
    }
    /// Borrowed packed 8-bit RGB bytes, if stored as [`ExportPixels::Rgb8`].
    pub fn as_rgb8(&self) -> Option<&[u8]> {
        match &self.pixels {
            ExportPixels::Rgb8(v) => Some(v),
            _ => None,
        }
    }
    /// Borrowed packed 16-bit RGB samples, if stored as [`ExportPixels::Rgb16`].
    pub fn as_rgb16(&self) -> Option<&[u16]> {
        match &self.pixels {
            ExportPixels::Rgb16(v) => Some(v),
            _ => None,
        }
    }
    /// The floating-point pixels, encoded in [`ExportImage::space`], row by row.
    ///
    /// Panics if this image was produced in packed `Rgb8` or `Rgb16` format; request
    /// [`ExportFormat::F32`] when floating-point pixel access is needed.
    pub fn pixels(&self) -> &[[f32; 3]] {
        self.as_f32()
            .expect("ExportImage is not stored as f32 pixels; request ExportFormat::F32")
    }
    /// The floating-point pixels, to draw on (a watermark) in [`ExportImage::space`].
    ///
    /// Panics if this image was produced in packed `Rgb8` or `Rgb16` format; request
    /// [`ExportFormat::F32`] when floating-point pixel editing is needed.
    pub fn pixels_mut(&mut self) -> &mut [[f32; 3]] {
        match &mut self.pixels {
            ExportPixels::F32(v) => v,
            _ => panic!("ExportImage is not stored as f32 pixels; request ExportFormat::F32"),
        }
    }
    /// Borrowed or converted 8-bit RGB bytes (zero-copy when stored as [`ExportPixels::Rgb8`]).
    pub fn rgb8_cow(&self) -> Cow<'_, [u8]> {
        match &self.pixels {
            ExportPixels::Rgb8(v) => Cow::Borrowed(v),
            _ => Cow::Owned(self.rgb8()),
        }
    }
    /// Borrowed or converted 16-bit RGB samples (zero-copy when stored as [`ExportPixels::Rgb16`]).
    pub fn rgb16_cow(&self) -> Cow<'_, [u16]> {
        match &self.pixels {
            ExportPixels::Rgb16(v) => Cow::Borrowed(v),
            _ => Cow::Owned(self.rgb16()),
        }
    }
    /// [`Rendered::rgb8`] of the pixels.
    pub fn rgb8(&self) -> Vec<u8> {
        match &self.pixels {
            ExportPixels::F32(pixels) => pixels.iter().flatten().map(|v| unit_to_u8(*v)).collect(),
            ExportPixels::Rgb8(bytes) => bytes.clone(),
            ExportPixels::Rgb16(samples) => samples
                .iter()
                .map(|&v| unit_to_u8(f32::from(v) / 65535.))
                .collect(),
        }
    }
    /// [`Rendered::rgb16`] of the pixels.
    pub fn rgb16(&self) -> Vec<u16> {
        match &self.pixels {
            ExportPixels::F32(pixels) => pixels.iter().flatten().map(|v| unit_to_u16(*v)).collect(),
            ExportPixels::Rgb8(bytes) => bytes.iter().map(|&v| u16::from(v) * 257).collect(),
            ExportPixels::Rgb16(samples) => samples.clone(),
        }
    }
    /// Consumes this image and returns 8-bit RGB bytes without copying when already [`ExportPixels::Rgb8`].
    pub fn into_rgb8(self) -> Vec<u8> {
        match self.pixels {
            ExportPixels::Rgb8(bytes) => bytes,
            _ => self.rgb8(),
        }
    }
    /// Consumes this image and returns 16-bit RGB samples without copying when already [`ExportPixels::Rgb16`].
    pub fn into_rgb16(self) -> Vec<u16> {
        match self.pixels {
            ExportPixels::Rgb16(samples) => samples,
            _ => self.rgb16(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera_data::{CameraImage, Metadata};
    use crate::model::effects::VignetteStyle;
    use crate::model::masks::{LocalAdjust, MaskComponent, MaskGroup, MaskShape};
    use crate::model::recipe::Recipe;
    use rawmakase_cancel::Cancel;

    /// XYZ (the identity camera's RGB) of chromaticity `xy` at luminance `y`.
    fn xyz([x, y]: [f32; 2], lum: f32) -> [f32; 3] {
        [x / y * lum, lum, (1. - x - y) / y * lum]
    }
    /// Two greens outside sRGB, inside Adobe RGB and ProPhoto: Adobe RGB's green
    /// primary and one a little less saturated, left and right.
    fn greens() -> CameraImage {
        let (w, h) = (64, 48);
        let metadata = Metadata {
            width: w,
            height: h,
            wb: [1.; 3],
            daylight_wb: [1.; 3],
            matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            cam_xyz: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            ..Default::default()
        };
        let a = xyz([0.21, 0.71], 0.18);
        let b = xyz([0.23, 0.67], 0.18);
        CameraImage {
            width: w,
            height: h,
            recovered: Default::default(),
            pixels: (0..w * h)
                .map(|i| if i % w < w / 2 { a } else { b })
                .collect(),
            metadata,
            fast: false,
            scale_factor: 1.,
            scale_clipped: 0,
            hdr: Default::default(),
        }
    }
    type Edit = (&'static str, u32, fn(&mut Recipe));
    const FINISHES: [Edit; 8] = [
        ("plain", 0, |_| {}),
        ("resized", 32, |_| {}),
        ("sharpened", 0, |r| r.sharpening = 0.8),
        ("local detail", 0, |r| {
            r.masks.push(MaskGroup {
                components: vec![MaskComponent::new(MaskShape::Linear {
                    from: [0.2, 0.5],
                    to: [0.8, 0.5],
                })],
                adjust: LocalAdjust {
                    sharpness: 0.8,
                    noise: 0.6,
                    ..Default::default()
                },
                ..Default::default()
            })
        }),
        ("grain", 0, |r| r.effects.grain = 0.3),
        ("highlight priority", 0, |r| {
            r.effects.vignette = -0.6;
            r.effects.vignette_highlights = 0.6;
        }),
        ("color priority", 0, |r| {
            r.effects.vignette = -0.6;
            r.effects.vignette_style = VignetteStyle::ColorPriority;
            r.effects.vignette_highlights = 0.6;
        }),
        ("paint overlay", 0, |r| {
            r.effects.vignette = 0.4;
            r.effects.vignette_style = VignetteStyle::PaintOverlay;
        }),
    ];

    /// An encoded pixel of `space` as unclipped linear sRGB.
    fn as_linear_srgb(space: OutputSpace, p: [f32; 3]) -> [f32; 3] {
        let m = crate::color::inverse(space.from_linear_srgb());
        crate::color::mul(m, p.map(|v| space.decode(v)))
    }

    /// The default space renders exactly what every other render does.
    #[test]
    fn an_srgb_export_is_the_render() -> anyhow::Result<()> {
        let im = greens();
        for (name, edge, edit) in FINISHES {
            let mut r = Recipe::default();
            edit(&mut r);
            let r = r.checked()?;
            let plain = crate::develop::render_with_ai(&im, &r, None, edge, Cancel::NEVER)?;
            let export = crate::develop::render_export(
                &im,
                &r,
                None,
                edge,
                OutputSpace::Srgb,
                Cancel::NEVER,
            )?;
            assert_eq!(export.space(), OutputSpace::Srgb);
            assert!(export.pixels() == plain.pixels.as_slice(), "{name}");
        }
        Ok(())
    }

    /// Greens sRGB can't hold come out of an Adobe RGB or ProPhoto export as
    /// themselves, after every finishing stage: outside sRGB, and apart where sRGB
    /// clips both to the same red channel.
    #[test]
    fn wide_gamut_greens_survive_every_finishing_stage() -> anyhow::Result<()> {
        let im = greens();
        for (name, edge, edit) in FINISHES {
            let mut r = Recipe::default();
            edit(&mut r);
            let r = r.checked()?;
            let srgb = crate::develop::render_with_ai(&im, &r, None, edge, Cancel::NEVER)?;
            let (w, h) = (srgb.width as usize, srgb.height as usize);
            // A block's mean, which grain's noise averages out of.
            let at = |p: &Rendered, x: usize| {
                let mut sum = [0f32; 3];
                for y in h / 2 - 3..=h / 2 + 3 {
                    for x in x - 3..=x + 3 {
                        for (s, v) in sum.iter_mut().zip(p.pixels[y * w + x]) {
                            *s += v / 49.;
                        }
                    }
                }
                sum
            };
            let (left, right) = (w * 5 / 16, w * 11 / 16);
            // sRGB clips both greens' red to nothing (but grain's noise).
            for x in [left, right] {
                assert!(at(&srgb, x)[0] < 0.04, "{name}: sRGB {:?}", at(&srgb, x));
            }
            for space in [OutputSpace::AdobeRgb, OutputSpace::ProPhoto] {
                let out = crate::develop::render_export(&im, &r, None, edge, space, Cancel::NEVER)?;
                assert_eq!(out.space(), space);
                assert_eq!((out.width(), out.height()), (srgb.width, srgb.height));
                let out = Rendered {
                    width: out.width(),
                    height: out.height(),
                    pixels: out.pixels().to_vec(),
                };
                let [a, b] = [left, right].map(|x| as_linear_srgb(space, at(&out, x)));
                assert!(
                    a[0] < -0.01 && b[0] < -0.01,
                    "{name} {space:?}: {a:?} {b:?}"
                );
                assert!(a[0] < b[0] - 0.005, "{name} {space:?}: {a:?} {b:?}");
                // The rest of the color is sRGB's where sRGB holds it.
                let s = at(&srgb, left).map(crate::color::srgb_decode);
                assert!((a[1] - s[1]).abs() < 0.08, "{name} {space:?}: {a:?} {s:?}");
            }
        }
        Ok(())
    }

    /// A Color Range mask measures the photo in sRGB in every space, so it brightens
    /// the same pixels: converted back and clipped into sRGB, each export is the sRGB
    /// render.
    #[test]
    fn range_masks_select_the_same_pixels_in_every_space() -> anyhow::Result<()> {
        // In-gamut colors: a red that the mask picks and a blue it does not.
        let (w, h) = (32, 16);
        let mut im = greens();
        (im.width, im.height) = (w, h);
        (im.metadata.width, im.metadata.height) = (w, h);
        let red = xyz([0.5, 0.33], 0.15);
        let blue = xyz([0.2, 0.2], 0.12);
        im.pixels = (0..w * h)
            .map(|i| if i % w < w / 2 { red } else { blue })
            .collect();
        let mut r = Recipe {
            sharpening: 0.,
            ..Default::default()
        };
        let plain = crate::develop::render(&im, &r.clone().checked()?, 0, Cancel::NEVER)?;
        let picked = plain.pixels[(h / 2 * w + 4) as usize];
        r.masks.push(MaskGroup {
            components: vec![MaskComponent::new(MaskShape::ColorRange {
                samples: vec![crate::develop::pipeline::srgb_to_lab(
                    picked.map(crate::color::srgb_decode),
                )],
                amount: 0.5,
            })],
            adjust: LocalAdjust {
                exposure: 1.,
                ..Default::default()
            },
            ..Default::default()
        });
        let r = r.checked()?;
        let srgb = crate::develop::render(&im, &r, 0, Cancel::NEVER)?;
        let left = (h / 2 * w + 4) as usize;
        let right = (h / 2 * w + w - 4) as usize;
        assert!(
            srgb.pixels[left][1] > plain.pixels[left][1] + 0.05,
            "the mask brightens red: {:?} {:?}",
            plain.pixels[left],
            srgb.pixels[left]
        );
        assert!((srgb.pixels[right][2] - plain.pixels[right][2]).abs() < 1e-3);
        for space in [OutputSpace::AdobeRgb, OutputSpace::ProPhoto] {
            let out = crate::develop::render_export(&im, &r, None, 0, space, Cancel::NEVER)?;
            for (o, s) in out.pixels().iter().zip(&srgb.pixels) {
                // Clipped back into sRGB: the brightened red passes sRGB's white.
                let back =
                    as_linear_srgb(space, *o).map(|v| crate::color::srgb_encode(v.clamp(0., 1.)));
                assert!(
                    (0..3).all(|c| (back[c] - s[c]).abs() < 2e-3),
                    "{space:?}: {back:?} {s:?}"
                );
            }
        }
        Ok(())
    }

    /// An export never shows Visualize Range, which grays encoded sRGB: in every
    /// space it renders as the edit without it.
    #[test]
    fn an_export_leaves_point_colors_visualize_range_out() -> anyhow::Result<()> {
        use crate::model::point_color::{PointColor, visualize_range};
        let im = greens();
        let r = Recipe {
            point_colors: vec![PointColor::sampled([0.2, 0.8, 0.1])],
            ..Default::default()
        };
        let mut shown = r.clone();
        shown.point_colors = visualize_range(&r.point_colors, 0).unwrap();
        for space in OutputSpace::ALL {
            let render = |r: &Recipe| -> anyhow::Result<Vec<[f32; 3]>> {
                let out = crate::develop::render_export(
                    &im,
                    &r.checked()?,
                    None,
                    0,
                    space,
                    Cancel::NEVER,
                )?;
                Ok(out.pixels().to_vec())
            };
            assert_eq!(render(&shown)?, render(&r)?, "{space:?}");
        }
        Ok(())
    }
}
