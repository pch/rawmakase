//! A Lensfun lens calibration as data, and its evaluation for one photo into the
//! radial correction the renderer applies ([`LensCorrection`]). Reading Lensfun's
//! database and choosing a lens are `lens::lensfun`'s; an edit keeps the calibration
//! it chose, so rendering never reads the database.
//!
//! The model, for Lensfun's database format 2 and rectilinear lenses:
//!
//! - A lens is calibrated on a camera of crop factor `crop` whose frame has the aspect
//!   ratio `aspect` (long edge over short, 1.5 when Lensfun gives none). Distortion and
//!   lateral CA are functions of `u`, the radius in units of half that frame's short
//!   edge; vignetting of `v`, in units of half its diagonal. The photo's own frame
//!   (the camera's default crop, of crop factor `Frame::crop`) sets the millimetres
//!   per pixel. Both are measured from the frame's centre, moved by Lensfun's `<center>`
//!   (in units of half the frame's short edge, y down).
//! - Distortion maps a corrected radius to the recorded one. The source radius over
//!   the output radius is 1 + k₁u² + k₂u⁴ (poly5), 1 + k₁u²/(1 − k₁)³ (poly3), and
//!   1 + a·u³/d⁴ + b·u²/d³ + c·u/d², d = 1 − a − b − c (ptlens): Lensfun's polynomials
//!   rescaled so the centre keeps its magnification, as Lensfun applies them.
//! - Lateral CA scales red's and blue's source radius relative to green's, as a
//!   function of green's source radius (after distortion, so the Distortion amount
//!   does not move it): k (linear), v + c·u + b·u² (poly3).
//! - Vignetting darkens by 1 + k₁v² + k₂v⁴ + k₃v⁶ (pa); the correction is its reciprocal.
//! - Distortion and CA are interpolated in focal length by a cubic Hermite spline
//!   between the calibrated focal lengths on either side, with tangents from the next
//!   ones out (from the pair itself at the ends), and held at the nearest beyond them.
//!   Distortion's coefficients and CA's `c` and `b` are multiplied by their focal length
//!   before and divided by the photo's after; CA's `k` and `v` are not.
//! - Vignetting is interpolated by inverse distance weighting (power 3.5) over the
//!   calibrated (focal length, aperture, distance) as (focal length over the calibrated
//!   range, 4 / f-number, 0.1 / distance in metres); a row nearer than 10⁻⁴ is taken as
//!   it is, and none nearer than 1 corrects nothing.
//!
//! These were checked numerically against Lensfun at revision bc5615d6 on synthetic
//! calibrations (`lens::lensfun` tests); the code is RAWmakase's own.
use super::{LensCorrection, Radial};
use serde::{Deserialize, Serialize};

/// The diagonal of a 36 × 24 mm frame, crop factor 1.
const FULL_FRAME_DIAGONAL: f32 = 43.266_617;
/// Lensfun's default aspect ratio of a calibration frame.
pub const DEFAULT_ASPECT: f32 = 1.5;
/// The focus distance Lensfun's vignetting is read at: RAWmakase reads no focus
/// distance yet, so every photo is taken as focused far away.
pub const DISTANCE: f32 = 1000.;
/// A camera of a smaller crop factor than the lens was calibrated on (by more than
/// this) sees parts of the image the calibration did not measure.
pub const CROP_TOLERANCE: f32 = 0.96;
/// How far a sampled distortion or CA table may stray from the model, in pixels of the
/// photo.
const TOLERANCE_PX: f64 = 0.01;
/// How far a sampled vignetting table may stray from the model, as a fraction of the
/// gain.
const TOLERANCE_GAIN: f64 = 1e-4;

