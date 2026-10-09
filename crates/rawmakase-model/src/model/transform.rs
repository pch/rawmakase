//! The Transform panel's settings as a recipe stores them: the manual sliders and
//! Lightroom's Upright, with the corrections it analysed and the guides drawn for
//! Guided. Rendering them, and analysing or solving Upright, is `develop`'s.
use serde::{Deserialize, Serialize};

/// Lightroom's Transform panel (manual sliders). Applied after lens correction and
/// before crop, in the oriented frame. Slider values are Lightroom's divided by 100,
/// except `rotate` (degrees) and `scale` (1 = 100%).
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Transform {
    pub vertical: f32,
    pub horizontal: f32,
    pub rotate: f32,
    pub aspect: f32,
    pub scale: f32,
    pub offset_x: f32,
    pub offset_y: f32,
}
impl Default for Transform {
    fn default() -> Self {
        Self {
            vertical: 0.,
            horizontal: 0.,
            rotate: 0.,
            aspect: 0.,
            scale: 1.,
            offset_x: 0.,
            offset_y: 0.,
        }
    }
}
impl Transform {
    /// The sliders as Lightroom shows them on the displayed photo, when `self` holds
    /// them as stored, in the recorded frame (`m` from [`display_axes`]). Lightroom
    /// stores Vertical −70 on a photo turned 90° left as `PerspectiveHorizontal` +70.
    pub fn displayed(&self, m: [[f32; 2]; 2]) -> Self {
        self.reoriented([[m[0][0], m[1][0]], [m[0][1], m[1][1]]])
    }
    /// The stored sliders for sliders shown on the displayed photo: the inverse of
    /// [`Self::displayed`].
    pub fn recorded(&self, m: [[f32; 2]; 2]) -> Self {
        self.reoriented(m)
    }
    /// Perspective (h, v) and offsets (x, −y) are vectors in the frame; a swap of axes
    /// flips Aspect, and a mirror flips Rotate.
    fn reoriented(&self, m: [[f32; 2]; 2]) -> Self {
        let map = |[x, y]: [f32; 2]| [m[0][0] * x + m[0][1] * y, m[1][0] * x + m[1][1] * y];
        let [horizontal, vertical] = map([self.horizontal, self.vertical]);
        let [offset_x, minus_y] = map([self.offset_x, -self.offset_y]);
        let determinant = m[0][0] * m[1][1] - m[0][1] * m[1][0];
        Self {
            vertical,
            horizontal,
            rotate: self.rotate * determinant,
            aspect: if m[0][0] == 0. {
                -self.aspect
            } else {
                self.aspect
            },
            scale: self.scale,
            offset_x,
            offset_y: -minus_y,
        }
    }
    pub fn is_identity(&self) -> bool {
        *self == Self::default()
    }
    pub fn validate(&self) -> bool {
        [
            self.vertical,
            self.horizontal,
            self.aspect,
            self.offset_x,
            self.offset_y,
        ]
        .iter()
        .all(|v| v.is_finite() && v.abs() <= 1.)
            && self.rotate.is_finite()
            && self.rotate.abs() <= 10.
            && (0.5..=1.5).contains(&self.scale)
    }
}

/// Lightroom's limit on guides.
pub const MAX_GUIDES: usize = 4;

