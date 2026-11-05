//! Camera Look (fitted): a profile fitted to one photo's embedded JPEG, the maker's
//! in-camera rendering, built the way RAWmakase Color is (a base and a look over it),
//! so it needs no new recipe fields. The fitting itself is
//! `rawmakase_decode::camera_look`'s; this module only builds, validates and names
//! what it found.
//!
//! - With a usable camera matrix the base is RAWmakase Standard.
//! - Without one the base is "Camera Matrix (fitted)": a fitted forward matrix that
//!   takes the as-shot neutral to D50, a colour matrix derived from it so that the
//!   as-shot neutral reads as D65, and the ACR3 tone curve. Its white balance is
//!   uncalibrated: temperatures are relative to the as-shot white.
//!
//! The look is an HSV table (72 hues × 8 saturations × 8 values, value sRGB-encoded,
//! in linear ProPhoto RGB as every look table) and a tone curve, with Profile Amount.
//! A fitted profile never changes: its look UUID is the hash of everything else in
//! it, so fitting again makes a new profile and leaves edits that use an old one as
//! they were.
use super::{CameraProfile, Matrix, Table, enhanced::AmountRange, enhanced::Enhanced};
use crate::{camera_data::Metadata, color::curve::CurveLut, color::mul_matrix};
use anyhow::{Result, ensure};

/// The fitted look's name.
pub const LOOK: &str = "Camera Look (fitted)";
/// The fitted base of a camera without a colour matrix.
pub const MATRIX_BASE: &str = "Camera Matrix (fitted)";
const COPYRIGHT: &str = "Fitted by RAWmakase to the camera's embedded preview";
/// The look table's size: hues, saturations, values.
pub const TABLE_DIMS: [usize; 3] = [72, 8, 8];
/// The canonical serialization's version, part of every fitted profile's hash.
const CANONICAL: &[u8] = b"rawmakase-fitted-profile-v1\n";

/// What a fit found for the look: per table node (value-major, then hue, then
/// saturation, as DNG tables are stored) a hue shift in degrees and saturation and
/// value scales; the look's tone curve sampled at 4097 points of sRGB-encoded input;
/// and whether the camera's rendering is black and white.
#[derive(Clone, Debug, PartialEq)]
pub struct FittedLook {
    pub table: Vec<[f32; 3]>,
    pub curve: Vec<f32>,
    pub monochrome: bool,
    /// The look of the camera JPEG it was fitted to, by the maker's name for it.
    pub camera_look: Option<String>,
}
impl FittedLook {
    /// The look that changes nothing.
    pub fn identity() -> Self {
        Self {
            table: vec![[0., 1., 1.]; TABLE_DIMS.iter().product()],
            curve: (0..4097).map(|i| i as f32 / 4096.).collect(),
            monochrome: false,
            camera_look: None,
        }
    }
}

/// The fitted look over `base` (RAWmakase Standard, or [`matrix_base`]): a complete
/// profile, validated, with its look UUID the canonical hash. A camera look name no
/// carried profile could hold is left out: it comes from the file, and the look doesn't
/// depend on it.
pub fn with_look(base: &CameraProfile, look: &FittedLook) -> Result<CameraProfile> {
    ensure!(
        base.enhanced.is_none(),
        "A fitted look needs a base profile"
    );
    ensure!(
        look.table.len() == TABLE_DIMS.iter().product::<usize>() && look.curve.len() == 4097,
        "Invalid fitted look size"
    );
    let mut p = base.clone();
    p.name = LOOK.into();
    p.copyright = COPYRIGHT.into();
    p.enhanced = Some(Enhanced {
        camera_look: look
            .camera_look
            .clone()
            .filter(|l| l.chars().count() <= CAMERA_LOOK_MAX),
        uuid: String::new(),
        base_name: base.name.clone(),
        highlights: 0.,
        shadows: 0.,
        clarity: 0.,
        contrast: 0.,
        blacks: 0.,
        monochrome: look.monochrome,
        amount: Some(AmountRange {
            table_min: 0.,
            table_max: 2.,
        }),
        table: Some(Table {
            dims: TABLE_DIMS,
            data: look.table.clone(),
            srgb: true,
        }),
        rgb: None,
        settings: Default::default(),
        curve: Box::new(CurveLut::from_fn(|i| look.curve[i])),
    });
    let uuid = canonical_hash(&p);
    if let Some(e) = p.enhanced.as_mut() {
        e.uuid = uuid;
    }
    p.validate()?;
    Ok(p)
}