/// The projection a lens draws. Only rectilinear lenses are corrected; the others
/// would need their projection converted, which RAWmakase does not do.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LensType {
    #[default]
    Rectilinear,
    Fisheye,
    Panoramic,
    Equirectangular,
    Orthographic,
    Stereographic,
    Equisolid,
    FisheyeThoby,
    /// A type Lensfun may add later.
    #[serde(untagged)]
    Other(String),
}
impl LensType {
    /// Lensfun's `<type>`; missing means rectilinear.
    pub fn parse(text: &str) -> Self {
        match text.trim() {
            "" | "rectilinear" => Self::Rectilinear,
            "fisheye" => Self::Fisheye,
            "panoramic" => Self::Panoramic,
            "equirectangular" => Self::Equirectangular,
            "orthographic" => Self::Orthographic,
            "stereographic" => Self::Stereographic,
            "equisolid" => Self::Equisolid,
            "fisheye_thoby" => Self::FisheyeThoby,
            other => Self::Other(other.to_string()),
        }
    }
    pub fn name(&self) -> &str {
        match self {
            Self::Rectilinear => "rectilinear",
            Self::Fisheye => "fisheye",
            Self::Panoramic => "panoramic",
            Self::Equirectangular => "equirectangular",
            Self::Orthographic => "orthographic",
            Self::Stereographic => "stereographic",
            Self::Equisolid => "equisolid",
            Self::FisheyeThoby => "fisheye (Thoby)",
            Self::Other(name) => name,
        }
    }
}

/// A distortion model and its coefficients.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Distortion {
    Poly3([f32; 1]),
    Poly5([f32; 2]),
    Ptlens([f32; 3]),
}
impl Distortion {
    fn terms(&self) -> [f32; 3] {
        match *self {
            Self::Poly3([k1]) => [k1, 0., 0.],
            Self::Poly5([k1, k2]) => [k1, k2, 0.],
            Self::Ptlens(t) => t,
        }
    }
    fn with_terms(&self, t: [f32; 3]) -> Self {
        match self {
            Self::Poly3(_) => Self::Poly3([t[0]]),
            Self::Poly5(_) => Self::Poly5([t[0], t[1]]),
            Self::Ptlens(_) => Self::Ptlens(t),
        }
    }
    fn same_model(&self, other: &Self) -> bool {
        std::mem::discriminant(self) == std::mem::discriminant(other)
    }
    /// Source radius over output radius at `u`.
    pub fn ratio(&self, u: f64) -> f64 {
        match *self {
            Self::Poly3([k1]) => {
                let k1 = f64::from(k1);
                1. + k1 * u * u / (1. - k1).powi(3)
            }
            Self::Poly5([k1, k2]) => {
                let u2 = u * u;
                1. + f64::from(k1) * u2 + f64::from(k2) * u2 * u2
            }
            Self::Ptlens([a, b, c]) => {
                let (a, b, c) = (f64::from(a), f64::from(b), f64::from(c));
                let d = 1. - a - b - c;
                1. + a * u.powi(3) / d.powi(4) + b * u * u / d.powi(3) + c * u / (d * d)
            }
        }
    }
    fn usable(&self) -> bool {
        let t = self.terms();
        let d = match self {
            Self::Poly3([k1]) => 1. - k1,
            Self::Ptlens([a, b, c]) => 1. - a - b - c,
            Self::Poly5(_) => 1.,
        };
        t.iter().all(|v| v.is_finite()) && d.abs() > 1e-3
    }
}

/// Distortion at one focal length.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DistortionRow {
    pub focal: f32,
    pub model: Distortion,
}

/// A lateral CA model: red's and blue's terms.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tca {
    /// `[kr, kb]`.
    Linear([f32; 2]),
    /// `[vr, vb, cr, cb, br, bb]`, in Lensfun's order.
    Poly3([f32; 6]),
}
impl Tca {
    fn terms(&self) -> [f32; 6] {
        match *self {
            Self::Linear([kr, kb]) => [kr, kb, 0., 0., 0., 0.],
            Self::Poly3(t) => t,
        }
    }
    fn with_terms(&self, t: [f32; 6]) -> Self {
        match self {
            Self::Linear(_) => Self::Linear([t[0], t[1]]),
            Self::Poly3(_) => Self::Poly3(t),
        }
    }
    fn same_model(&self, other: &Self) -> bool {
        std::mem::discriminant(self) == std::mem::discriminant(other)
    }
    /// Red's and blue's source radius over green's, at green's source radius `u`.
    pub fn ratios(&self, u: f64) -> [f64; 2] {
        let t = self.terms().map(f64::from);
        match self {
            Self::Linear(_) => [t[0], t[1]],
            Self::Poly3(_) => [
                t[0] + t[2] * u + t[4] * u * u,
                t[1] + t[3] * u + t[5] * u * u,
            ],
        }
    }
}

