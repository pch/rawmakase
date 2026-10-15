//! An AVIF still: our reading of its container ([`rawmakase_interop::isobmff`]),
//! its AV1 decoded by rav1d ([`super::av1`]) and turned into RGB with the
//! matrix and range it says.
//!
//! The colour comes from the `colr` boxes: an ICC profile, or one built from
//! the CICP code points; an AVIF with neither is taken as sRGB (the AV1
//! header's own CICP is not consulted, as it is read only when decoding).
//! The matrix and range come from the `nclx` box, or from the AV1 header
//! without one. Alpha is dropped. Tiled (`grid`) images are refused.
use super::{Unsupported, av1};
use anyhow::{Result, anyhow, bail};
use image::ImageDecoder;
use image::metadata::Orientation;
use moxcms::{
    CicpColorPrimaries, CicpProfile, ColorProfile, MatrixCoefficients, TransferCharacteristics,
};
use rawmakase_interop::isobmff::{Colour, Heif, Property};
use std::path::Path;
use yuv::{YuvGrayImage, YuvPlanarImage, YuvRange, YuvStandardMatrix};

/// The CICP code points of an `nclx` box.
#[derive(Clone, Copy, Debug)]
struct Nclx {
    primaries: u16,
    transfer: u16,
    matrix: u16,
    full_range: bool,
}

pub(super) struct AvifFile {
    width: u32,
    height: u32,
    /// More than 8 bits per sample, from `pixi` or `av1C`.
    deep: bool,
    /// The primary item's AV1 data; empty for a tiled image, which is not read.
    obu: Vec<u8>,
    icc: Option<Vec<u8>>,
    nclx: Option<Nclx>,
    orientation: Orientation,
    refused: Option<Unsupported>,
}

/// `irot` (anticlockwise quarter turns) then `imir` (0: left and right swap,
/// 1: top and bottom) as the orientation `image` applies (a clockwise turn,
/// then a horizontal flip).
pub(super) fn orientation(rotation: u8, mirror: Option<u8>) -> Orientation {
    use Orientation::*;
    match (rotation & 3, mirror.map(|m| m & 1)) {
        (0, None) => NoTransforms,
        (1, None) => Rotate270,
        (2, None) => Rotate180,
        (3, None) => Rotate90,
        (0, Some(0)) | (2, Some(1)) => FlipHorizontal,
        (0, Some(_)) | (2, Some(_)) => FlipVertical,
        (1, Some(0)) | (3, Some(1)) => Rotate270FlipH,
        (1 | 3, Some(_)) => Rotate90FlipH,
        _ => unreachable!("a rotation is masked to 0..=3"),
    }
}

impl AvifFile {
    pub(super) fn open(path: &Path) -> Result<Self> {
        let mut file = std::fs::File::open(path)?;
        let heif = Heif::read(&mut file).ok_or_else(|| anyhow!("Could not read the AVIF file"))?;
        let primary = heif
            .item(heif.primary)
            .ok_or_else(|| anyhow!("The AVIF file has no image"))?;
        let tiled = &primary.kind == b"grid";
        if !tiled && &primary.kind != b"av01" {
            bail!("The AVIF image is not AV1");
        }
        let props = heif.properties(heif.primary);
        let (width, height) = props
            .iter()
            .find_map(|p| match p {
                Property::Size(w, h) => Some((*w, *h)),
                _ => None,
            })
            .ok_or_else(|| anyhow!("The AVIF image has no size"))?;
        let icc = props.iter().find_map(|p| match p {
            Property::Colour(Colour::Icc(icc)) => Some(icc.clone()),
            _ => None,
        });
        let nclx = props.iter().find_map(|p| match p {
            Property::Colour(Colour::Nclx {
                primaries,
                transfer,
                matrix,
                full_range,
            }) => Some(Nclx {
                primaries: *primaries,
                transfer: *transfer,
                matrix: *matrix,
                full_range: *full_range,
            }),
            _ => None,
        });
        let rotation = props.iter().find_map(|p| match p {
            Property::Rotation(r) => Some(*r),
            _ => None,
        });
        let mirror = props.iter().find_map(|p| match p {
            Property::Mirror(m) => Some(*m),
            _ => None,
        });
        // av1C's third byte: seq_tier_0, then high_bitdepth.
        let deep = props.iter().any(|p| match p {
            Property::Depth(d) => d.iter().any(|b| *b > 8),
            Property::Av1Config(c) => c.get(2).is_some_and(|b| b & 0x40 != 0),
            _ => false,
        });
        let refused = if tiled {
            Some(Unsupported("tiled AVIF".into()))
        } else if nclx.is_some_and(|n| matches!(n.transfer, 16 | 18)) {
            Some(Unsupported("HDR".into()))
        } else if heif.brands.first() == Some(b"avis") {
            Some(Unsupported("animated".into()))
        } else {
            None
        };
        // `prepare` refuses a file above 150 MP from its size before decoding;
        // its data is not read either. Otherwise at most 256 MiB is.
        let obu = if tiled || (width as u64) * (height as u64) > 150_000_000 {
            Vec::new()
        } else {
            heif.data(&mut file, heif.primary)
                .ok_or_else(|| anyhow!("The AVIF image data is missing or cut off"))?
        };
        Ok(Self {
            width,
            height,
            deep,
            obu,
            icc,
            nclx,
            orientation: orientation(rotation.unwrap_or(0), mirror),
            refused,
        })
    }