/// `base` with its neutrals tinted by `gains` on linear ProPhoto: a look's table can't
/// move grey, which has no hue, so a camera's grey is the base's to match.
pub fn with_grey_gains(base: &CameraProfile, gains: [f32; 3]) -> CameraProfile {
    use crate::color::{inverse, mul_matrix};
    let diag: Matrix =
        std::array::from_fn(|i| std::array::from_fn(|j| if i == j { gains[i] } else { 0. }));
    let tint = mul_matrix(
        inverse(super::XYZ_TO_PRO),
        mul_matrix(diag, super::XYZ_TO_PRO),
    );
    let mut p = base.clone();
    p.forward1 = mul_matrix(tint, p.forward1);
    p.forward2 = mul_matrix(tint, p.forward2);
    p
}

/// RAWmakase Standard as a fitted look's base: from the camera's colour matrix
/// (the camera table's or the DNG's own ColorMatrix), never from a DNG's embedded
/// profile; `None` without a usable matrix.
pub fn standard_base(m: &Metadata) -> Option<CameraProfile> {
    super::open::matrix_standard(m)
}

/// What a fitted profile's look table makes of linear ProPhoto `pro` (clamped to
/// 0–1 first, as `CameraProfile::finish` does), before the tone curves: what a fit
/// compares with the colour the curves must receive.
pub fn table_output(p: &CameraProfile, pro: [f32; 3]) -> [f32; 3] {
    match &p.enhanced {
        Some(look) => look.apply_table(pro),
        None => pro.map(|v| v.clamp(0., 1.)),
    }
}

/// The fitted base of a camera without a colour matrix: `forward` takes
/// white-balanced camera RGB to XYZ D50 (camera neutral [1, 1, 1] to D50 white, as a
/// DNG ForwardMatrix does), and the colour matrix is derived from it so that the
/// photo's as-shot neutral (`1 / Metadata::wb`, after CameraCalibration and
/// AnalogBalance as every profile takes them) is D65's:
/// CM = diag(neutral) · (CAT(D65→D50)⁻¹ · FM)⁻¹. The ACR3 tone curve.
pub fn matrix_base(m: &Metadata, forward: Matrix) -> Result<CameraProfile> {
    let mut p = CameraProfile {
        name: MATRIX_BASE.into(),
        camera: format!("{} {}", m.make, m.model),
        copyright: COPYRIGHT.into(),
        enhanced: None,
        color1: None,
        calibration_signature: String::new(),
        matrix_calibration_signature: None,
        color2: None,
        forward1: forward,
        forward2: forward,
        kelvin1: 6504.,
        kelvin2: 6504.,
        hue1: None,
        hue2: None,
        look: None,
        tone: super::default_tone(),
        exposure: 0.,
        black_render: super::BlackRender::Auto,
        dynamic_range: super::DynamicRange::Standard,
    };
    let calibration = p.neutral_calibration(m);
    let neutral: [f32; 3] = std::array::from_fn(|c| 1. / (m.wb[c] * calibration[c]));
    ensure!(
        neutral.iter().all(|n| n.is_finite() && *n > 0.),
        "Invalid as-shot white balance"
    );
    let xyz65 = mul_matrix(inverse(super::D65_TO_D50)?, forward);
    let inv = inverse(xyz65)?;
    p.color1 = Some(std::array::from_fn(|i| {
        std::array::from_fn(|j| neutral[i] * inv[i][j])
    }));
    p.validate()?;
    Ok(p)
}