/// Lateral CA at one focal length.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TcaRow {
    pub focal: f32,
    pub model: Tca,
}

/// Vignetting (pa) at one focal length, aperture (f-number) and focus distance
/// (metres), saved as `[focal, aperture, distance, k1, k2, k3]` to keep a calibration
/// of several hundred rows small.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(from = "[f32; 6]", into = "[f32; 6]")]
pub struct VignettingRow {
    pub focal: f32,
    pub aperture: f32,
    pub distance: f32,
    pub k: [f32; 3],
}
impl From<[f32; 6]> for VignettingRow {
    fn from(v: [f32; 6]) -> Self {
        Self {
            focal: v[0],
            aperture: v[1],
            distance: v[2],
            k: [v[3], v[4], v[5]],
        }
    }
}
impl From<VignettingRow> for [f32; 6] {
    fn from(r: VignettingRow) -> Self {
        [r.focal, r.aperture, r.distance, r.k[0], r.k[1], r.k[2]]
    }
}

/// One lens's calibration as Lensfun's database gives it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Calibration {
    /// The crop factor of the camera it was calibrated on.
    pub crop: f32,
    /// That camera's aspect ratio, long edge over short.
    pub aspect: f32,
    #[serde(rename = "type", default, skip_serializing_if = "is_rectilinear")]
    pub lens_type: LensType,
    /// Lensfun's `<center>`: the optical axis off the frame's centre, in units of half
    /// the frame's short edge, y down.
    #[serde(default, skip_serializing_if = "is_zero2")]
    pub centre: [f32; 2],
    /// The lens's focal range, from its `<focal>` or else its calibration: the same
    /// twice for a prime.
    pub focal: [f32; 2],
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub distortion: Vec<DistortionRow>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tca: Vec<TcaRow>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub vignetting: Vec<VignettingRow>,
}
fn is_rectilinear(t: &LensType) -> bool {
    *t == LensType::Rectilinear
}
fn is_zero2(c: &[f32; 2]) -> bool {
    *c == [0., 0.]
}

/// The corrections a calibration holds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Components {
    pub distortion: bool,
    pub chromatic: bool,
    pub vignetting: bool,
}
impl Components {
    pub fn any(self) -> bool {
        self.distortion || self.chromatic || self.vignetting
    }
    /// "distortion, lateral CA and vignetting", for messages.
    pub fn describe(self) -> String {
        let names: Vec<&str> = [
            (self.distortion, "distortion"),
            (self.chromatic, "lateral CA"),
            (self.vignetting, "vignetting"),
        ]
        .into_iter()
        .filter_map(|(has, name)| has.then_some(name))
        .collect();
        match names.as_slice() {
            [] => "nothing".into(),
            [one] => (*one).into(),
            [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
        }
    }
    /// The corrections of a [`LensCorrection`].
    pub fn of(c: &LensCorrection) -> Self {
        Self {
            distortion: c.distortion.is_some(),
            chromatic: c.chromatic.is_some(),
            vignetting: c.vignetting.is_some() || c.gain_map.is_some(),
        }
    }
}

/// The photo frame a correction is made for: the decoded image of `size` pixels, its
/// camera's default crop `rect` (left, top, width, height in pixels), and that crop's
/// crop factor (its diagonal against a 36 × 24 mm frame's).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub size: [f32; 2],
    pub rect: [f32; 4],
    pub crop: f32,
}

/// How a photo was taken: focal length (mm), aperture (f-number) and focus distance
/// (metres); 0 when the photo does not say.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shot {
    pub focal: f32,
    pub aperture: f32,
    pub distance: f32,
}