/// How the displayed photo's axes lie in the frame the camera recorded: `m` maps a
/// displayed direction (x right, y down) to a recorded one. Its entries are 0 or ±1.
pub fn display_axes(turns: u8, flip_x: bool, flip_y: bool) -> [[f32; 2]; 2] {
    let recorded = |x: f32, y: f32| {
        let x = if flip_x { 1. - x } else { x };
        let y = if flip_y { 1. - y } else { y };
        super::image_frame::turn(turns, x, y)
    };
    let [ox, oy] = recorded(0., 0.);
    let [xx, xy] = recorded(1., 0.);
    let [yx, yy] = recorded(0., 1.);
    [[xx - ox, yx - ox], [xy - oy, yy - oy]]
}
/// The lens settings an analysis is made through, as they render: when any of them
/// changes, the corrections analysed before no longer fit the photo. A setting a
/// switched-off panel leaves unrendered changes nothing.
#[derive(Clone, Debug, PartialEq)]
pub struct LensInputs {
    builtin: bool,
    profile: bool,
    profile_choice: crate::lens::choice::LensProfileChoice,
    distortion: f32,
    manual_distortion: f32,
}
impl LensInputs {
    pub fn of(r: &super::recipe::Recipe) -> Self {
        let shown = r.as_rendered();
        Self {
            builtin: shown.lens_builtin,
            profile: shown.lens_profile,
            profile_choice: if shown.lens_profile {
                shown.lens_profile_choice.rendering()
            } else {
                Default::default()
            },
            distortion: shown.lens_distortion,
            manual_distortion: shown.lens_manual_distortion,
        }
    }
}

/// Lightroom's Upright modes, in Adobe's `crs:PerspectiveUpright` order.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum UprightMode {
    #[default]
    Off,
    Auto,
    Full,
    Level,
    Vertical,
    Guided,
}
impl UprightMode {
    pub const ALL: [Self; 6] = [
        Self::Off,
        Self::Auto,
        Self::Full,
        Self::Level,
        Self::Vertical,
        Self::Guided,
    ];
    /// Adobe's `crs:PerspectiveUpright` value, which also indexes `UprightTransform_N`.
    pub fn code(self) -> usize {
        self as usize
    }
    pub fn from_code(code: usize) -> Option<Self> {
        Self::ALL.get(code).copied()
    }
}
/// Lightroom's Upright: the chosen mode and the correction for each mode, as Lightroom
/// stores them so that switching modes needs no new analysis.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, from = "StoredUpright")]
pub struct Upright {
    pub mode: UprightMode,
    /// Forward (source-to-output) homographies indexed by [`UprightMode::code`], row
    /// major, in 0–1 coordinates of the photo as the camera recorded it, before any
    /// rotation or flip and after the camera's default crop. This is how Lightroom
    /// stores `crs:UprightTransform_N`; Camera Raw renders them exactly (docs/transform.md).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub corrections: Vec<[f32; 9]>,
    /// Guided Upright's guides, drawn on this photo; `corrections` holds what they
    /// solve to.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub guides: Vec<UprightGuide>,
    /// Lightroom's other Upright settings (analysis centre, focal length, version),
    /// kept to write back unchanged.
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub lightroom: std::collections::BTreeMap<String, String>,
}
/// [`Upright`] as saved. Edits saved before guides were editable kept Lightroom's
/// guides among its other settings; they become guides when read.
#[derive(Deserialize)]
#[serde(default)]
#[derive(Default)]
struct StoredUpright {
    mode: UprightMode,
    corrections: Vec<[f32; 9]>,
    guides: Vec<UprightGuide>,
    lightroom: std::collections::BTreeMap<String, String>,
}
impl From<StoredUpright> for Upright {
    fn from(s: StoredUpright) -> Self {
        let StoredUpright {
            mode,
            corrections,
            mut guides,
            mut lightroom,
        } = s;
        if guides.is_empty() {
            let mut found: Vec<(usize, UprightGuide)> = lightroom
                .iter()
                .filter_map(|(key, value)| {
                    let i = key.strip_prefix("UprightFourSegments_")?.parse().ok()?;
                    Some((i, parse_guide(value)?))
                })
                .collect();
            found.sort_by_key(|(i, _)| *i);
            guides = found.into_iter().map(|(_, g)| g).take(MAX_GUIDES).collect();
        }
        lightroom.retain(|key, _| !key.starts_with("UprightFourSegments"));
        Self {
            mode,
            corrections,
            guides,
            lightroom,
        }
    }
}
/// A Guided Upright guide: a line drawn along an edge that should be vertical or
/// horizontal. Its ends are in 0–1 coordinates of the photo as recorded, where Upright
/// applies: after lens corrections, before the photo is turned or flipped for display
/// and before Upright itself, so the guide stays on the edge it was drawn along
/// whatever the correction (docs/transform.md#guided-upright).
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct UprightGuide {
    pub a: [f32; 2],
    pub b: [f32; 2],
}
impl Upright {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
    /// Keeps the mode but drops what was analysed from one photo, for settings moving
    /// to another: the corrections, and Lightroom's analysis details.
    pub fn clear_analysis(&mut self) {
        self.corrections.clear();
        self.lightroom.clear();
        self.guides.clear();
        // Guided can't be analysed again without its guides.
        if self.mode == UprightMode::Guided {
            self.mode = UprightMode::Off;
        }
    }
    /// Drops the corrections analysed through lens settings that changed since, for a new
    /// analysis of the same photo. Guided keeps its guides, to solve again; without
    /// guides it has nothing to solve from and turns Off.
    pub fn analyse_again(&mut self) {
        self.corrections.clear();
        self.lightroom.clear();
        if self.mode == UprightMode::Guided && self.guides.is_empty() {
            self.mode = UprightMode::Off;
        }
    }
    /// Whether the mode needs an analysis (or, for Guided, its guides solved) that is
    /// not there yet.
    pub fn needs_analysis(&self) -> bool {
        let missing = self.corrections.len() <= self.mode.code();
        match self.mode {
            UprightMode::Off => false,
            UprightMode::Guided => missing && !self.guides.is_empty(),
            _ => missing,
        }
    }
    pub fn validate(&self) -> bool {
        self.corrections.len() <= UprightMode::ALL.len()
            && self.guides.len() <= MAX_GUIDES
            && self
                .guides
                .iter()
                .all(|g| g.a.iter().chain(&g.b).all(|v| v.is_finite()))
            && self.corrections.iter().all(|m| {
                let [a, b, c, d, e, f, g, h, i] = *m;
                let determinant = a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g);
                // Rendering inverts it; a singular one would silently render as Off.
                m.iter().all(|v| v.is_finite()) && determinant.abs() > 1e-6
            })
    }
    /// The chosen mode's correction, when it is not the identity.
    pub fn correction(&self) -> Option<[[f32; 3]; 3]> {
        if self.mode == UprightMode::Off {
            return None;
        }
        let m = self.corrections.get(self.mode.code())?;
        let m: [[f32; 3]; 3] = std::array::from_fn(|i| std::array::from_fn(|j| m[3 * i + j]));
        (m != IDENTITY && m[2][2] != 0.).then_some(m)
    }
}

