//! Heal and Clone operations as a recipe stores them (Lightroom's Remove panel):
//! where each copies from and to, its shape, feather and opacity, and the
//! generated patches of experimental AI Remove. Positions are in image space,
//! sizes fractions of the photo's long edge; rendering them is
//! `develop::retouch`'s.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum RetouchMode {
    /// Copy the source texture and match its tone and colour to the surroundings.
    #[default]
    Heal,
    /// Copy the source pixels as they are.
    Clone,
    /// Lay a generated patch ([`RetouchPatch`]) over the shape, matched to the
    /// surroundings as Heal matches its source: experimental AI Remove. Only
    /// RAWmakase reads it; it is never written to XMP.
    Generate,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum RetouchShape {
    /// A circle, from a click.
    Spot { center: [f32; 2], radius: f32 },
    /// A brushed path of dabs with a common radius, from a drag.
    Brush {
        points: Arc<[[f32; 2]]>,
        radius: f32,
    },
}

/// The pixels a generator made for a [`RetouchMode::Generate`] operation, held
/// by content id in the asset store (`storage::mask_assets`): 3 channels of
/// little-endian f16, linear camera RGB before white balance, rows top to
/// bottom, upright in the image frame over `rect`. The patch renders without
/// the generator that made it.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RetouchPatch {
    /// The pixels' content id ([`crate::storage::bitmaps::Bitmap::content_id`]).
    pub id: String,
    pub width: u32,
    pub height: u32,
    /// Where the pixels lie, `[x0, y0, x1, y1]` in the image frame; pixel
    /// centres as a buffer region's (`(i + 0.5) / width` across it).
    pub rect: [f32; 4],
    /// What made it: `<extension>/<generator>`.
    pub generator: String,
    /// The model the generator named.
    pub model: String,
    /// Whether it was made from the photo with AI Denoise on. It renders on
    /// either; the spot's settings say when the photo's differs.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ai_denoise: bool,
}
/// The longest side of a patch, in pixels.
pub const MAX_PATCH_SIDE: u32 = 2048;
/// The longest generator or model name a patch keeps, in bytes.
pub const MAX_PROVENANCE: usize = 256;
impl RetouchPatch {
    fn validate(&self) -> Result<()> {
        let [x0, y0, x1, y1] = self.rect;
        ensure!(
            self.rect
                .iter()
                .all(|v| v.is_finite() && (0. ..=1.).contains(v))
                && x0 < x1
                && y0 < y1,
            "Invalid AI Remove patch placement"
        );
        ensure!(
            (1..=MAX_PATCH_SIDE).contains(&self.width)
                && (1..=MAX_PATCH_SIDE).contains(&self.height),
            "An AI Remove patch is at most {MAX_PATCH_SIDE} pixels along either side"
        );
        ensure!(
            crate::storage::bitmaps::Bitmap::is_valid_id(&self.id),
            "Invalid AI Remove patch id"
        );
        let name = |s: &str| {
            !s.is_empty() && s.len() <= MAX_PROVENANCE && !s.chars().any(char::is_control)
        };
        ensure!(
            name(&self.generator) && name(&self.model),
            "Invalid AI Remove patch provenance"
        );
        Ok(())
    }
}