    pub(super) fn refusal(&self) -> Option<Unsupported> {
        self.refused.as_ref().map(|r| Unsupported(r.0.clone()))
    }
}

/// A profile for CICP `primaries` and `transfer`, unspecified ones taken as
/// sRGB's (as libavif does); `None` for sRGB itself or primaries moxcms can't
/// describe. A transfer it can't describe is taken as sRGB's.
fn cicp_profile(primaries: u16, transfer: u16) -> Option<ColorProfile> {
    let primaries = if primaries == 2 { 1 } else { primaries };
    let transfer = if transfer == 2 { 13 } else { transfer };
    if (primaries, transfer) == (1, 13) {
        return None;
    }
    let build = |transfer| {
        let cicp = CicpProfile {
            color_primaries: CicpColorPrimaries::try_from(u8::try_from(primaries).ok()?).ok()?,
            transfer_characteristics: transfer,
            matrix_coefficients: MatrixCoefficients::Bt709,
            full_range: true,
        };
        let mut profile = ColorProfile::new_srgb();
        // moxcms 0.8 returns false even when it updated the profile; whether
        // it did is in `cicp`.
        profile.update_rgb_colorimetry_from_cicp(cicp);
        profile.cicp.is_some().then_some(profile)
    };
    u8::try_from(transfer)
        .ok()
        .and_then(|t| TransferCharacteristics::try_from(t).ok())
        .and_then(build)
        .or_else(|| build(TransferCharacteristics::Srgb))
}

/// The `yuv` matrix for CICP matrix coefficients `code`.
fn matrix(code: u8) -> YuvStandardMatrix {
    match code {
        1 => YuvStandardMatrix::Bt709,
        9 | 10 => YuvStandardMatrix::Bt2020,
        7 => YuvStandardMatrix::Smpte240,
        4 => YuvStandardMatrix::Fcc,
        // 5, 6 and unspecified (2): BT.601, as libavif assumes.
        _ => YuvStandardMatrix::Bt601,
    }
}

/// A plane of 16-bit samples as `u16`s and its stride in them: borrowed
/// when aligned (rav1d's planes are), copied otherwise.
fn words(bytes: &[u8], stride: usize) -> (std::borrow::Cow<'_, [u16]>, u32) {
    let words = match bytemuck::try_cast_slice(bytes) {
        Ok(words) => std::borrow::Cow::Borrowed(words),
        Err(_) => bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_ne_bytes(*c))
            .collect(),
    };
    (words, (stride / 2) as u32)
}

/// `samples` of `depth` bits scaled to 16, as native-endian bytes.
fn widen(samples: &[u16], depth: u8) -> Vec<u8> {
    let max = (1u32 << depth) - 1;
    let mut out = vec![0u16; samples.len()];
    for (o, v) in out.iter_mut().zip(samples) {
        *o = ((u32::from(*v).min(max) * 65535 + max / 2) / max) as u16;
    }
    bytemuck::cast_slice(&out).to_vec()
}