/// A guide as Camera Raw writes `UprightFourSegments_N`: "x1,y1,x2,y2", 0–1
/// coordinates separated by commas; None unless it is four finite numbers.
pub fn parse_guide(value: &str) -> Option<UprightGuide> {
    let v: Vec<f32> = value
        .split(',')
        .map(|x| x.trim().parse::<f32>())
        .collect::<Result<_, _>>()
        .ok()?;
    let [x1, y1, x2, y2]: [f32; 4] = v.try_into().ok()?;
    [x1, y1, x2, y2]
        .iter()
        .all(|x| x.is_finite())
        .then_some(UprightGuide {
            a: [x1, y1],
            b: [x2, y2],
        })
}

const IDENTITY: [[f32; 3]; 3] = [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]];

#[cfg(test)]
mod upright_tests {
    use super::*;
    #[test]
    fn clearing_the_analysis_turns_guided_off() {
        let mut u = Upright {
            mode: UprightMode::Guided,
            corrections: vec![[1., 0., 0., 0., 1., 0., 0., 0., 1.]; 6],
            ..Default::default()
        };
        u.clear_analysis();
        assert_eq!(u.mode, UprightMode::Off);
        u.mode = UprightMode::Vertical;
        u.clear_analysis();
        assert_eq!(u.mode, UprightMode::Vertical);
    }
}
#[cfg(test)]
mod singular_tests {
    use super::*;
    #[test]
    fn singular_upright_corrections_are_invalid() {
        let mut u = Upright {
            mode: UprightMode::Level,
            corrections: vec![[1., 0., 0., 0., 1., 0., 0., 0., 1.]; 4],
            ..Default::default()
        };
        assert!(u.validate());
        u.corrections[3] = [1., 0., 0., 0., 0., 0., 0., 0., 1.];
        assert!(!u.validate());
    }
}

