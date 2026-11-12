//! Viewer input: what the user points at on the photo before a request runs.
//!
//! Coordinates are in the photo's image frame (the app's `model::image_frame`):
//! the camera-oriented default crop, before lens correction, Transform and
//! crop, normalised to [0, 1] with (0, 0) at the top left. Masks and spots use
//! the same frame, so a click lands where the mask it shapes is drawn.
use crate::manifest::Input;
use crate::message::{ErrorCode, RemoteError};
use serde::{Deserialize, Serialize};

/// The most clicks one input carries.
///
/// Kept equal to `rawmakase_selection::process::MAX_POINTS` (today in
/// `rawmakase-inference`), the most clicks Select Subject's model takes, so
/// every input the app collects can reach it.
pub const MAX_CLICKS: usize = 32;

/// Viewer input, as a request carries it. Written
/// `{"kind":"prompt","clicks":[…],"bounds":{…}}`; a kind this version doesn't
/// know (`brush`) is refused.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ViewerInput {
    /// One click.
    Point {
        /// Across, 0 to 1.
        x: f32,
        /// Down, 0 to 1.
        y: f32,
    },
    /// Positive and negative clicks.
    Points {
        /// At most [`MAX_CLICKS`], in the order they were made.
        clicks: Vec<Click>,
    },
    /// One dragged box.
    Box {
        /// The left edge, 0 to 1.
        x0: f32,
        /// The top edge, 0 to 1.
        y0: f32,
        /// The right edge, greater than `x0`.
        x1: f32,
        /// The bottom edge, greater than `y0`.
        y1: f32,
    },
    /// Clicks and an optional box, refined together: each change sends the
    /// whole prompt again.
    Prompt {
        /// At most [`MAX_CLICKS`], in the order they were made.
        #[serde(default)]
        clicks: Vec<Click>,
        /// The box, when there is one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bounds: Option<Bounds>,
    },
}

/// One click of [`ViewerInput::Points`] or [`ViewerInput::Prompt`].
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Click {
    /// Across, 0 to 1.
    pub x: f32,
    /// Down, 0 to 1.
    pub y: f32,
    /// Whether it adds what is under it (a click) or takes it away (Alt-click).
    pub positive: bool,
}

/// A box of [`ViewerInput::Prompt`].
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Bounds {
    /// The left edge, 0 to 1.
    pub x0: f32,
    /// The top edge, 0 to 1.
    pub y0: f32,
    /// The right edge, greater than `x0`.
    pub x1: f32,
    /// The bottom edge, greater than `y0`.
    pub y1: f32,
}

impl ViewerInput {
    /// The manifest's name for its kind: what a command or a generator that
    /// takes it declares as its `input`.
    pub fn kind(&self) -> Input {
        match self {
            Self::Point { .. } => Input::Point,
            Self::Points { .. } => Input::Points,
            Self::Box { .. } => Input::Box,
            Self::Prompt { .. } => Input::Prompt,
        }
    }

    /// Checks what the types can't: every coordinate finite and in [0, 1], a
    /// box ordered and not empty, and at most [`MAX_CLICKS`] clicks.
    ///
    /// There is no minimum box size. The app ignores a drag smaller than 1% of
    /// the photo as a gesture, but a box it derives (a raster's extent) may be
    /// thinner, and is valid.
    pub fn validate(&self) -> Result<(), RemoteError> {
        match self {
            Self::Point { x, y } => check_point(*x, *y),
            Self::Points { clicks } => check_clicks(clicks),
            Self::Box { x0, y0, x1, y1 } => Bounds {
                x0: *x0,
                y0: *y0,
                x1: *x1,
                y1: *y1,
            }
            .validate(),
            Self::Prompt { clicks, bounds } => {
                check_clicks(clicks)?;
                bounds.as_ref().map_or(Ok(()), Bounds::validate)
            }
        }
    }
}

impl Bounds {
    /// Checks that every edge is finite and in [0, 1], and that the box is
    /// ordered and not empty.
    pub fn validate(&self) -> Result<(), RemoteError> {
        check_point(self.x0, self.y0)?;
        check_point(self.x1, self.y1)?;
        if self.x0 < self.x1 && self.y0 < self.y1 {
            Ok(())
        } else {
            Err(invalid(format!(
                "the box ({}, {}) to ({}, {}) is empty or reversed",
                self.x0, self.y0, self.x1, self.y1
            )))
        }
    }
}