/// One Heal, Clone or Generate operation. Operations apply in list order; later
/// ones see the result of earlier ones, as in Lightroom.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RetouchOp {
    pub mode: RetouchMode,
    pub shape: RetouchShape,
    /// Soft edge, 0–1 of the radius.
    pub feather: f32,
    /// 0–1.
    pub opacity: f32,
    /// Source position minus destination position, in image space. `[0, 0]`
    /// for a Generate operation, whose source is its patch.
    pub offset: [f32; 2],
    /// A Generate operation's pixels; none for Heal and Clone. Releases
    /// before AI Remove refuse an operation with one, so it is never dropped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch: Option<RetouchPatch>,
}
/// Longest brush path kept, in dabs.
pub const MAX_POINTS: usize = 4096;
/// Most operations per photo.
pub const MAX_OPS: usize = 1000;
impl RetouchOp {
    pub fn validate(&self) -> Result<()> {
        let unit = |v: f32| v.is_finite() && (0. ..=1.).contains(&v);
        let position = |p: &[f32; 2]| p.iter().all(|v| v.is_finite() && v.abs() <= 2.);
        ensure!(
            unit(self.feather) && unit(self.opacity) && position(&self.offset),
            "Invalid spot removal settings"
        );
        match (self.mode, &self.patch) {
            (RetouchMode::Generate, Some(patch)) => {
                ensure!(
                    self.offset == [0.; 2],
                    "An AI Remove spot copies from nowhere"
                );
                patch.validate()?;
            }
            (RetouchMode::Generate, None) => anyhow::bail!("An AI Remove spot has no patch"),
            (_, Some(_)) => anyhow::bail!("Only an AI Remove spot has a patch"),
            (_, None) => {}
        }
        let radius = |r: f32| r.is_finite() && (1e-4..=0.5).contains(&r);
        match &self.shape {
            RetouchShape::Spot { center, radius: r } => {
                ensure!(position(center) && radius(*r), "Invalid spot")
            }
            RetouchShape::Brush { points, radius: r } => ensure!(
                !points.is_empty()
                    && points.len() <= MAX_POINTS
                    && points.iter().all(position)
                    && radius(*r),
                "Invalid spot removal brush"
            ),
        }
        Ok(())
    }
    /// The destination's radius, as a fraction of the long edge.
    pub fn radius(&self) -> f32 {
        match &self.shape {
            RetouchShape::Spot { radius, .. } | RetouchShape::Brush { radius, .. } => *radius,
        }
    }
    pub fn set_radius(&mut self, r: f32) {
        match &mut self.shape {
            RetouchShape::Spot { radius, .. } | RetouchShape::Brush { radius, .. } => *radius = r,
        }
    }
    /// Where the pin sits: the spot's centre, or the brush path's last dab.
    pub fn pin(&self) -> [f32; 2] {
        match &self.shape {
            RetouchShape::Spot { center, .. } => *center,
            RetouchShape::Brush { points, .. } => points[points.len() - 1],
        }
    }
    /// Whether it lays a generated patch: its shape and source stay where the
    /// patch was made.
    pub fn is_generated(&self) -> bool {
        self.mode == RetouchMode::Generate
    }
    /// Moves the destination by `delta` (image space), keeping the source in place.
    pub fn translate(&mut self, delta: [f32; 2]) {
        let shift = |p: [f32; 2]| [p[0] + delta[0], p[1] + delta[1]];
        match &mut self.shape {
            RetouchShape::Spot { center, .. } => *center = shift(*center),
            RetouchShape::Brush { points, .. } => {
                *points = points.iter().map(|p| shift(*p)).collect();
            }
        }
        self.offset = [self.offset[0] - delta[0], self.offset[1] - delta[1]];
    }
}
/// A long-edge fraction as normalised x and y radii.
pub fn radii(r: f32, aspect: f32) -> (f32, f32) {
    if aspect >= 1. {
        (r, r * aspect)
    } else {
        (r / aspect, r)
    }
}
pub fn validate(ops: &[RetouchOp]) -> Result<()> {
    ensure!(ops.len() <= MAX_OPS, "Too many spot removals");
    ops.iter().try_for_each(RetouchOp::validate)
}
/// The patches' content ids, in order.
pub fn patch_ids(ops: &[RetouchOp]) -> impl Iterator<Item = &str> {
    ops.iter()
        .filter_map(|op| op.patch.as_ref().map(|p| p.id.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operations_keep_their_stored_form() {
        let spot = RetouchOp {
            mode: RetouchMode::Clone,
            shape: RetouchShape::Spot {
                center: [0.25, 0.5],
                radius: 0.02,
            },
            feather: 0.5,
            opacity: 1.,
            offset: [0.1, 0.],
            patch: None,
        };
        let json = serde_json::to_string(&spot).unwrap();
        assert_eq!(
            json,
            r#"{"mode":"Clone","shape":{"Spot":{"center":[0.25,0.5],"radius":0.02}},"feather":0.5,"opacity":1.0,"offset":[0.1,0.0]}"#
        );
        assert_eq!(serde_json::from_str::<RetouchOp>(&json).unwrap(), spot);
        let brush = r#"{"mode":"Heal","shape":{"Brush":{"points":[[0.1,0.1],[0.2,0.1]],"radius":0.01}},"feather":0.0,"opacity":0.5,"offset":[0.0,0.2]}"#;
        let op: RetouchOp = serde_json::from_str(brush).unwrap();
        assert_eq!(op.pin(), [0.2, 0.1]);
        assert!(validate(&[spot, op]).is_ok());
        let unknown = format!("{},\"extra\":1}}", &brush[..brush.len() - 1]);
        assert!(serde_json::from_str::<serde_json::Value>(&unknown).is_ok());
        assert!(serde_json::from_str::<RetouchOp>(&unknown).is_err());
    }

    #[test]
    fn validation_keeps_its_limits() {
        let at = |radius| RetouchOp {
            mode: RetouchMode::Heal,
            shape: RetouchShape::Spot {
                center: [0.5, 0.5],
                radius,
            },
            feather: 0.,
            opacity: 1.,
            offset: [0.; 2],
            patch: None,
        };
        assert!(at(0.5).validate().is_ok());
        assert!(at(0.6).validate().is_err());
        assert!(validate(&vec![at(0.1); 1000]).is_ok());
        assert!(validate(&vec![at(0.1); 1001]).is_err());
        let mut long = at(0.1);
        long.shape = RetouchShape::Brush {
            points: vec![[0.5; 2]; 4096].into(),
            radius: 0.1,
        };
        assert!(long.validate().is_ok());
        long.shape = RetouchShape::Brush {
            points: vec![[0.5; 2]; 4097].into(),
            radius: 0.1,
        };
        assert!(long.validate().is_err());
    }

    fn patch() -> RetouchPatch {
        RetouchPatch {
            id: format!("sha256:{}", "a".repeat(64)),
            width: 64,
            height: 48,
            rect: [0.25, 0.25, 0.5, 0.5],
            generator: "dev.pch.ai-remove/remove".into(),
            model: "bria/fibo-edit-1.5/eraser".into(),
            ai_denoise: false,
        }
    }

    #[test]
    fn a_generated_spot_keeps_its_patch_and_only_it_has_one() {
        let generated = RetouchOp {
            mode: RetouchMode::Generate,
            shape: RetouchShape::Brush {
                points: vec![[0.3, 0.3], [0.4, 0.4]].into(),
                radius: 0.02,
            },
            feather: 0.5,
            opacity: 1.,
            offset: [0.; 2],
            patch: Some(patch()),
        };
        generated.validate().unwrap();
        assert!(generated.is_generated());
        let json = serde_json::to_string(&generated).unwrap();
        assert!(json.contains(r#""mode":"Generate""#), "{json}");
        assert!(!json.contains("ai_denoise"), "{json}");
        assert_eq!(serde_json::from_str::<RetouchOp>(&json).unwrap(), generated);
        assert_eq!(
            patch_ids(std::slice::from_ref(&generated)).collect::<Vec<_>>(),
            [generated.patch.as_ref().unwrap().id.as_str()]
        );
        let broken = |change: &dyn Fn(&mut RetouchOp)| {
            let mut op = generated.clone();
            change(&mut op);
            assert!(op.validate().is_err(), "{op:?}");
        };
        broken(&|op| op.patch = None);
        broken(&|op| op.offset = [0.1, 0.]);
        broken(&|op| op.mode = RetouchMode::Heal);
        broken(&|op| op.patch.as_mut().unwrap().rect = [0.5, 0.25, 0.25, 0.5]);
        broken(&|op| op.patch.as_mut().unwrap().rect = [0.25, 0.25, 1.5, 0.5]);
        broken(&|op| op.patch.as_mut().unwrap().rect[0] = f32::NAN);
        broken(&|op| op.patch.as_mut().unwrap().width = 0);
        broken(&|op| op.patch.as_mut().unwrap().height = MAX_PATCH_SIDE + 1);
        broken(&|op| op.patch.as_mut().unwrap().id = "not-an-id".into());
        broken(&|op| op.patch.as_mut().unwrap().generator = String::new());
        broken(&|op| op.patch.as_mut().unwrap().model = "m".repeat(MAX_PROVENANCE + 1));
        broken(&|op| op.patch.as_mut().unwrap().model = "a\nb".into());
        // Heal and Clone ops saved before AI Remove read as they were.
        let heal: RetouchOp = serde_json::from_str(
            r#"{"mode":"Heal","shape":{"Spot":{"center":[0.5,0.5],"radius":0.01}},"feather":0.0,"opacity":1.0,"offset":[0.1,0.0]}"#,
        )
        .unwrap();
        assert_eq!(heal.patch, None);
    }
}