/// `m`'s inverse, refusing a singular or badly conditioned matrix.
fn inverse(m: Matrix) -> Result<Matrix> {
    let [a, b, c] = m;
    let det = a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
        + a[2] * (b[0] * c[1] - b[1] * c[0]);
    ensure!(det.is_finite() && det.abs() > 1e-8, "Singular matrix");
    let inv = crate::color::inverse(m);
    ensure!(
        inv.iter().flatten().all(|v| v.is_finite()),
        "Singular matrix"
    );
    Ok(inv)
}

/// Whether `p`'s base is a fitted matrix, whose white balance has no calibrated
/// temperature (named white balance presets mean nothing under it).
pub fn is_uncalibrated_base(p: &CameraProfile) -> bool {
    match &p.enhanced {
        Some(look) => look.base_name == MATRIX_BASE,
        None => p.name == MATRIX_BASE,
    }
}

/// How much colour data a photo has, for whether Camera Look (fitted) becomes its
/// default. Only `Absent` does; every other photo can still fit one from the
/// Profile menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Eligibility {
    /// No colour matrix (no camera table row, nothing in the file or its maker
    /// notes), no embedded profile and no imported camera profile for it.
    Absent,
    /// A matrix the camera transform rejects, or an embedded profile that can't be
    /// read or is another camera's. Wins over `Absent`.
    Invalid,
    /// A matrix of doubtful origin: RAWmakase's earlier decoder's, Olympus maker
    /// notes', or a DNG's tungsten calibration alone.
    Questionable,
    /// No colour data of its own, but an imported camera profile fits it: the user
    /// chose that data.
    Suppressed,
    /// Colour data RAWmakase uses (and rendered files, monochrome DNGs).
    Usable,
}

/// The photo's [`Eligibility`], given `profiles`, the profiles listed for it.
pub fn eligibility(m: &Metadata, profiles: &[std::sync::Arc<CameraProfile>]) -> Eligibility {
    use crate::camera_data::{EmbeddedProfileStatus as Embedded, MatrixSource as Source};
    if m.is_rendered() || m.monochrome {
        return Eligibility::Usable;
    }
    let invalid_embedded = matches!(
        m.embedded_profile_status,
        Embedded::Unreadable | Embedded::OtherCamera
    );
    match m.matrix_source {
        Source::Absent => {
            if invalid_embedded {
                return Eligibility::Invalid;
            }
            if m.embedded_profile_status != Embedded::None {
                return Eligibility::Usable;
            }
            let imported = profiles.iter().any(|p| {
                p.enhanced.is_none()
                    && !p.is_embedded()
                    && !is_fitted(p)
                    && p.name != super::open::STANDARD
                    && p.ensure_camera(m).is_ok()
            });
            if imported {
                Eligibility::Suppressed
            } else {
                Eligibility::Absent
            }
        }
        _ if standard_base(m).is_none() || invalid_embedded => Eligibility::Invalid,
        Source::CompatObserved | Source::MakerNote | Source::FileTungstenOnly => {
            Eligibility::Questionable
        }
        _ => Eligibility::Usable,
    }
}

/// The carried form's version prefix, and the most a carried profile may expand
/// to (a fitted one is about 250 KB of JSON).
const CARRIED: &str = "v1:";
const CARRIED_MAX: usize = 1 << 20;

/// `p` as an XMP sidecar or preset carries it (`rawmakase:FittedProfile`): the
/// whole profile, base included, so it renders the same wherever it is read,
/// versioned, as zlib in Adobe's base 85 (the encoding of look tables).
pub fn carried(p: &CameraProfile) -> String {
    let json = serde_json::to_vec(p).unwrap_or_default();
    format!("{CARRIED}{}", super::enhanced::compress(&json))
}

