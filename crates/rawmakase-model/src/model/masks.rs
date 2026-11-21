//! Masks as a recipe stores them (Lightroom's Masking panel): named groups of brush,
//! gradient and range components, each group with its own local adjustment, and
//! their limits. Positions are in image space; sizes are fractions of the long edge.
//! Rendering their weights is `develop::masks`'s.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Most masks per photo; the GPU carries one weight byte per mask and pixel.
pub const MAX_GROUPS: usize = 16;
pub const MAX_COMPONENTS: usize = 32;
pub const MAX_STROKES: usize = 2000;
pub const MAX_POINTS: usize = 4096;

/// A named mask with its adjustment.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct MaskGroup {
    pub name: String,
    pub components: Vec<MaskComponent>,
    pub invert: bool,
    /// Lightroom's mask Amount: scales every slider, 0–2 (1 = 100).
    pub amount: f32,
    pub adjust: LocalAdjust,
    /// Hidden masks are kept but not rendered (the eye icon in Lightroom's mask list).
    pub hidden: bool,
}
impl Default for MaskGroup {
    fn default() -> Self {
        Self {
            name: String::new(),
            components: Vec::new(),
            invert: false,
            amount: 1.,
            adjust: LocalAdjust::default(),
            hidden: false,
        }
    }
}
/// How a component combines with the components above it.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum MaskOp {
    /// Union: the larger weight.
    #[default]
    Add,
    /// The weight so far minus this one, not below zero.
    Subtract,
    /// The smaller weight.
    Intersect,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MaskComponent {
    pub op: MaskOp,
    pub invert: bool,
    /// 0–1.
    pub opacity: f32,
    pub shape: MaskShape,
}
impl MaskComponent {
    pub fn new(shape: MaskShape) -> Self {
        Self {
            op: MaskOp::Add,
            invert: false,
            opacity: 1.,
            shape,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum MaskShape {
    Brush {
        strokes: Vec<BrushStroke>,
    },
    /// Full effect at `from`, none at `to`, with a smooth transition between.
    Linear {
        from: [f32; 2],
        to: [f32; 2],
    },
    /// An ellipse with radii as fractions of the long edge, rotated by `angle`
    /// degrees. `feather` (0–1) is the share of the radius that fades out.
    Radial {
        center: [f32; 2],
        radii: [f32; 2],
        angle: f32,
        feather: f32,
    },
    /// Colors near the sampled ones (Oklab of the developed display color).
    /// `amount` (0–1) is Lightroom's Refine: lower selects fewer colors.
    ColorRange {
        samples: Vec<[f32; 3]>,
        amount: f32,
    },
    /// Display luminance between `low` and `high` (0–1), fading over `falloff`
    /// below and above.
    LuminanceRange {
        low: f32,
        high: f32,
        falloff: [f32; 2],
    },
    /// Coverage painted by a model or imported, stored once in the bitmap store and
    /// named by its content ID (see [`crate::storage::mask_assets`]).
    Bitmap(BitmapMask),
    // Later: Depth { low, high, falloff } from a depth map.
}
/// The sampling contract of a [`BitmapMask`]: its raster covers exactly the
/// camera-oriented default-crop image frame, normalised to `[0,1]²` before lens
/// correction, Transform, crop, straightening and flips; coverage is bilinear at
/// pixel centres, clamped at the raster's edge and zero outside the frame.
pub const BITMAP_SAMPLING: u32 = 1;
/// Raster coverage as a mask component.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct BitmapMask {
    /// Content ID of the 8-bit coverage raster.
    pub id: String,
    pub width: u32,
    pub height: u32,
    /// [`BITMAP_SAMPLING`] when it was made.
    pub sampling: u32,
    /// What made it, when a model did; not needed to render it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<BitmapSource>,
}
/// Which feature generated a raster, and from which inputs.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct BitmapSource {
    /// `subject` or `sky`.
    pub feature: String,
    /// The model's id and the digest of its file.
    pub model: String,
    /// A hash of everything the model was given, for telling whether a regeneration
    /// would differ.
    pub input: String,
}
pub const FEATURE_SUBJECT: &str = "subject";
pub const FEATURE_SKY: &str = "sky";
impl MaskShape {
    pub fn kind(&self) -> &'static str {
        match self {
            MaskShape::Brush { .. } => "Brush",
            MaskShape::Linear { .. } => "Linear Gradient",
            MaskShape::Radial { .. } => "Radial Gradient",
            MaskShape::ColorRange { .. } => "Color Range",
            MaskShape::LuminanceRange { .. } => "Luminance Range",
            MaskShape::Bitmap(b) => match b.source.as_ref().map(|s| s.feature.as_str()) {
                Some(FEATURE_SUBJECT) => "Subject",
                Some(FEATURE_SKY) => "Sky",
                _ => "Bitmap",
            },
        }
    }
    /// Whether the weight depends on the developed photo, not only on the position.
    pub fn is_range(&self) -> bool {
        matches!(
            self,
            MaskShape::ColorRange { .. } | MaskShape::LuminanceRange { .. }
        )
    }
}
/// One brush stroke: dabs along a path.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BrushStroke {
    pub points: Arc<[[f32; 2]]>,
    /// Fraction of the long edge.
    pub radius: f32,
    /// Share of the radius that fades out, 0–1.
    pub feather: f32,
    /// How much each dab adds, 0–1; overlapping dabs build up to `density`.
    pub flow: f32,
    /// Most weight the stroke reaches, 0–1.
    pub density: f32,
    /// Removes weight instead of adding it (Lightroom's Erase brush).
    pub erase: bool,
    /// Limits dabs to areas similar to the color under the brush centre (Auto Mask).
    pub auto_mask: bool,
}
/// A mask's adjustment. Sliders are Lightroom's divided by 100, except Exposure (EV),
/// Hue (degrees) and Temp/Tint (Lightroom's local ±100 as ±1).
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct LocalAdjust {
    pub temperature: f32,
    pub tint: f32,
    pub exposure: f32,
    pub contrast: f32,
    pub highlights: f32,
    pub shadows: f32,
    pub whites: f32,
    pub blacks: f32,
    pub texture: f32,
    pub clarity: f32,
    pub dehaze: f32,
    pub hue: f32,
    pub saturation: f32,
    pub sharpness: f32,
    pub noise: f32,
    /// Lightroom's Color swatch: hue (0–1) and saturation (0–1).
    pub color: [f32; 2],
}
impl LocalAdjust {
    pub fn is_neutral(&self) -> bool {
        let mut a = *self;
        if a.color[1] == 0. {
            a.color = [0.; 2];
        }
        a == Self::default()
    }
    pub fn validate(&self) -> Result<()> {
        let unit = |v: f32| v.is_finite() && v.abs() <= 1.;
        ensure!(
            [
                self.temperature,
                self.tint,
                self.contrast,
                self.highlights,
                self.shadows,
                self.whites,
                self.blacks,
                self.texture,
                self.clarity,
                self.dehaze,
                self.saturation,
                self.sharpness,
                self.noise,
            ]
            .into_iter()
            .all(unit)
                && self.exposure.is_finite()
                && self.exposure.abs() <= 4.
                && self.hue.is_finite()
                && self.hue.abs() <= 180.
                && self
                    .color
                    .iter()
                    .all(|v| v.is_finite() && (0. ..=1.).contains(v)),
            "Invalid mask adjustment"
        );
        Ok(())
    }
}
fn position(p: &[f32; 2]) -> bool {
    p.iter().all(|v| v.is_finite() && v.abs() <= 4.)
}
fn unit(v: f32) -> bool {
    v.is_finite() && (0. ..=1.).contains(&v)
}
impl BrushStroke {
    fn validate(&self) -> Result<()> {
        ensure!(
            !self.points.is_empty()
                && self.points.len() <= MAX_POINTS
                && self.points.iter().all(position)
                && self.radius.is_finite()
                && (1e-4..=0.5).contains(&self.radius)
                && unit(self.feather)
                && unit(self.flow)
                && unit(self.density),
            "Invalid brush stroke"
        );
        Ok(())
    }
}
impl MaskShape {
    fn validate(&self) -> Result<()> {
        match self {
            MaskShape::Brush { strokes } => {
                ensure!(strokes.len() <= MAX_STROKES, "Too many brush strokes");
                strokes.iter().try_for_each(BrushStroke::validate)?;
            }
            MaskShape::Linear { from, to } => {
                ensure!(position(from) && position(to), "Invalid linear gradient")
            }
            MaskShape::Radial {
                center,
                radii,
                angle,
                feather,
            } => ensure!(
                position(center)
                    && radii
                        .iter()
                        .all(|r| r.is_finite() && (1e-4..=4.).contains(r))
                    && angle.is_finite()
                    && angle.abs() <= 360.
                    && unit(*feather),
                "Invalid radial gradient"
            ),
            MaskShape::ColorRange { samples, amount } => ensure!(
                !samples.is_empty()
                    && samples.len() <= 5
                    && samples
                        .iter()
                        .flatten()
                        .all(|v| v.is_finite() && v.abs() <= 2.)
                    && unit(*amount),
                "Invalid color range"
            ),
            MaskShape::LuminanceRange { low, high, falloff } => ensure!(
                unit(*low) && unit(*high) && low <= high && falloff.iter().all(|f| unit(*f)),
                "Invalid luminance range"
            ),
            MaskShape::Bitmap(b) => ensure!(
                crate::storage::bitmaps::Bitmap::is_valid_id(&b.id)
                    && (1..=crate::storage::mask_assets::MAX_SIDE).contains(&b.width)
                    && (1..=crate::storage::mask_assets::MAX_SIDE).contains(&b.height)
                    && b.sampling == BITMAP_SAMPLING,
                "Invalid bitmap mask"
            ),
        }
        Ok(())
    }
}
impl MaskGroup {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.components.len() <= MAX_COMPONENTS,
            "Too many mask components"
        );
        ensure!(
            self.amount.is_finite() && (0. ..=2.).contains(&self.amount),
            "Invalid mask amount"
        );
        ensure!(self.name.len() <= 256, "Mask name too long");
        for c in &self.components {
            ensure!(unit(c.opacity), "Invalid mask opacity");
            c.shape.validate()?;
        }
        self.adjust.validate()
    }
    /// Whether the mask renders at all: visible, with components and an adjustment.
    pub fn is_active(&self) -> bool {
        !self.hidden && !self.components.is_empty() && self.amount > 0. && !self.adjust.is_neutral()
    }
}
/// The content IDs of the rasters `groups` refer to, in order, with repeats.
pub fn bitmap_ids(groups: &[MaskGroup]) -> impl Iterator<Item = &str> {
    groups
        .iter()
        .flat_map(|g| &g.components)
        .filter_map(|c| match &c.shape {
            MaskShape::Bitmap(b) => Some(b.id.as_str()),
            _ => None,
        })
}
impl MaskGroup {
    /// Whether any component is a raster, which belongs to the photo it was made
    /// from and cannot travel in a preset or to another photo.
    pub fn has_raster(&self) -> bool {
        self.components
            .iter()
            .any(|c| matches!(c.shape, MaskShape::Bitmap(_)))
    }
}
/// The rasters `groups` refer to, as they describe them: content ID, width, height.
pub fn bitmap_refs(groups: &[MaskGroup]) -> impl Iterator<Item = (&str, u32, u32)> {
    groups
        .iter()
        .flat_map(|g| &g.components)
        .filter_map(|c| match &c.shape {
            MaskShape::Bitmap(b) => Some((b.id.as_str(), b.width, b.height)),
            _ => None,
        })
}
pub fn validate(groups: &[MaskGroup]) -> Result<()> {
    ensure!(groups.len() <= MAX_GROUPS, "Too many masks");
    groups.iter().try_for_each(MaskGroup::validate)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_keep_their_stored_form() {
        let stroke = BrushStroke {
            points: vec![[0.1, 0.2]].into(),
            radius: 0.05,
            feather: 0.5,
            flow: 1.,
            density: 1.,
            erase: false,
            auto_mask: true,
        };
        let mut group = MaskGroup {
            name: "Sky".into(),
            components: [
                MaskShape::Brush {
                    strokes: vec![stroke],
                },
                MaskShape::Linear {
                    from: [0., 0.],
                    to: [0., 0.5],
                },
                MaskShape::Radial {
                    center: [0.5, 0.5],
                    radii: [0.25, 0.5],
                    angle: 0.,
                    feather: 0.5,
                },
                MaskShape::ColorRange {
                    samples: vec![[0.5, 0., 0.]],
                    amount: 0.5,
                },
                MaskShape::LuminanceRange {
                    low: 0.,
                    high: 0.5,
                    falloff: [0., 0.25],
                },
            ]
            .map(MaskComponent::new)
            .into(),
            ..Default::default()
        };
        group.components[1].op = MaskOp::Subtract;
        group.adjust.exposure = 0.5;
        let json = serde_json::to_value(&group).unwrap();
        let component = |op: &str, shape: serde_json::Value| serde_json::json!({"op": op, "invert": false, "opacity": 1.0, "shape": shape});
        let adjust = serde_json::json!({
            "temperature": 0.0, "tint": 0.0, "exposure": 0.5, "contrast": 0.0,
            "highlights": 0.0, "shadows": 0.0, "whites": 0.0, "blacks": 0.0,
            "texture": 0.0, "clarity": 0.0, "dehaze": 0.0, "hue": 0.0,
            "saturation": 0.0, "sharpness": 0.0, "noise": 0.0, "color": [0.0, 0.0],
        });
        let expected = serde_json::json!({
            "name": "Sky",
            "components": [
                component("Add", serde_json::json!({"Brush": {"strokes": [{
                    "points": [[0.1_f32, 0.2_f32]], "radius": 0.05_f32, "feather": 0.5,
                    "flow": 1.0, "density": 1.0, "erase": false, "auto_mask": true,
                }]}})),
                component("Subtract", serde_json::json!({"Linear": {"from": [0.0, 0.0], "to": [0.0, 0.5]}})),
                component("Add", serde_json::json!({"Radial": {
                    "center": [0.5, 0.5], "radii": [0.25, 0.5], "angle": 0.0, "feather": 0.5,
                }})),
                component("Add", serde_json::json!({"ColorRange": {"samples": [[0.5, 0.0, 0.0]], "amount": 0.5}})),
                component("Add", serde_json::json!({"LuminanceRange": {
                    "low": 0.0, "high": 0.5, "falloff": [0.0, 0.25],
                }})),
            ],
            "invert": false,
            "amount": 1.0,
            "adjust": adjust,
            "hidden": false,
        });
        assert_eq!(json, expected);
        assert_eq!(serde_json::from_value::<MaskGroup>(json).unwrap(), group);
        assert!(validate(&[group]).is_ok());
        // A group saved with only some settings takes the defaults; unknown ones fail.
        let stored: MaskGroup = serde_json::from_str(r#"{"name": "Old"}"#).unwrap();
        assert_eq!(
            (stored.amount, stored.hidden, stored.adjust),
            (1., false, LocalAdjust::default())
        );
        assert!(serde_json::from_str::<MaskGroup>(r#"{"unknown": 1}"#).is_err());
        assert!(serde_json::from_str::<LocalAdjust>(r#"{"unknown": 1}"#).is_err());
    }

    #[test]
    fn validation_keeps_its_limits() {
        let line = || {
            MaskComponent::new(MaskShape::Linear {
                from: [0.; 2],
                to: [1.; 2],
            })
        };
        let mut group = MaskGroup {
            components: vec![line(); 32],
            ..Default::default()
        };
        assert!(group.validate().is_ok());
        group.components.push(line());
        assert!(group.validate().is_err());
        assert!(validate(&vec![MaskGroup::default(); 16]).is_ok());
        assert!(validate(&vec![MaskGroup::default(); 17]).is_err());
        let brush = |strokes: usize, points: usize| MaskShape::Brush {
            strokes: vec![
                BrushStroke {
                    points: vec![[0.5; 2]; points].into(),
                    radius: 0.1,
                    feather: 0.,
                    flow: 1.,
                    density: 1.,
                    erase: false,
                    auto_mask: false,
                };
                strokes
            ],
        };
        assert!(brush(2000, 4096).validate().is_ok());
        assert!(brush(2001, 1).validate().is_err());
        assert!(brush(1, 4097).validate().is_err());
    }
}