fn check_clicks(clicks: &[Click]) -> Result<(), RemoteError> {
    if clicks.len() > MAX_CLICKS {
        return Err(invalid(format!(
            "{} clicks, more than the {MAX_CLICKS} allowed",
            clicks.len()
        )));
    }
    clicks.iter().try_for_each(|c| check_point(c.x, c.y))
}

fn check_point(x: f32, y: f32) -> Result<(), RemoteError> {
    let inside = |v: f32| v.is_finite() && (0.0..=1.0).contains(&v);
    if inside(x) && inside(y) {
        Ok(())
    } else {
        Err(invalid(format!(
            "the point ({x}, {y}) is outside the photo"
        )))
    }
}

fn invalid(message: String) -> RemoteError {
    RemoteError::new(ErrorCode::InvalidRequest, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn click(x: f32, y: f32) -> Click {
        Click {
            x,
            y,
            positive: true,
        }
    }

    #[test]
    fn every_kind_reads_in_its_documented_shape() {
        let prompt: ViewerInput = serde_json::from_value(json!({
            "kind": "prompt",
            "clicks": [{"x": 0.25, "y": 0.5, "positive": false}],
            "bounds": {"x0": 0.0, "y0": 0.0, "x1": 0.5, "y1": 1.0}
        }))
        .unwrap();
        assert_eq!(
            prompt,
            ViewerInput::Prompt {
                clicks: vec![Click {
                    x: 0.25,
                    y: 0.5,
                    positive: false
                }],
                bounds: Some(Bounds {
                    x0: 0.0,
                    y0: 0.0,
                    x1: 0.5,
                    y1: 1.0
                }),
            }
        );
        for input in [
            prompt,
            ViewerInput::Point { x: 0.5, y: 0.5 },
            ViewerInput::Points {
                clicks: vec![click(0.1, 0.9)],
            },
            ViewerInput::Box {
                x0: 0.1,
                y0: 0.2,
                x1: 0.3,
                y1: 0.4,
            },
            ViewerInput::Prompt {
                clicks: vec![click(0.0, 1.0)],
                bounds: None,
            },
        ] {
            input.validate().unwrap();
            let value = serde_json::to_value(&input).unwrap();
            assert_eq!(serde_json::from_value::<ViewerInput>(value).unwrap(), input);
        }
        let brush = serde_json::from_value::<ViewerInput>(json!({"kind": "brush"}));
        assert!(brush.is_err());
    }

    #[test]
    fn coordinates_must_be_finite_and_inside_the_photo() {
        for bad in [-0.01, 1.01, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let point = ViewerInput::Point { x: bad, y: 0.5 };
            assert_eq!(
                point.validate().unwrap_err().code,
                ErrorCode::InvalidRequest
            );
            let points = ViewerInput::Points {
                clicks: vec![click(0.5, bad)],
            };
            assert!(points.validate().is_err(), "{bad}");
            let edge = ViewerInput::Box {
                x0: 0.0,
                y0: 0.0,
                x1: bad,
                y1: 1.0,
            };
            assert!(edge.validate().is_err(), "{bad}");
        }
    }

    #[test]
    fn a_box_is_ordered_and_not_empty_but_may_be_thin() {
        let boxed = |x0, y0, x1, y1| ViewerInput::Box { x0, y0, x1, y1 };
        assert!(boxed(0.5, 0.1, 0.5, 0.9).validate().is_err());
        assert!(boxed(0.1, 0.5, 0.9, 0.5).validate().is_err());
        assert!(boxed(0.6, 0.1, 0.5, 0.9).validate().is_err());
        // Thinner than the app's 1% drag threshold: a raster's extent may be.
        boxed(0.5, 0.1, 0.501, 0.9).validate().unwrap();
        let reversed = ViewerInput::Prompt {
            clicks: Vec::new(),
            bounds: Some(Bounds {
                x0: 0.9,
                y0: 0.1,
                x1: 0.1,
                y1: 0.9,
            }),
        };
        assert!(reversed.validate().is_err());
    }

    #[test]
    fn clicks_are_capped() {
        let clicks = vec![click(0.5, 0.5); MAX_CLICKS];
        ViewerInput::Points {
            clicks: clicks.clone(),
        }
        .validate()
        .unwrap();
        let mut over = clicks;
        over.push(click(0.5, 0.5));
        assert!(
            ViewerInput::Points {
                clicks: over.clone()
            }
            .validate()
            .is_err()
        );
        let prompt = ViewerInput::Prompt {
            clicks: over,
            bounds: None,
        };
        assert!(prompt.validate().is_err());
    }
}