/// The fitted profile `text` carries, checked: of a known version, at most 1 MB,
/// a valid fitted look whose UUID is its content's hash (so a profile altered on
/// the way is refused), for the photo's camera.
pub fn from_carried(text: &str, m: &Metadata) -> Result<CameraProfile> {
    let body = text
        .strip_prefix(CARRIED)
        .ok_or_else(|| anyhow::anyhow!("Unknown fitted profile version"))?;
    ensure!(body.len() <= CARRIED_MAX * 2, "Fitted profile too large");
    let json = super::enhanced::expand(body)?;
    ensure!(json.len() <= CARRIED_MAX, "Fitted profile too large");
    let p: CameraProfile = serde_json::from_slice(&json)?;
    p.validate()?;
    let look = p
        .enhanced
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("The fitted profile has no look"))?;
    ensure!(p.name == LOOK, "Not a fitted camera look");
    ensure!(
        look.camera_look
            .as_ref()
            .is_none_or(|l| l.chars().count() <= CAMERA_LOOK_MAX),
        "Invalid fitted profile"
    );
    ensure!(
        look.uuid.eq_ignore_ascii_case(&canonical_hash(&p)),
        "The fitted profile does not match its UUID"
    );
    p.ensure_camera(m)?;
    Ok(p)
}

/// The most characters of a carried profile's camera look name.
const CAMERA_LOOK_MAX: usize = 64;

/// A fitted look's title in the Profile menu: with the camera's look it was fitted
/// to, when the file names one. Others have none.
pub fn title(p: &CameraProfile) -> Option<String> {
    let look = p.enhanced.as_ref()?.camera_look.as_ref()?;
    (p.name == LOOK).then(|| format!("{LOOK}: {look}"))
}

/// Whether `p` is a fitted camera look (or a fitted base).
pub fn is_fitted(p: &CameraProfile) -> bool {
    p.name == LOOK || p.name == MATRIX_BASE
}

