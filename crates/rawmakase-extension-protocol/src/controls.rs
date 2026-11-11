//! Host-drawn controls: what a panel is made of. The program describes them and
//! the app draws them with its own widgets, so an extension never draws.
//!
//! A panel is a list of [`Control`]s, laid out in one column; a
//! [`ControlKind::Section`] groups some under a title. The program returns the
//! whole list when a panel opens and after each change, and may push a new one
//! with the `panel-update` event. The app sends a change when the user commits
//! it (a text field on Enter or when it loses focus), and a validation error
//! comes back as the control's `help`.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The deepest sections nest, and the most controls a panel holds, so a
/// program can't make the app draw without end.
pub const MAX_DEPTH: usize = 8;
/// See [`MAX_DEPTH`].
pub const MAX_CONTROLS: usize = 1000;

/// One control of a panel.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Control {
    /// The control's id within its panel, sent back with its changes. Only
    /// `label`, `help`, `progress`, `image` and `section` may go without.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// The text beside or above the control.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// A line of help under the control; a validation error goes here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
    /// Shown only while another control of the panel has a value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible_when: Option<Condition>,
    /// Drawn but not editable.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub disabled: bool,
    /// What the control is, with its value.
    #[serde(flatten)]
    pub kind: ControlKind,
}

/// A condition on another control's value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Condition {
    /// The other control's id.
    pub control: String,
    /// The value it must have.
    pub equals: Value,
}

/// The controls the app draws, with their values.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ControlKind {
    /// Controls grouped under a title.
    Section {
        /// The section's title.
        title: String,
        /// Its controls.
        controls: Vec<Control>,
        /// Drawn collapsed until opened.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        collapsed: bool,
    },
    /// A number in a range.
    Slider {
        /// The value.
        value: f64,
        /// The smallest value.
        min: f64,
        /// The largest value.
        max: f64,
        /// The step between values; continuous without one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        step: Option<f64>,
        /// The unit shown after the value, for example `%`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        unit: Option<String>,
        /// Zero is in the middle of the track, as for Temperature.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        centred: bool,
    },
    /// An angle in degrees.
    Angle {
        /// The value, in degrees.
        value: f64,
    },
    /// On or off.
    Toggle {
        /// The value.
        value: bool,
    },
    /// One of a list, as a drop-down.
    Choice {
        /// The options' ids and titles.
        options: Vec<Choice>,
        /// The chosen option's id.
        value: String,
    },
    /// One of a short list, as buttons side by side.
    Segmented {
        /// The options' ids and titles.
        options: Vec<Choice>,
        /// The chosen option's id.
        value: String,
    },
    /// An sRGB colour, components 0 to 1.
    Color {
        /// Red, green, blue and, with `alpha`, alpha.
        value: Vec<f64>,
        /// Whether the colour has an alpha component.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        alpha: bool,
    },
    /// A curve through points, inputs and outputs 0 to 1, as the Tone Curve's.
    Curve {
        /// The points, ordered by input.
        points: Vec<[f64; 2]>,
    },
    /// A number typed in, checked against its range.
    Number {
        /// The value.
        value: f64,
        /// The smallest value accepted.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min: Option<f64>,
        /// The largest value accepted.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max: Option<f64>,
    },
    /// Text typed in.
    Text {
        /// The value.
        value: String,
        /// Several lines.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        multiline: bool,
    },
    /// A file chosen in the system's dialog.
    File {
        /// The chosen path, or none.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        value: Option<String>,
        /// File name extensions the dialog offers, without dots.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        extensions: Vec<String>,
    },
    /// A folder chosen in the system's dialog.
    Folder {
        /// The chosen path, or none.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        value: Option<String>,
    },
    /// A button; clicking it sends `panel.clicked`.
    Button {
        /// The button's text.
        title: String,
    },
    /// Text to read.
    Label {
        /// The text.
        text: String,
    },
    /// Smaller explanatory text.
    Help {
        /// The text.
        text: String,
    },
    /// How far something is, driven by the program.
    Progress {
        /// The fraction done, 0 to 1; unknown without one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        value: Option<f64>,
    },
    /// A thumbnail the program wrote, a PNG in its data or cache folder.
    Image {
        /// The PNG's path.
        path: String,
    },
}

/// One option of a [`ControlKind::Choice`] or [`ControlKind::Segmented`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Choice {
    /// What the value is when it is chosen.
    pub id: String,
    /// What the user reads.
    pub title: String,
}

/// Checks a panel's controls before the app draws them: a bounded tree, ids
/// unique and present where a value is sent back, values inside their ranges.
pub fn validate(controls: &[Control]) -> Result<(), String> {
    let mut ids = std::collections::BTreeSet::new();
    let mut count = 0;
    validate_in(controls, 1, &mut ids, &mut count)
}

