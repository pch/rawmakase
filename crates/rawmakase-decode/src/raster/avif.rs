//! An AVIF still: our reading of its container ([`rawmakase_interop::isobmff`]),
//! its AV1 decoded by rav1d ([`super::av1`]) and turned into RGB with the
//! matrix and range it says.
//!
//! The colour comes from the `colr` boxes: an ICC profile, or one built from
//! the CICP code points; an AVIF with neither is taken as sRGB (the AV1
//! header's own CICP is not consulted, as it is read only when decoding).
//! The matrix and range come from the `nclx` box, or from the AV1 header
//! without one. Alpha is dropped. Tiled (`grid`) images are refused.
//!
//! A HEIC ([`super::heic`]) shares the reading of colour, orientation and
//! CICP here.
use super::{Unsupported, av1};
use anyhow::{Result, anyhow, bail};
use image::ImageDecoder;
use image::metadata::Orientation;
use moxcms::{
    CicpColorPrimaries, CicpProfile, ColorProfile, MatrixCoefficients, TransferCharacteristics,
};
use rawmakase_interop::isobmff::{Colour, Heif, Property};
use rayon::prelude::*;
use std::path::Path;
use yuv::{YuvChromaSubsampling, YuvGrayImage, YuvPlanarImage, YuvRange, YuvStandardMatrix};

/// The CICP code points of an `nclx` box.
#[derive(Clone, Copy, Debug)]
pub(super) struct Nclx {
    pub(super) primaries: u16,
    pub(super) transfer: u16,
    pub(super) matrix: u16,
    pub(super) full_range: bool,
}

impl Nclx {
    /// An HDR transfer: PQ or HLG.
    pub(super) fn hdr(self) -> bool {
        matches!(self.transfer, 16 | 18)
    }
}

/// An item's colour: its ICC profile and its `nclx`, either when it has one.
pub(super) fn colour(props: &[&Property]) -> (Option<Vec<u8>>, Option<Nclx>) {
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
    (icc, nclx)
}

/// The ICC profile describing an item's pixels: its own, else one built
/// from its CICP code points; `None` for sRGB.
pub(super) fn profile(icc: Option<&Vec<u8>>, nclx: Option<Nclx>) -> Option<Vec<u8>> {
    match (icc, nclx) {
        (Some(icc), _) => Some(icc.clone()),
        (None, Some(n)) => cicp_profile(n.primaries, n.transfer).and_then(|p| p.encode().ok()),
        (None, None) => None,
    }
}

/// The orientation an item's `irot` and `imir` give, applied in the order
/// they are associated; a turn after a mirror is the opposite turn before it.
pub(super) fn transforms(props: &[&Property]) -> Orientation {
    let (mut rotation, mut mirror) = (0, None);
    for p in props {
        match p {
            Property::Rotation(r) if mirror.is_some() => rotation = r.wrapping_neg() & 3,
            Property::Rotation(r) => rotation = *r,
            Property::Mirror(m) => mirror = Some(*m),
            _ => {}
        }
    }
    orientation(rotation, mirror)
}

pub(super) struct AvifFile {
    width: u32,
    height: u32,
    /// More than 8 bits per sample, from `pixi` or `av1C`.
    deep: bool,
    /// The file and its boxes, to read the primary item's AV1 data from when
    /// decoding; `None` for a tiled image, which is not read.
    source: Option<(std::fs::File, Heif)>,
    icc: Option<Vec<u8>>,
    nclx: Option<Nclx>,
    orientation: Orientation,
    refused: Option<Unsupported>,
}