/// The picture's pixels as interleaved RGB: 8-bit samples, or 16-bit ones
/// (native endian) scaled up from its depth.
fn to_rgb(picture: &av1::Picture, nclx: Option<Nclx>) -> Result<Vec<u8>> {
    let (w, h) = (picture.width(), picture.height());
    let (code, full) = match nclx {
        Some(n) => (u8::try_from(n.matrix).unwrap_or(2), n.full_range),
        None => (picture.matrix(), picture.full_range()),
    };
    // Identity (GBR): the planes are green, blue and red.
    if code == 0 {
        return identity(picture);
    }
    let range = if full {
        YuvRange::Full
    } else {
        YuvRange::Limited
    };
    let m = matrix(code);
    let depth = picture.depth();
    let (y, ys) = picture.plane(0);
    let (u, us) = picture.plane(1);
    let (v, vs) = picture.plane(2);
    let stride = w * 3;
    let failed = |e: yuv::YuvError| anyhow!("Could not convert the AVIF image: {e:?}");
    if depth == 8 {
        let image = YuvPlanarImage {
            y_plane: y,
            y_stride: ys as u32,
            u_plane: u,
            u_stride: us as u32,
            v_plane: v,
            v_stride: vs as u32,
            width: w,
            height: h,
        };
        let gray = YuvGrayImage {
            y_plane: y,
            y_stride: ys as u32,
            width: w,
            height: h,
        };
        let mut rgb = vec![0u8; stride as usize * h as usize];
        match picture.layout() {
            av1::Layout::I400 => yuv::yuv400_to_rgb(&gray, &mut rgb, stride, range, m),
            av1::Layout::I420 => yuv::yuv420_to_rgb_bilinear(&image, &mut rgb, stride, range, m),
            av1::Layout::I422 => yuv::yuv422_to_rgb_bilinear(&image, &mut rgb, stride, range, m),
            av1::Layout::I444 => yuv::yuv444_to_rgb(&image, &mut rgb, stride, range, m),
        }
        .map_err(failed)?;
        return Ok(rgb);
    }
    let (y16, ys) = words(y, ys);
    let (u16s, us) = words(u, us);
    let (v16, vs) = words(v, vs);
    let image = YuvPlanarImage {
        y_plane: &y16,
        y_stride: ys,
        u_plane: &u16s,
        u_stride: us,
        v_plane: &v16,
        v_stride: vs,
        width: w,
        height: h,
    };
    let gray = YuvGrayImage {
        y_plane: &y16,
        y_stride: ys,
        width: w,
        height: h,
    };
    let mut rgb = vec![0u16; stride as usize * h as usize];
    match (depth, picture.layout()) {
        (10, av1::Layout::I400) => yuv::y010_to_rgb10(&gray, &mut rgb, stride, range, m),
        (10, av1::Layout::I420) => yuv::i010_to_rgb10_bilinear(&image, &mut rgb, stride, range, m),
        (10, av1::Layout::I422) => yuv::i210_to_rgb10_bilinear(&image, &mut rgb, stride, range, m),
        (10, av1::Layout::I444) => yuv::i410_to_rgb10(&image, &mut rgb, stride, range, m),
        (12, av1::Layout::I400) => yuv::y012_to_rgb12(&gray, &mut rgb, stride, range, m),
        (12, av1::Layout::I420) => yuv::i012_to_rgb12_bilinear(&image, &mut rgb, stride, range, m),
        (12, av1::Layout::I422) => yuv::i212_to_rgb12_bilinear(&image, &mut rgb, stride, range, m),
        (12, av1::Layout::I444) => yuv::i412_to_rgb12(&image, &mut rgb, stride, range, m),
        _ => bail!("{depth}-bit AVIF images are not supported"),
    }
    .map_err(failed)?;
    Ok(widen(&rgb, depth))
}