#[cfg(test)]
mod stored_tests {
    use super::*;

    #[test]
    fn transform_keeps_its_stored_form_and_limits() {
        let stored = serde_json::to_value(Transform::default()).unwrap();
        assert_eq!(
            stored,
            serde_json::json!({
                "vertical": 0.0, "horizontal": 0.0, "rotate": 0.0, "aspect": 0.0,
                "scale": 1.0, "offset_x": 0.0, "offset_y": 0.0,
            })
        );
        let partial: Transform = serde_json::from_str(r#"{"rotate": -10}"#).unwrap();
        assert_eq!((partial.rotate, partial.scale), (-10., 1.));
        assert!(partial.validate());
        assert!(serde_json::from_str::<Transform>(r#"{"unknown": 1}"#).is_err());
        let with = |edit: fn(&mut Transform)| {
            let mut t = Transform::default();
            edit(&mut t);
            t.validate()
        };
        assert!(with(|t| t.scale = 0.5) && with(|t| t.scale = 1.5) && with(|t| t.vertical = -1.));
        assert!(!with(|t| t.rotate = 10.5));
        assert!(!with(|t| t.scale = 1.6));
        assert!(!with(|t| t.offset_x = 1.1));
    }

    #[test]
    fn upright_keeps_its_stored_form() {
        let names: Vec<String> = UprightMode::ALL
            .iter()
            .map(|m| {
                serde_json::to_value(m)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect();
        assert_eq!(
            names,
            ["off", "auto", "full", "level", "vertical", "guided"]
        );
        assert_eq!(UprightMode::from_code(3), Some(UprightMode::Level));
        // Empty analysis is not written.
        assert_eq!(
            serde_json::to_string(&Upright::default()).unwrap(),
            r#"{"mode":"off"}"#
        );
        let guide = UprightGuide {
            a: [0.1, 0.2],
            b: [0.1, 0.9],
        };
        let tilt = [1., 0.1, 0., 0., 1., 0., 0., 0.05, 1.];
        let guided = Upright {
            mode: UprightMode::Guided,
            corrections: vec![tilt],
            guides: vec![guide],
            ..Default::default()
        };
        assert_eq!(
            serde_json::to_value(&guided).unwrap(),
            serde_json::json!({
                "mode": "guided",
                "corrections": [[1.0, 0.1_f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.05_f32, 1.0]],
                "guides": [{"a": [0.1_f32, 0.2_f32], "b": [0.1_f32, 0.9_f32]}],
            })
        );
        assert_eq!(MAX_GUIDES, 4);
    }

    #[test]
    fn lightroom_guides_saved_among_its_settings_become_guides() {
        let stored = r#"{"mode": "guided", "lightroom": {
            "UprightFourSegments_1": "0.5, 0.1, 0.5, 0.9",
            "UprightFourSegments_0": "0.1,0.2,0.1,0.9",
            "UprightFourSegments_2": "not a guide",
            "UprightFocalLength35mm": "24"}}"#;
        let u: Upright = serde_json::from_str(stored).unwrap();
        assert_eq!(
            u.guides,
            [
                UprightGuide {
                    a: [0.1, 0.2],
                    b: [0.1, 0.9]
                },
                UprightGuide {
                    a: [0.5, 0.1],
                    b: [0.5, 0.9]
                },
            ]
        );
        assert_eq!(
            u.lightroom.keys().collect::<Vec<_>>(),
            ["UprightFocalLength35mm"]
        );
    }
}
