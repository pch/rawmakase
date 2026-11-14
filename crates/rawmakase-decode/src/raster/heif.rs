//! What AVIF ([`super::avif`]) and HEIC ([`super::heic`]) files share as HEIF
//! images: an item's colour (its ICC profile, or one built from its CICP code
//! points), its orientation from `irot` and `imir`, and widening decoded samples to
//! their bytes.
use image::metadata::Orientation;
use moxcms::{
    CicpColorPrimaries, CicpProfile, ColorProfile, MatrixCoefficients, TransferCharacteristics,
};
use rawmakase_interop::isobmff::{Colour, Property};
use rawmakase_model::color::hdr_output::CicpPrimaries;

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

/// Whether RAWmakase decodes HDR in CICP primaries `code`: BT.709, BT.2020 and Display
/// P3 (1, 9 and 12), and [`CicpPrimaries`]'. The system decoders give the signal back
/// as coded in each (ImageIO drawn into its own space, libheif converting by the
/// matrix alone).
fn hdr_primaries(code: u16) -> bool {
    [1, 9, 12].contains(&code)
        || u8::try_from(code).is_ok_and(|p| CicpPrimaries::from_cicp(p).is_some())
}

/// The CICP of an ICC profile's `cicp` tag: its primaries, its transfer, and whether
/// it is full range; `None` without one, or for a profile moxcms can't read.
fn icc_cicp(icc: &[u8]) -> Option<(u16, u16, bool)> {
    let cicp = ColorProfile::new_from_slice(icc).ok()?.cicp?;
    Some((
        cicp.color_primaries as u16,
        cicp.transfer_characteristics as u16,
        cicp.full_range,
    ))
}

/// Whether a HEIC of colour `nclx` and ICC profile `icc` is HDR: either says PQ or
/// HLG.
pub(super) fn names_hdr(nclx: Option<Nclx>, icc: Option<&[u8]>) -> bool {
    nclx.is_some_and(Nclx::hdr)
        || icc
            .and_then(icc_cicp)
            .is_some_and(|(_, t, _)| matches!(t, 16 | 18))
}

/// Why an HDR HEIC of colour `nclx` and ICC profile `icc` can't open as an HDR edit,
/// if it can't; `None` for an SDR one. Its signal decodes exactly only where the
/// system decoder gives it back as coded, converted to RGB by a matrix it applies as
/// a matrix, which its `nclx` names. An ICC profile, when there is one, describes the
/// pixels, and the `nclx` beside it only how they convert to RGB (ISO/IEC 23008-12);
/// without a profile, the `nclx` describes them. Refused:
/// - **"HDR"** on Windows, whose decoder isn't checked; without an `nclx`, HDR a
///   profile's `cicp` names, as its matrix is then the HEVC stream's own or MIAF's
///   default, which RAWmakase doesn't read (libheif converts constant luminance as
///   non-constant); for a profile without a `cicp` tag of PQ or HLG at full range
///   beside an HDR `nclx`, or one whose `cicp` names other primaries or another
///   transfer than the `nclx`; and for primaries RAWmakase doesn't decode HDR in
///   ([`hdr_primaries`]);
/// - **"HDR matrix N"** for an `nclx` matrix but BT.709 (1), BT.601 (5, 6) and
///   BT.2020's non-constant luminance (9): constant luminance and ICtCp aren't
///   matrices.
pub(super) fn hdr_refusal(nclx: Option<Nclx>, icc: Option<&[u8]>) -> Option<super::Unsupported> {
    use super::Unsupported;
    if !names_hdr(nclx, icc) {
        return None;
    }
    let refused = || Some(Unsupported("HDR".into()));
    if cfg!(windows) {
        return refused();
    }
    let Some(nclx) = nclx else {
        return refused();
    };
    let primaries = match icc {
        Some(icc) => {
            let Some((primaries, transfer, true)) =
                icc_cicp(icc).filter(|(_, t, _)| matches!(t, 16 | 18))
            else {
                return refused();
            };
            if nclx.hdr() && (nclx.primaries, nclx.transfer) != (primaries, transfer) {
                return refused();
            }
            primaries
        }
        None => nclx.primaries,
    };
    if !hdr_primaries(primaries) {
        refused()
    } else if ![1, 5, 6, 9].contains(&nclx.matrix) {
        Some(Unsupported(format!("HDR matrix {}", nclx.matrix)))
    } else {
        None
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

/// `samples` of `depth` bits scaled to 16, as native-endian bytes.
pub(super) fn widen(samples: &[u16], depth: u8) -> Vec<u8> {
    let max = (1u32 << depth) - 1;
    let mut out = vec![0u16; samples.len()];
    for (o, v) in out.iter_mut().zip(samples) {
        *o = ((u32::from(*v).min(max) * 65535 + max / 2) / max) as u16;
    }
    bytemuck::cast_slice(&out).to_vec()
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