/// The MD5 of the profile's canonical serialization without its look UUID, as 32
/// uppercase hex digits: every field that changes how it renders or where it
/// applies (camera, matrices, tables, curves, amount bounds, monochrome).
pub fn canonical_hash(p: &CameraProfile) -> String {
    use md5::Digest;
    let mut p = p.clone();
    if let Some(e) = p.enhanced.as_mut() {
        e.uuid.clear();
    }
    let mut h = md5::Md5::new();
    h.update(CANONICAL);
    h.update(serde_json::to_vec(&p).unwrap_or_default());
    h.finalize().iter().map(|b| format!("{b:02X}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_camera_look_name_too_long_to_carry_is_left_out_of_the_profile() {
        let m = camera();
        let base = standard_base(&m).expect("a matrix");
        let long = with_look(
            &base,
            &FittedLook {
                camera_look: Some("x".repeat(CAMERA_LOOK_MAX + 1)),
                ..FittedLook::identity()
            },
        )
        .unwrap();
        assert_eq!(title(&long), None);
        assert_eq!(long.enhanced.as_ref().unwrap().camera_look, None);
    }

    #[test]
    fn grey_gains_tint_what_the_base_makes_of_camera_neutral() {
        let base = standard_base(&camera()).expect("a matrix");
        let m = base.camera_matrix(6504.);
        let tinted = with_grey_gains(&base, [1., 0.98, 1.]).camera_matrix(6504.);
        let grey = |m: Matrix| crate::color::mul(m, [1., 1., 1.]);
        let (a, b) = (grey(m), grey(tinted));
        assert!((b[0] / a[0] - 1.).abs() < 1e-3 && (b[2] / a[2] - 1.).abs() < 1e-3);
        assert!((b[1] / a[1] - 0.98).abs() < 1e-3);
    }

    #[test]
    fn the_camera_s_look_is_in_the_title_and_travels_with_the_profile() {
        let m = camera();
        let base = standard_base(&m).expect("a matrix");
        let plain = with_look(&base, &FittedLook::identity()).unwrap();
        assert_eq!(title(&plain), None);
        let named = with_look(
            &base,
            &FittedLook {
                camera_look: Some("Classic Chrome".into()),
                ..FittedLook::identity()
            },
        )
        .unwrap();
        assert_eq!(
            title(&named).as_deref(),
            Some("Camera Look (fitted): Classic Chrome")
        );
        assert_ne!(
            plain.enhanced.unwrap().uuid,
            named.enhanced.as_ref().unwrap().uuid
        );
        let back = from_carried(&carried(&named), &m).unwrap();
        assert_eq!(title(&back), title(&named));
    }

    fn camera() -> Metadata {
        Metadata {
            make: "Test".into(),
            model: "Camera".into(),
            cam_xyz: [[0.8, -0.2, -0.1], [-0.4, 1.2, 0.2], [-0.05, 0.2, 0.6]],
            ..Default::default()
        }
    }

    #[test]
    fn the_identity_look_renders_as_its_base() {
        let base = super::super::open::standard(&camera()).unwrap();
        let fitted = with_look(&base, &FittedLook::identity()).unwrap();
        assert_eq!(fitted.name, LOOK);
        assert!(fitted.supports_amount());
        for pro in [[0.2, 0.1, 0.05], [0.5, 0.5, 0.5], [0.05, 0.3, 0.6]] {
            let (a, b) = (base.finish(pro), fitted.finish(pro));
            assert!(
                a.iter().zip(b).all(|(x, y)| (x - y).abs() < 2e-3),
                "{a:?} {b:?}"
            );
        }
    }

    #[test]
    fn the_uuid_is_the_hash_of_everything_else() {
        let base = super::super::open::standard(&camera()).unwrap();
        let mut look = FittedLook::identity();
        let a = with_look(&base, &look).unwrap();
        let uuid = &a.enhanced.as_ref().unwrap().uuid;
        assert_eq!(uuid.len(), 32);
        assert_eq!(*uuid, canonical_hash(&a), "stable, and blind to itself");
        assert_eq!(with_look(&base, &look).unwrap(), a, "deterministic");
        look.table[100][0] = 3.;
        let b = with_look(&base, &look).unwrap();
        assert_ne!(b.enhanced.as_ref().unwrap().uuid, *uuid);
        look = FittedLook::identity();
        look.monochrome = true;
        let c = with_look(&base, &look).unwrap();
        assert_ne!(c.enhanced.as_ref().unwrap().uuid, *uuid);
        let mut other = camera();
        other.model = "Other".into();
        let d = with_look(
            &super::super::open::standard(&other).unwrap(),
            &FittedLook::identity(),
        );
        assert_ne!(d.unwrap().enhanced.unwrap().uuid, *uuid);
    }

    #[test]
    fn only_a_photo_without_colour_data_is_absent() {
        use crate::camera_data::{EmbeddedProfileStatus as Embedded, MatrixSource as Source};
        let bare = Metadata {
            make: "Test".into(),
            model: "Camera".into(),
            ..Default::default()
        };
        assert_eq!(eligibility(&bare, &[]), Eligibility::Absent);
        let unreadable = Metadata {
            embedded_profile_status: Embedded::Unreadable,
            ..bare.clone()
        };
        assert_eq!(eligibility(&unreadable, &[]), Eligibility::Invalid);
        // An imported DCP of any name for the camera: the user's choice stands.
        let mut dcp = super::super::open::standard(&camera()).unwrap();
        dcp.name = "My Own".into();
        dcp.camera = "Test Camera".into();
        let imported = [std::sync::Arc::new(dcp)];
        assert_eq!(eligibility(&bare, &imported), Eligibility::Suppressed);
        let with = |source| Metadata {
            matrix_source: source,
            ..camera()
        };
        assert_eq!(
            eligibility(&with(Source::AdobeDng), &[]),
            Eligibility::Usable
        );
        assert_eq!(
            eligibility(&with(Source::CompatObserved), &[]),
            Eligibility::Questionable
        );
        assert_eq!(
            eligibility(&with(Source::FileTungstenOnly), &[]),
            Eligibility::Questionable
        );
        let singular = Metadata {
            matrix_source: Source::File,
            cam_xyz: [[1., 1., 1.]; 3],
            ..bare
        };
        assert_eq!(eligibility(&singular, &[]), Eligibility::Invalid);
    }

    #[test]
    fn a_carried_profile_comes_back_only_whole_and_for_its_camera() {
        let m = camera();
        let base = super::super::open::standard(&m).unwrap();
        // An incompressible look: every node different.
        let mut look = FittedLook::identity();
        for (i, node) in look.table.iter_mut().enumerate() {
            *node = [
                ((i * 7919) % 61) as f32 - 30.,
                1. + (i % 13) as f32 / 40.,
                1.,
            ];
        }
        let p = with_look(&base, &look).unwrap();
        let text = carried(&p);
        assert!(
            text.starts_with("v1:") && text.len() < 400_000,
            "{}",
            text.len()
        );
        assert_eq!(from_carried(&text, &m).unwrap(), p);
        // Another version, damage, a changed look, another camera: refused.
        assert!(from_carried(&text.replacen("v1:", "v2:", 1), &m).is_err());
        assert!(from_carried(&text[..text.len() / 2], &m).is_err());
        let mut altered = p.clone();
        altered.enhanced.as_mut().unwrap().monochrome = true;
        assert!(
            from_carried(&carried(&altered), &m).is_err(),
            "UUID mismatch"
        );
        let other = Metadata {
            model: "Other".into(),
            ..m
        };
        assert!(from_carried(&text, &other).is_err());
        assert!(is_fitted(&p) && !is_fitted(&base));
    }

    #[test]
    fn a_matrix_base_reads_the_as_shot_neutral_as_d65() {
        let m = Metadata {
            make: "Test".into(),
            model: "Camera".into(),
            wb: [2., 1., 1.4],
            ..Default::default()
        };
        // Some forward matrix: camera neutral to D50 white.
        let d50 = [0.9642, 1., 0.8249];
        let raw: Matrix = [[0.6, 0.3, 0.1], [0.25, 0.7, 0.05], [0.0, 0.1, 0.9]];
        let forward: Matrix = std::array::from_fn(|i| {
            let sum: f32 = raw[i].iter().sum();
            std::array::from_fn(|j| raw[i][j] * d50[i] / sum)
        });
        let p = matrix_base(&m, forward).unwrap();
        assert!(is_uncalibrated_base(&p));
        let back = crate::color::mul(inverse(p.color1.unwrap()).unwrap(), [0.5, 1., 1. / 1.4]);
        let d65 = [0.95047, 1., 1.08883];
        assert!(
            back.iter().zip(d65).all(|(a, b)| (a - b).abs() < 1e-3),
            "{back:?}"
        );
        // As shot reads as D65 (tint about +10: D65 lies off the Planckian locus), and
        // that white gives back the as-shot multipliers.
        let [t, tint] = p.as_shot_white_balance(&m).unwrap();
        assert!(
            (t - 6504.).abs() < 60. && (tint - 10.).abs() < 3.,
            "{t} {tint}"
        );
        let wb = p.white_balance(t, tint, &m).unwrap();
        assert!(wb.iter().all(|g| (g - 1.).abs() < 1e-3), "{wb:?}");
        let look = with_look(&p, &FittedLook::identity()).unwrap();
        assert!(is_uncalibrated_base(&look));
        assert!(!is_uncalibrated_base(
            &super::super::open::standard(&camera()).unwrap()
        ));
        let unbalanced = Metadata {
            wb: [0., 1., 1.],
            ..m
        };
        assert!(matrix_base(&unbalanced, forward).is_err());
    }
}