/// An identity-matrix (GBR) picture as RGB, 4:4:4 only.
fn identity(picture: &av1::Picture) -> Result<Vec<u8>> {
    anyhow::ensure!(
        picture.layout() == av1::Layout::I444,
        "A GBR AVIF image must be 4:4:4"
    );
    let (w, h, depth) = (
        picture.width() as usize,
        picture.height() as usize,
        picture.depth(),
    );
    // R, G, B from planes V, Y, U.
    let planes = [picture.plane(2), picture.plane(0), picture.plane(1)];
    if depth == 8 {
        let mut out = Vec::with_capacity(w * h * 3);
        for row in 0..h {
            for x in 0..w {
                out.extend(planes.iter().map(|(data, stride)| data[row * stride + x]));
            }
        }
        return Ok(out);
    }
    let planes = planes.map(|(data, stride)| words(data, stride));
    let mut samples = Vec::with_capacity(w * h * 3);
    for row in 0..h {
        for x in 0..w {
            samples.extend(
                planes
                    .iter()
                    .map(|(data, stride)| data[row * *stride as usize + x]),
            );
        }
    }
    Ok(widen(&samples, depth))
}

impl ImageDecoder for AvifFile {
    fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }
    fn color_type(&self) -> image::ColorType {
        if self.deep {
            image::ColorType::Rgb16
        } else {
            image::ColorType::Rgb8
        }
    }
    fn icc_profile(&mut self) -> image::ImageResult<Option<Vec<u8>>> {
        Ok(match (&self.icc, self.nclx) {
            (Some(icc), _) => Some(icc.clone()),
            (None, Some(n)) => cicp_profile(n.primaries, n.transfer).and_then(|p| p.encode().ok()),
            (None, None) => None,
        })
    }
    fn orientation(&mut self) -> image::ImageResult<Orientation> {
        Ok(self.orientation)
    }
    fn read_image(self, buf: &mut [u8]) -> image::ImageResult<()> {
        let failed = |e: anyhow::Error| {
            image::ImageError::Decoding(image::error::DecodingError::new(
                image::error::ImageFormatHint::Name("AVIF".into()),
                e.to_string(),
            ))
        };
        if self.obu.is_empty() {
            return Err(failed(anyhow!("Tiled AVIF images can't be read")));
        }
        let picture = av1::decode(&self.obu).map_err(failed)?;
        if (picture.width(), picture.height()) != (self.width, self.height)
            || (picture.depth() > 8) != self.deep
        {
            return Err(failed(anyhow!(
                "The AVIF image is not the size or depth it says"
            )));
        }
        let rgb = to_rgb(&picture, self.nclx).map_err(failed)?;
        if rgb.len() != buf.len() {
            return Err(failed(anyhow!("The AVIF image is not the size it says")));
        }
        buf.copy_from_slice(&rgb);
        Ok(())
    }
    fn read_image_boxed(self: Box<Self>, buf: &mut [u8]) -> image::ImageResult<()> {
        (*self).read_image(buf)
    }
}

#[cfg(test)]
mod tests {
    use image::metadata::Orientation::*;

    #[test]
    fn rotations_and_mirrors_turn_into_orientations() {
        let o = super::orientation;
        assert_eq!(o(0, None), NoTransforms);
        assert_eq!(o(1, None), Rotate270);
        assert_eq!(o(2, None), Rotate180);
        assert_eq!(o(3, None), Rotate90);
        assert_eq!(o(0, Some(0)), FlipHorizontal);
        assert_eq!(o(0, Some(1)), FlipVertical);
        assert_eq!(o(1, Some(0)), Rotate270FlipH);
        assert_eq!(o(1, Some(1)), Rotate90FlipH);
        assert_eq!(o(2, Some(0)), FlipVertical);
        assert_eq!(o(3, Some(0)), Rotate90FlipH);
        assert_eq!(o(3, Some(1)), Rotate270FlipH);
    }

    #[test]
    fn unspecified_cicp_is_srgb_and_p3_is_not() {
        assert!(super::cicp_profile(2, 2).is_none());
        assert!(super::cicp_profile(1, 13).is_none());
        let p3 = moxcms::ColorProfile::new_display_p3();
        // P3 primaries with an unspecified transfer keep their primaries.
        for transfer in [13, 2] {
            let profile = super::cicp_profile(12, transfer).unwrap();
            assert!((profile.red_colorant.x - p3.red_colorant.x).abs() < 1e-3);
        }
    }
}