/// Why a calibration corrects less of a photo than it holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Shortfall {
    /// Not a rectilinear lens.
    Projection(LensType),
    /// The camera's crop factor is smaller than the calibration covers.
    Coverage,
    /// A zoom lens and no focal length in the photo.
    Focal,
    /// No aperture in the photo: no vignetting.
    Aperture,
    /// No vignetting calibrated near this focal length and aperture.
    VignettingCoverage,
    /// A correction this calibration gives here is outside what RAWmakase applies.
    OutOfRange(&'static str),
}
impl Shortfall {
    /// Says it, after "the Lensfun profile …".
    pub fn describe(&self) -> String {
        match self {
            Self::Projection(t) => {
                format!("is for a {} lens, which isn't supported", t.name())
            }
            Self::Coverage => {
                "was calibrated on a camera with a larger crop factor, so it doesn't cover this photo".into()
            }
            Self::Focal => "needs the focal length, which this photo doesn't record".into(),
            Self::Aperture => "can't correct vignetting without the photo's aperture".into(),
            Self::VignettingCoverage => {
                "has no vignetting calibration near this focal length and aperture".into()
            }
            Self::OutOfRange(what) => format!("gives an unusable {what} correction here"),
        }
    }
}

/// A calibration evaluated for one photo.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Evaluation {
    /// What applies; `None` when nothing does.
    pub correction: Option<LensCorrection>,
    /// What it leaves out, and why.
    pub shortfalls: Vec<Shortfall>,
}