/// `irot` (anticlockwise quarter turns) then `imir` (0: top and bottom swap,
/// 1: left and right) as the orientation `image` applies (a clockwise turn,
/// then a horizontal flip).
pub(super) fn orientation(rotation: u8, mirror: Option<u8>) -> Orientation {
    use Orientation::*;
    match (rotation & 3, mirror.map(|m| m & 1)) {
        (0, None) => NoTransforms,
        (1, None) => Rotate270,
        (2, None) => Rotate180,
        (3, None) => Rotate90,
        (0, Some(1)) | (2, Some(0)) => FlipHorizontal,
        (0, Some(_)) | (2, Some(_)) => FlipVertical,
        (1, Some(1)) | (3, Some(0)) => Rotate270FlipH,
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
        if let Some(kind) = heif.unread_essential(heif.primary) {
            let kind = String::from_utf8_lossy(&kind);
            bail!("The AVIF image needs `{kind}`, which RAWmakase can't read");
        }
        let props = heif.properties(heif.primary);
        // An essential crop is read (a HEIC applies it), but an AVIF's is not
        // applied yet; one not marked essential is ignored, as before.
        if heif
            .essential_properties(heif.primary)
            .iter()
            .any(|p| matches!(p, Property::CleanAperture(_)))
        {
            bail!("The AVIF image needs `clap`, which RAWmakase can't read");
        }
        let (width, height) = props
            .iter()
            .find_map(|p| match p {
                Property::Size(w, h) => Some((*w, *h)),
                _ => None,
            })
            .ok_or_else(|| anyhow!("The AVIF image has no size"))?;
        let (icc, nclx) = colour(&props);
        let orientation = transforms(&props);
        // av1C's third byte: seq_tier_0, then high_bitdepth.
        let deep = props.iter().any(|p| match p {
            Property::Depth(d) => d.iter().any(|b| *b > 8),
            Property::Av1Config(c) => c.get(2).is_some_and(|b| b & 0x40 != 0),
            _ => false,
        });
        let refused = if tiled {
            Some(Unsupported("tiled AVIF".into()))
        } else if nclx.is_some_and(Nclx::hdr) {
            Some(Unsupported("HDR".into()))
        } else if heif.brands.contains(b"avis") {
            Some(Unsupported("animated".into()))
        } else {
            None
        };
        Ok(Self {
            width,
            height,
            deep,
            source: (!tiled).then_some((file, heif)),
            icc,
            nclx,
            orientation,
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
pub(super) fn cicp_profile(primaries: u16, transfer: u16) -> Option<ColorProfile> {
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
pub(super) fn matrix(code: u8) -> YuvStandardMatrix {
    match code {
        1 => YuvStandardMatrix::Bt709,
        9 => YuvStandardMatrix::Bt2020,
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

/// A 10- or 12-bit 4:2:0 `image` as RGB of its depth, with yuv's bilinear
/// chroma weights. yuv 0.8.19's own bilinear 4:2:0 above 8 bits gets them
/// wrong on ARM (NEON), tinting every other pixel, so the chroma is upsampled
/// here and converted as 4:4:4, in strips of rows.
pub(super) fn deep420_to_rgb(
    image: &YuvPlanarImage<u16>,
    depth: u8,
    rgb: &mut [u16],
    range: YuvRange,
    m: YuvStandardMatrix,
) -> std::result::Result<(), yuv::YuvError> {
    const STRIP: usize = 16;
    image.check_constraints(YuvChromaSubsampling::Yuv420)?;
    let (w, h) = (image.width as usize, image.height as usize);
    if w == 0 || h == 0 {
        return Err(yuv::YuvError::ZeroBaseSize);
    }
    let stride = w * 3;
    rgb.par_chunks_mut(stride * STRIP)
        .take(h.div_ceil(STRIP))
        .enumerate()
        .try_for_each(|(strip, out)| {
            let top = strip * STRIP;
            let rows = STRIP.min(h - top);
            let mut u = vec![0; w * rows];
            let mut v = vec![0; w * rows];
            for (row, (u, v)) in u.chunks_exact_mut(w).zip(v.chunks_exact_mut(w)).enumerate() {
                upsample(image.u_plane, image.u_stride, w, h, top + row, u);
                upsample(image.v_plane, image.v_stride, w, h, top + row, v);
            }
            let strip = YuvPlanarImage {
                y_plane: &image.y_plane[top * image.y_stride as usize..],
                y_stride: image.y_stride,
                u_plane: &u,
                u_stride: w as u32,
                v_plane: &v,
                v_stride: w as u32,
                width: image.width,
                height: rows as u32,
            };
            if depth == 12 {
                yuv::i412_to_rgb12(&strip, out, stride as u32, range, m)
            } else {
                yuv::i410_to_rgb10(&strip, out, stride as u32, range, m)
            }
        })
}

/// Row `y` of a `w` × `h` picture's 4:2:0 chroma `plane`, rows `stride`
/// apart (checked to hold them), upsampled into `out` as yuv's portable code
/// does: 9/16 from the nearest sample, 3/16 from each next one across and
/// down, 1/16 diagonally.
fn upsample(plane: &[u16], stride: u32, w: usize, h: usize, y: usize, out: &mut [u16]) {
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let (r, last) = (y / 2, ch - 1);
    let (near, far) = if y.is_multiple_of(2) {
        (r, (r + 1).min(last))
    } else {
        ((r + 1).min(last), r)
    };
    let row = |r: usize| &plane[r * stride as usize..][..cw];
    let (n, f) = (row(near), row(far));
    for (x, o) in out.iter_mut().enumerate() {
        let k = x / 2;
        let (a, b) = if x.is_multiple_of(2) {
            (k, (k + 1).min(cw - 1))
        } else {
            ((k + 1).min(cw - 1), k)
        };
        let sum = 9 * u32::from(n[a]) + 3 * u32::from(n[b]) + 3 * u32::from(f[a]) + u32::from(f[b]);
        *o = ((sum + 8) >> 4) as u16;
    }
}

/// `samples` of `depth` bits scaled to 16, as native-endian bytes.
pub(super) fn widen(samples: &[u16], depth: u8) -> Vec<u8> {
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
    // BT.2020 constant luminance (10) and chroma-derived constant luminance
    // (13) need a reconstruction no matrix gives.
    anyhow::ensure!(
        !matches!(code, 10 | 13),
        "AVIF images in constant luminance can't be read"
    );
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
        (10 | 12, av1::Layout::I420) => deep420_to_rgb(&image, depth, &mut rgb, range, m),
        (10, av1::Layout::I422) => yuv::i210_to_rgb10_bilinear(&image, &mut rgb, stride, range, m),
        (10, av1::Layout::I444) => yuv::i410_to_rgb10(&image, &mut rgb, stride, range, m),
        (12, av1::Layout::I400) => yuv::y012_to_rgb12(&gray, &mut rgb, stride, range, m),
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
        Ok(profile(self.icc.as_ref(), self.nclx))
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
        let Some((mut file, heif)) = self.source else {
            return Err(failed(anyhow!("Tiled AVIF images can't be read")));
        };
        // At most 256 MiB; `prepare` refuses a file above 150 MP from its size
        // before this.
        let obu = heif
            .data(&mut file, heif.primary)
            .ok_or_else(|| failed(anyhow!("The AVIF image data is missing or cut off")))?;
        let picture = av1::decode(&obu).map_err(failed)?;
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
        assert_eq!(o(0, Some(0)), FlipVertical);
        assert_eq!(o(0, Some(1)), FlipHorizontal);
        assert_eq!(o(1, Some(0)), Rotate90FlipH);
        assert_eq!(o(1, Some(1)), Rotate270FlipH);
        assert_eq!(o(2, Some(0)), FlipHorizontal);
        assert_eq!(o(2, Some(1)), FlipVertical);
        assert_eq!(o(3, Some(0)), Rotate270FlipH);
        assert_eq!(o(3, Some(1)), Rotate90FlipH);
    }

    #[test]
    fn deep_420_chroma_is_upsampled_with_yuvs_bilinear_weights() {
        // 2×2 chroma under a 4×4 picture, rows 2 apart.
        let plane = [0, 160, 0, 320, 480, 0];
        let mut rows = [[0; 4]; 2];
        for (y, row) in rows.iter_mut().enumerate() {
            super::upsample(&plane, 3, 4, 4, y, row);
        }
        assert_eq!(rows, [[120, 200, 240, 240], [280, 360, 400, 400]]);
    }

    /// yuv 0.8.19's own bilinear 4:2:0 above 8 bits tints every other pixel
    /// of a flat colour on ARM; strips of rows must not show either.
    #[test]
    fn a_flat_deep_420_colour_is_flat_everywhere() {
        use yuv::{YuvPlanarImage, YuvRange, YuvStandardMatrix};
        let (w, h) = (37, 35);
        let (cw, ch) = (19, 18);
        for (depth, [y, u, v]) in [(10, [306, 339, 1023]), (12, [1224, 1356, 4092])] {
            let image = YuvPlanarImage {
                y_plane: &vec![y; w * h],
                y_stride: w as u32,
                u_plane: &vec![u; cw * ch],
                u_stride: cw as u32,
                v_plane: &vec![v; cw * ch],
                v_stride: cw as u32,
                width: w as u32,
                height: h as u32,
            };
            let mut rgb = vec![0; w * h * 3];
            super::deep420_to_rgb(
                &image,
                depth,
                &mut rgb,
                YuvRange::Full,
                YuvStandardMatrix::Bt601,
            )
            .unwrap();
            // Red, within an 8-bit level.
            let (max, near) = ((1 << depth) - 1, 1 << (depth - 8));
            assert!(
                rgb[0] >= max - near && rgb[1] <= near && rgb[2] <= near,
                "{:?}",
                &rgb[..3]
            );
            let [first, ..] = rgb.as_chunks::<3>().0 else {
                unreachable!()
            };
            assert!(
                rgb.as_chunks::<3>().0.iter().all(|p| p == first),
                "{depth} bits"
            );
        }
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