fn validate_in<'a>(
    controls: &'a [Control],
    depth: usize,
    ids: &mut std::collections::BTreeSet<&'a str>,
    count: &mut usize,
) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Err(format!("sections nest deeper than {MAX_DEPTH}"));
    }
    for control in controls {
        *count += 1;
        if *count > MAX_CONTROLS {
            return Err(format!("more than {MAX_CONTROLS} controls"));
        }
        let needs_id = !matches!(
            control.kind,
            ControlKind::Section { .. }
                | ControlKind::Label { .. }
                | ControlKind::Help { .. }
                | ControlKind::Progress { .. }
                | ControlKind::Image { .. }
        );
        match &control.id {
            Some(id) if !ids.insert(id) => return Err(format!("the id {id:?} is used twice")),
            None if needs_id => return Err("a control that sends a value has no id".into()),
            _ => {}
        }
        let name = control.id.as_deref().unwrap_or("a control");
        let fail = |why: &str| Err(format!("{name}: {why}"));
        match &control.kind {
            ControlKind::Section { controls, .. } => {
                validate_in(controls, depth + 1, ids, count)?;
            }
            ControlKind::Slider {
                value,
                min,
                max,
                step,
                ..
            } => {
                if !(min.is_finite() && max.is_finite() && min < max) {
                    return fail("its range is empty");
                }
                if !(value.is_finite() && min <= value && value <= max) {
                    return fail("its value is outside its range");
                }
                if step.is_some_and(|s| !(s.is_finite() && s > 0.0)) {
                    return fail("its step is not positive");
                }
            }
            ControlKind::Angle { value } if !value.is_finite() => return fail("not a number"),
            ControlKind::Choice { options, value } | ControlKind::Segmented { options, value } => {
                if options.is_empty() {
                    return fail("it has no options");
                }
                if !options.iter().any(|o| &o.id == value) {
                    return fail("its value is not one of its options");
                }
            }
            ControlKind::Color { value, alpha } => {
                if value.len() != if *alpha { 4 } else { 3 }
                    || value.iter().any(|c| !(0.0..=1.0).contains(c))
                {
                    return fail("a colour has three components (four with alpha), 0 to 1");
                }
            }
            ControlKind::Curve { points } => {
                if points.len() < 2
                    || points.iter().flatten().any(|c| !(0.0..=1.0).contains(c))
                    || points.windows(2).any(|w| w[0][0] >= w[1][0])
                {
                    return fail("a curve has two or more points, 0 to 1, in increasing order");
                }
            }
            ControlKind::Number { value, min, max } => {
                if !value.is_finite()
                    || min.is_some_and(|m| *value < m)
                    || max.is_some_and(|m| *value > m)
                {
                    return fail("its value is outside its range");
                }
            }
            ControlKind::Progress { value: Some(v) } if !(0.0..=1.0).contains(v) => {
                return fail("progress is 0 to 1");
            }
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(v: Value) -> Vec<Control> {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn a_panel_reads_in_its_documented_shape_and_round_trips() {
        let controls = parse(json!([
            {"type": "section", "title": "Device", "controls": [
                {"type": "choice", "id": "port", "label": "Input",
                 "options": [{"id": "a", "title": "Loupedeck"}], "value": "a"},
                {"type": "slider", "id": "speed", "value": 2, "min": 1, "max": 5, "step": 1,
                 "visible_when": {"control": "port", "equals": "a"}},
                {"type": "toggle", "id": "on", "value": true, "help": "Listens while on"},
            ]},
            {"type": "button", "id": "learn", "title": "Learn"},
            {"type": "progress"},
        ]));
        validate(&controls).unwrap();
        let json = serde_json::to_value(&controls).unwrap();
        assert_eq!(parse(json), controls);
    }

    #[test]
    fn values_outside_their_ranges_and_reused_ids_are_refused() {
        let bad = [
            json!([{"type": "slider", "id": "s", "value": 9, "min": 0, "max": 5}]),
            json!([{"type": "slider", "id": "s", "value": 1, "min": 5, "max": 0}]),
            json!([{"type": "choice", "id": "c", "options": [], "value": "x"}]),
            json!([{"type": "color", "id": "c", "value": [1, 0]}]),
            json!([{"type": "curve", "id": "c", "points": [[0.5, 0], [0.2, 1]]}]),
            json!([{"type": "toggle", "id": "t", "value": true},
                   {"type": "text", "id": "t", "value": ""}]),
            json!([{"type": "toggle", "value": true}]),
        ];
        for controls in bad {
            assert!(validate(&parse(controls.clone())).is_err(), "{controls}");
        }
        assert!(serde_json::from_value::<Vec<Control>>(json!([{"type": "canvas"}])).is_err());
    }

    #[test]
    fn a_tree_too_deep_or_too_large_is_refused() {
        let mut deep = json!([{"type": "label", "text": "leaf"}]);
        for _ in 0..MAX_DEPTH {
            deep = json!([{"type": "section", "title": "s", "controls": deep}]);
        }
        assert!(validate(&parse(deep)).is_err());
        let many: Vec<Value> = (0..=MAX_CONTROLS)
            .map(|_| json!({"type": "label", "text": "x"}))
            .collect();
        assert!(validate(&parse(Value::Array(many))).is_err());
    }
}