impl Calibration {
    /// The corrections it holds.
    pub fn components(&self) -> Components {
        Components {
            distortion: !self.distortion.is_empty(),
            chromatic: !self.tca.is_empty(),
            vignetting: !self.vignetting.is_empty(),
        }
    }
    /// Whether a camera of crop factor `crop` is covered by it.
    pub fn covers(&self, crop: f32) -> bool {
        crop > 0. && self.crop > 0. && crop / self.crop >= CROP_TOLERANCE
    }
    pub fn is_prime(&self) -> bool {
        self.focal[1] <= self.focal[0]
    }
    /// The calibration checked as values: positive crop, aspect and focal lengths and
    /// finite coefficients, as a saved edit must have.
    pub fn validate(&self) -> bool {
        let finite = |v: &[f32]| v.iter().all(|v| v.is_finite());
        self.crop > 0.
            && self.crop.is_finite()
            && self.aspect >= 1.
            && self.aspect.is_finite()
            && finite(&self.centre)
            && self.centre.iter().all(|c| c.abs() <= 1.)
            && self.focal[0] > 0.
            && self.focal[1] >= self.focal[0]
            && self.focal[1].is_finite()
            && self.distortion.len() + self.tca.len() + self.vignetting.len() <= MAX_ROWS
            && self
                .distortion
                .iter()
                .all(|r| r.focal > 0. && finite(&[r.focal]) && finite(&r.model.terms()))
            && self
                .tca
                .iter()
                .all(|r| r.focal > 0. && finite(&[r.focal]) && finite(&r.model.terms()))
            && self.vignetting.iter().all(|r| {
                r.focal > 0. && r.aperture > 0. && r.distance > 0. && finite(&<[f32; 6]>::from(*r))
            })
    }
    /// The focal length to evaluate at: the photo's, else a prime's own.
    fn focal_for(&self, focal: f32) -> Option<f32> {
        if focal > 0. && focal.is_finite() {
            Some(focal)
        } else if self.is_prime() {
            Some(self.focal[0])
        } else {
            None
        }
    }
    /// Distortion at `focal`.
    pub fn distortion_at(&self, focal: f32) -> Option<Distortion> {
        let usable: Vec<DistortionRow> = self
            .distortion
            .iter()
            .filter(|r| r.model.usable())
            .copied()
            .collect();
        let nearest = nearest_focal(&usable, |r| r.focal, focal)?;
        let rows: Vec<(f32, [f32; 3])> = rows_by_focal(
            usable
                .iter()
                .filter(|r| r.model.same_model(&nearest.model))
                .map(|r| (r.focal, r.model.terms().map(|t| t * r.focal))),
        );
        let terms = hermite(&rows, focal, |f| {
            // Held beyond the calibration: the end row as it is.
            usable
                .iter()
                .find(|r| r.focal == f && r.model.same_model(&nearest.model))
                .map(|r| r.model.terms().map(|t| t * focal))
        })?;
        Some(nearest.model.with_terms(terms.map(|t| t / focal)))
    }
    /// Lateral CA at `focal`.
    pub fn tca_at(&self, focal: f32) -> Option<Tca> {
        let nearest = nearest_focal(&self.tca, |r| r.focal, focal)?;
        // `c` and `b` scale with focal length; `k` and `v` don't.
        let scale = |t: [f32; 6], f: f32| [t[0], t[1], t[2] * f, t[3] * f, t[4] * f, t[5] * f];
        let rows: Vec<(f32, [f32; 6])> = rows_by_focal(
            self.tca
                .iter()
                .filter(|r| r.model.same_model(&nearest.model))
                .map(|r| (r.focal, scale(r.model.terms(), r.focal))),
        );
        let terms = hermite(&rows, focal, |f| {
            self.tca
                .iter()
                .find(|r| r.focal == f && r.model.same_model(&nearest.model))
                .map(|r| scale(r.model.terms(), focal))
        })?;
        let unscale = |t: [f32; 6]| {
            [
                t[0],
                t[1],
                t[2] / focal,
                t[3] / focal,
                t[4] / focal,
                t[5] / focal,
            ]
        };
        Some(nearest.model.with_terms(unscale(terms)))
    }
    /// Vignetting's k₁, k₂, k₃ at `focal`, `aperture` and `distance`; `None` when no
    /// calibrated row is near enough.
    pub fn vignetting_at(&self, focal: f32, aperture: f32, distance: f32) -> Option<[f32; 3]> {
        if self.vignetting.is_empty() || aperture <= 0. || distance <= 0. {
            return None;
        }
        let (lo, hi) = self
            .vignetting
            .iter()
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), r| {
                (lo.min(r.focal), hi.max(r.focal))
            });
        let range = if hi > lo { f64::from(hi - lo) } else { 1. };
        let point = |f: f32, n: f32, d: f32| {
            [
                f64::from(f - lo) / range,
                4. / f64::from(n),
                0.1 / f64::from(d),
            ]
        };
        let q = point(focal, aperture, distance);
        let mut weights = 0f64;
        let mut sum = [0f64; 3];
        let mut nearest = f64::INFINITY;
        for r in &self.vignetting {
            let p = point(r.focal, r.aperture, r.distance);
            let d = ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2) + (p[2] - q[2]).powi(2)).sqrt();
            if d < 1e-4 {
                return Some(r.k);
            }
            nearest = nearest.min(d);
            let w = d.powf(-3.5);
            weights += w;
            for (s, k) in sum.iter_mut().zip(r.k) {
                *s += w * f64::from(k);
            }
        }
        (nearest <= 1.).then(|| sum.map(|s| (s / weights) as f32))
    }

    /// The correction for a photo of `frame` taken with `shot`, as far as the
    /// calibration covers it. `source` names it.
    pub fn evaluate(&self, frame: &Frame, shot: &Shot, source: &str) -> Evaluation {
        let mut shortfalls = Vec::new();
        let refuse = |s: Shortfall| Evaluation {
            correction: None,
            shortfalls: vec![s],
        };
        if self.lens_type != LensType::Rectilinear {
            return refuse(Shortfall::Projection(self.lens_type.clone()));
        }
        if !self.covers(frame.crop) {
            return refuse(Shortfall::Coverage);
        }
        let Some(focal) = self.focal_for(shot.focal) else {
            return refuse(Shortfall::Focal);
        };
        let Some(geometry) = Geometry::new(self, frame) else {
            return refuse(Shortfall::Coverage);
        };
        let distortion = self.distortion_at(focal);
        let green = |rho: f64| distortion.map_or(1., |d| d.ratio(geometry.u(rho)));
        let distortion_table = distortion.and_then(|_| {
            let table = sample(
                &green,
                geometry.reach,
                &|rho| rho * geometry.half,
                TOLERANCE_PX,
            );
            if table.is_none() {
                shortfalls.push(Shortfall::OutOfRange("distortion"));
            }
            table
        });
        let chromatic = self.tca_at(focal).and_then(|tca| {
            // Green's source radius reaches farther than the output's at a Distortion
            // amount of up to 200%.
            let steps = 256;
            let widest = (0..=steps)
                .map(|i| {
                    let rho = geometry.reach * f64::from(i) / f64::from(steps);
                    (1. + 2. * (green(rho) - 1.)).max(green(rho))
                })
                .fold(1f64, f64::max);
            let to = geometry.reach * widest * 1.02;
            let channel = |c: usize| {
                sample(
                    &|rho| tca.ratios(geometry.u(rho))[c],
                    to,
                    &|rho| rho * geometry.half,
                    TOLERANCE_PX,
                )
            };
            let tables = channel(0).zip(channel(1)).map(|(r, b)| [r, b]);
            if tables.is_none() {
                shortfalls.push(Shortfall::OutOfRange("lateral CA"));
            }
            tables
        });
        let vignetting = if self.vignetting.is_empty() {
            None
        } else if shot.aperture <= 0. || !shot.aperture.is_finite() {
            shortfalls.push(Shortfall::Aperture);
            None
        } else {
            let distance = if shot.distance > 0. {
                shot.distance
            } else {
                DISTANCE
            };
            match self.vignetting_at(focal, shot.aperture, distance) {
                None => {
                    shortfalls.push(Shortfall::VignettingCoverage);
                    None
                }
                Some(k) => {
                    let k = k.map(f64::from);
                    let falloff = |rho: f64| {
                        let v2 = geometry.v(rho).powi(2);
                        1. + k[0] * v2 + k[1] * v2 * v2 + k[2] * v2 * v2 * v2
                    };
                    let steps = 256;
                    let positive = (0..=steps)
                        .all(|i| falloff(geometry.reach * f64::from(i) / f64::from(steps)) > 0.);
                    let table = positive
                        .then(|| {
                            sample(
                                &|rho| 1. / falloff(rho),
                                geometry.reach,
                                &|_| 1.,
                                TOLERANCE_GAIN,
                            )
                        })
                        .flatten();
                    if table.is_none() {
                        shortfalls.push(Shortfall::OutOfRange("vignetting"));
                    }
                    table
                }
            }
        };
        let correction = LensCorrection {
            source: source.to_string(),
            default_on: false,
            vignetting,
            vignetting_measured: false,
            distortion: distortion_table,
            chromatic,
            chromatic_at_source: true,
            gain_map: None,
            centre: geometry.centre,
        };
        Evaluation {
            correction: (!correction.is_empty() && correction.validate()).then_some(correction),
            shortfalls,
        }
    }
}

/// Most calibration rows a lens may have (Lensfun's largest has about 440).
pub const MAX_ROWS: usize = 4096;

/// The photo's radius (in units of the decoded image's half diagonal) in the
/// calibration's units.
struct Geometry {
    /// The optical centre in fractions of the decoded image.
    centre: [f32; 2],
    /// The decoded image's half diagonal, in pixels.
    half: f64,
    /// Lensfun's distortion unit and vignetting unit per decoded half diagonal.
    per_u: f64,
    per_v: f64,
    /// The decoded image's corner farthest from the centre.
    reach: f64,
}
impl Geometry {
    fn new(c: &Calibration, frame: &Frame) -> Option<Self> {
        let [w, h] = frame.size.map(f64::from);
        let [left, top, fw, fh] = frame.rect.map(f64::from);
        if !(w > 0. && h > 0. && fw > 0. && fh > 0. && frame.crop > 0.) {
            return None;
        }
        let half = w.hypot(h) * 0.5;
        let diagonal = f64::from(FULL_FRAME_DIAGONAL);
        // Millimetres on the sensor per pixel of the photo.
        let mm = diagonal / (2. * f64::from(frame.crop)) / (fw.hypot(fh) * 0.5);
        let lens_crop = f64::from(c.crop);
        let aspect = f64::from(c.aspect);
        let u_mm = diagonal / (2. * lens_crop * (aspect * aspect + 1.).sqrt());
        let v_mm = diagonal / (2. * lens_crop);
        let short = fw.min(fh) * 0.5;
        let cx = left + fw * 0.5 + f64::from(c.centre[0]) * short;
        let cy = top + fh * 0.5 + f64::from(c.centre[1]) * short;
        let centre = [(cx / w) as f32, (cy / h) as f32];
        if !centre.iter().all(|v| (0. ..=1.).contains(v)) {
            return None;
        }
        Some(Self {
            centre,
            half,
            per_u: half * mm / u_mm,
            per_v: half * mm / v_mm,
            reach: f64::from(super::reach(centre, frame.size)),
        })
    }
    fn u(&self, rho: f64) -> f64 {
        rho * self.per_u
    }
    fn v(&self, rho: f64) -> f64 {
        rho * self.per_v
    }
}

/// The row nearest `focal`, whose model the interpolation uses.
fn nearest_focal<T>(rows: &[T], focal_of: impl Fn(&T) -> f32, focal: f32) -> Option<&T> {
    rows.iter().min_by(|a, b| {
        (focal_of(a) - focal)
            .abs()
            .total_cmp(&(focal_of(b) - focal).abs())
    })
}
/// Rows sorted by focal length, the first of any repeated one kept.
fn rows_by_focal<const N: usize>(
    rows: impl Iterator<Item = (f32, [f32; N])>,
) -> Vec<(f32, [f32; N])> {
    let mut rows: Vec<_> = rows.collect();
    rows.sort_by(|a, b| a.0.total_cmp(&b.0));
    rows.dedup_by(|b, a| a.0 == b.0);
    rows
}
/// The cubic Hermite spline through `rows` at `focal`, tangents taken from the
/// neighbouring rows by index (from the pair itself where one is missing). Beyond the
/// first or last row, `held` gives that row's values.
fn hermite<const N: usize>(
    rows: &[(f32, [f32; N])],
    focal: f32,
    held: impl Fn(f32) -> Option<[f32; N]>,
) -> Option<[f32; N]> {
    let (first, last) = (rows.first()?, rows.last()?);
    if focal <= first.0 {
        return held(first.0);
    }
    if focal >= last.0 {
        return held(last.0);
    }
    let i = rows.partition_point(|r| r.0 <= focal) - 1;
    let (f1, y1) = rows[i];
    let (f2, y2) = rows[i + 1];
    let y0 = i.checked_sub(1).map(|j| rows[j].1);
    let y3 = rows.get(i + 2).map(|r| r.1);
    let t = f64::from((focal - f1) / (f2 - f1));
    let (t2, t3) = (t * t, t * t * t);
    let h00 = 2. * t3 - 3. * t2 + 1.;
    let h10 = t3 - 2. * t2 + t;
    let h01 = -2. * t3 + 3. * t2;
    let h11 = t3 - t2;
    Some(std::array::from_fn(|k| {
        let (a, b) = (f64::from(y1[k]), f64::from(y2[k]));
        let m1 = y0.map_or(b - a, |y0| (b - f64::from(y0[k])) * 0.5);
        let m2 = y3.map_or(b - a, |y3| (f64::from(y3[k]) - a) * 0.5);
        (h00 * a + h10 * m1 + h01 * b + h11 * m2) as f32
    }))
}

/// `f` sampled on [0, `to`] as a table whose linear interpolation strays from it by
/// at most `tolerance`, weighted by `weight` (pixels per unit of `f` at a radius: a
/// ratio's error times the radius in pixels is how far a sample moves), within
/// [`Radial::MAX_KNOTS`]. Knots start spread by the square root of the weighted
/// curvature, which evens out the linear interpolation's error h²|f''|/8, and a
/// stretch that still strays is halved until none does. `None` when it can't be a
/// valid table.
fn sample(
    f: &dyn Fn(f64) -> f64,
    to: f64,
    weight: &dyn Fn(f64) -> f64,
    tolerance: f64,
) -> Option<Radial> {
    if !(to > 0. && to.is_finite()) {
        return None;
    }
    const FINE: usize = 512;
    let step = to / FINE as f64;
    let x = |i: usize| i as f64 * step;
    let fine: Vec<f64> = (0..=FINE).map(|i| f(x(i))).collect();
    if !fine.iter().all(|v| v.is_finite()) {
        return None;
    }
    let mut density = vec![0f64; FINE + 1];
    for i in 1..FINE {
        let bend = (fine[i - 1] - 2. * fine[i] + fine[i + 1]).abs() / (step * step);
        density[i] = (bend * weight(x(i)).max(0.) / (8. * tolerance)).sqrt();
    }
    density[0] = density[1];
    density[FINE] = density[FINE - 1];
    // A floor, so a stretch where the curve barely bends (an inflection) still gets
    // knots near its bending ends.
    let mean = density.iter().sum::<f64>() / density.len() as f64;
    density.iter_mut().for_each(|d| *d += 0.2 * mean);
    let mut cumulative = vec![0f64; FINE + 1];
    for i in 1..=FINE {
        cumulative[i] = cumulative[i - 1] + 0.5 * (density[i - 1] + density[i]) * step;
    }
    let total = cumulative[FINE];
    let n = (total.ceil() as usize + 2).clamp(2, Radial::MAX_KNOTS / 2);
    let mut knots: Vec<f64> = (0..n)
        .map(|k| {
            let target = total * k as f64 / (n - 1) as f64;
            let i = cumulative.partition_point(|c| *c < target).clamp(1, FINE);
            let (c0, c1) = (cumulative[i - 1], cumulative[i]);
            let t = if c1 > c0 {
                ((target - c0) / (c1 - c0)).clamp(0., 1.)
            } else {
                0.
            };
            x(i - 1) + t * step
        })
        .collect();
    knots[0] = 0.;
    knots[n - 1] = to;
    knots.dedup_by(|b, a| *b <= *a);
    // How far the line between two knots strays from `f` inside them.
    let strays = |a: f64, b: f64| {
        let (fa, fb) = (f(a), f(b));
        [0.25, 0.5, 0.75].iter().any(|t| {
            let at = a + (b - a) * t;
            (fa + (fb - fa) * t - f(at)).abs() * weight(at).max(0.) > tolerance
        })
    };
    loop {
        let mut refined = Vec::with_capacity(knots.len() * 2);
        refined.push(knots[0]);
        let mut split = false;
        for (i, pair) in knots.windows(2).enumerate() {
            let room = refined.len() + (knots.len() - i) < Radial::MAX_KNOTS;
            if room && strays(pair[0], pair[1]) {
                refined.push((pair[0] + pair[1]) * 0.5);
                split = true;
            }
            refined.push(pair[1]);
        }
        knots = refined;
        if !split {
            break;
        }
    }
    // Out of knots before every stretch followed the model: refused if it strays by
    // more than twice the tolerance anywhere.
    let worst = knots
        .windows(2)
        .flat_map(|pair| {
            let (a, b) = (pair[0], pair[1]);
            let (fa, fb) = (f(a), f(b));
            (1..8).map(move |i| (a, b, fa, fb, f64::from(i) / 8.))
        })
        .map(|(a, b, fa, fb, t)| {
            let at = a + (b - a) * t;
            (fa + (fb - fa) * t - f(at)).abs() * weight(at).max(0.)
        })
        .fold(0f64, f64::max);
    if worst > 2. * tolerance {
        return None;
    }
    let mut table = Radial {
        knots: knots.iter().map(|k| *k as f32).collect(),
        values: knots.iter().map(|k| f(*k) as f32).collect(),
    };
    // Knots too close for f32 to tell apart.
    let mut i = 1;
    while i < table.knots.len() {
        if table.knots[i] <= table.knots[i - 1] {
            table.knots.remove(i);
            table.values.remove(i);
        } else {
            i += 1;
        }
    }
    table.valid().then_some(table)
}

#[cfg(test)]
mod tests;
