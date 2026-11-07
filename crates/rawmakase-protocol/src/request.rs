//! The commands a client sends, as they travel on the wire.
//!
//! A request is one JSON object: the envelope (`token`, `protocol`,
//! `request_id` and an optional [`Target`](crate::Target)) beside the fields
//! of one [`Request`], named by its `cmd`. [`Request`] reads only its own
//! fields and ignores the rest, as the app always has, so the envelope stays
//! where it is.
//!
//! Values keep their wire types: a parameter or an action is still a name, for
//! the app to resolve. What every client must respect, the ranges and the
//! choices, is checked here, with the messages the app has always given.
use serde::de::{Deserializer, Error, IgnoredAny};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::CurveChannel;

/// The most catalog entries one `photos` request returns.
pub const MAX_PHOTOS: u64 = 500;
/// How many catalog entries `photos` returns when it is not told.
pub const DEFAULT_PHOTOS: u64 = 100;
/// How far one `turn` moves a parameter at most, either way.
pub const MAX_TICKS: i32 = 1000;
/// The longest edge `export` and `preview` accept, in pixels.
pub const MAX_EDGE: u32 = 16384;
/// The long edge of a `preview` that does not choose one. An `export` that
/// does not is full size.
pub const DEFAULT_PREVIEW_EDGE: u32 = 1600;
/// The longest a `wait` waits, in milliseconds: within clients' read timeout.
pub const MAX_WAIT_MS: u64 = 5000;
/// How long a `wait` waits when it is not told, in milliseconds.
pub const DEFAULT_WAIT_MS: u64 = 1000;

/// One command, tagged by `cmd`. Fields marked optional may be left out;
/// those read leniently (`photos`, `presets`, `preset`, `open` and `note`'s
/// `press`) also treat a value of another type as left out, as the app always
/// has.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    /// The app's state.
    State,
    /// The protocol version, actions, parameters, ranges and scopes.
    Capabilities,
    /// Catalog photos matching `query`, a page at a time.
    Photos {
        /// Only photos whose file name contains this text, ignoring case; every photo
        /// when left out.
        #[serde(
            default,
            deserialize_with = "lenient",
            skip_serializing_if = "Option::is_none"
        )]
        query: Option<String>,
        /// Entries to skip; none by default.
        #[serde(
            default,
            deserialize_with = "lenient",
            skip_serializing_if = "Option::is_none"
        )]
        offset: Option<u64>,
        /// At most this many entries, from 1 to [`MAX_PHOTOS`] (a value out of
        /// range is brought into it); [`DEFAULT_PHOTOS`] by default.
        #[serde(
            default,
            deserialize_with = "lenient",
            skip_serializing_if = "Option::is_none"
        )]
        limit: Option<u64>,
    },
    /// The develop presets, of one group or all.
    Presets {
        /// Only the presets of this group, by its name as the Presets panel shows it.
        #[serde(
            default,
            deserialize_with = "lenient",
            skip_serializing_if = "Option::is_none"
        )]
        group: Option<String>,
    },
    /// Applies a develop preset by `id`, or else by `name` within an optional
    /// `group`. One of `id` and `name` is required.
    Preset {
        /// The preset's id, as `presets` lists it.
        #[serde(
            default,
            deserialize_with = "lenient",
            skip_serializing_if = "Option::is_none"
        )]
        id: Option<String>,
        /// The preset's name, when there is no `id`.
        #[serde(
            default,
            deserialize_with = "lenient",
            skip_serializing_if = "Option::is_none"
        )]
        name: Option<String>,
        /// The group `name` is looked for in; every group when left out.
        #[serde(
            default,
            deserialize_with = "lenient",
            skip_serializing_if = "Option::is_none"
        )]
        group: Option<String>,
    },
    /// Sets a tone curve's points.
    Curve {
        /// Which curve: the composite one or a colour channel's.
        #[serde(deserialize_with = "channel")]
        channel: CurveChannel,
        /// Normalized `[input, output]` pairs; see [`crate::curve`].
        #[serde(deserialize_with = "points")]
        points: Vec<[f32; 2]>,
    },
    /// Sets a parameter to a value, finite as an `f32`.
    Set {
        /// A parameter's name, as `capabilities` lists it; a colour band needs its
        /// channel (`band3.sat`).
        #[serde(deserialize_with = "param")]
        param: String,
        /// The value in the units the panel shows where it is set, clamped to what
        /// the parameter supports there: Exposure in EV (±8 globally, though the
        /// slider shows ±5; ±4 on a mask), Temperature in kelvin on a RAW's own
        /// edit, but −100…100 on a mask or a rendered photo, as Tint is. See
        /// docs/automation.md.
        #[serde(deserialize_with = "finite")]
        value: f64,
    },
    /// Turns a parameter by ticks, as a dial would.
    Turn {
        /// A parameter's name, as for `set`.
        #[serde(deserialize_with = "param")]
        param: String,
        /// Steps of the parameter's own size, up or down, at most [`MAX_TICKS`] either
        /// way.
        #[serde(deserialize_with = "ticks")]
        ticks: i32,
    },
    /// Runs a named action, or a keyboard shortcut such as `cmd+shift+z`.
    Action {
        /// The action's name, as `capabilities` lists it, or a shortcut.
        #[serde(deserialize_with = "action")]
        action: String,
    },
    /// Opens a catalog photo by `id`, or else by `name`. One is required.
    Open {
        /// The photo's catalog id, as `photos` lists it.
        #[serde(
            default,
            deserialize_with = "lenient",
            skip_serializing_if = "Option::is_none"
        )]
        id: Option<i64>,
        /// When there is no `id`: the file name, with or without its extension, or,
        /// with a `/` or `\`, the path. It must match one photo only.
        #[serde(
            default,
            deserialize_with = "lenient",
            skip_serializing_if = "Option::is_none"
        )]
        name: Option<String>,
    },
    /// Searches the Library; empty text clears the search.
    Search {
        /// What to search for, as the Library's search field takes it.
        #[serde(deserialize_with = "text")]
        text: String,
    },
    /// Switches between Develop and the Library.
    Module {
        /// The module to show.
        #[serde(deserialize_with = "module")]
        module: Module,
    },
    /// Goes to the next (1) or previous (-1) photo.
    Photo {
        /// 1 for the next photo, -1 for the previous one.
        #[serde(deserialize_with = "step")]
        step: i32,
    },
    /// Saves the current edit.
    Save,
    /// Renders the edit to a new file at `path`, full size unless `max_edge`
    /// says otherwise, in sRGB unless `color_space` says otherwise.
    Export {
        /// The new file; an existing one is refused.
        #[serde(deserialize_with = "path")]
        path: PathBuf,
        /// The output's long edge in pixels, at most [`MAX_EDGE`].
        #[serde(
            default,
            deserialize_with = "max_edge",
            skip_serializing_if = "Option::is_none"
        )]
        max_edge: Option<u32>,
        /// The pixels' colour space and the profile embedded; the app refuses one
        /// the file's format can't hold (ProPhoto RGB needs TIFF).
        #[serde(
            default,
            deserialize_with = "color_space",
            skip_serializing_if = "Option::is_none"
        )]
        color_space: Option<ColorSpace>,
    },
    /// Renders a reduced preview to a new file at `path`, with a long edge of
    /// [`DEFAULT_PREVIEW_EDGE`] unless `max_edge` says otherwise.
    Preview {
        /// The new file; an existing one is refused.
        #[serde(deserialize_with = "path")]
        path: PathBuf,
        /// The preview's long edge in pixels, at most [`MAX_EDGE`].
        #[serde(
            default,
            deserialize_with = "max_edge",
            skip_serializing_if = "Option::is_none"
        )]
        max_edge: Option<u32>,
    },
    /// Queues previews of the selected photos, built in the background; the
    /// reply says how many were queued, and `state` how far they have got.
    BuildPreviews {
        /// Which previews; Standard by default.
        #[serde(default)]
        kind: PreviewKind,
    },
    /// Queues editable previews of the selected photos (their masters', for
    /// virtual copies), built in the background; the reply says how many were
    /// queued and skipped, and `state` how far they have got.
    BuildEditablePreviews,
    /// Discards the selected photos' editable previews and stops their builds;
    /// the reply says how many photos' went, and how many were kept because
    /// their originals are offline.
    DiscardEditablePreviews {
        /// Discards those of photos whose originals are offline too, which can't
        /// be built again until the originals are back; kept by default.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        include_offline: bool,
    },
    /// An output job's status.
    Job {
        /// The job's id, from the reply that started it.
        #[serde(deserialize_with = "job_id")]
        job_id: u64,
    },
    /// Answers once `until` holds, or after `timeout_ms` ([`DEFAULT_WAIT_MS`]
    /// by default, at most [`MAX_WAIT_MS`]).
    Wait {
        /// The condition, tagged by `until`.
        #[serde(flatten)]
        until: Until,
        /// How long to wait at most, in milliseconds.
        #[serde(
            default,
            deserialize_with = "timeout_ms",
            skip_serializing_if = "Option::is_none"
        )]
        timeout_ms: Option<u64>,
    },
    /// A Loupedeck dial's MIDI control change, through the default mapping.
    /// Refuses a `target`.
    Cc {
        /// The control number, 0 to 127.
        #[serde(deserialize_with = "cc")]
        cc: u8,
        /// The control's value, 0 to 127.
        #[serde(deserialize_with = "cc_value")]
        value: u8,
    },
    /// A Loupedeck button's MIDI note, through the default mapping. Refuses a
    /// `target`.
    Note {
        /// The note number, 0 to 127.
        #[serde(deserialize_with = "note")]
        note: u8,
        /// How the button is pressed; a click by default.
        #[serde(default, deserialize_with = "press")]
        press: Press,
    },
}

/// What a `wait` waits for, tagged by `until`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "until", rename_all = "snake_case")]
pub enum Until {
    /// The photo is loaded, or another replaced it.
    Loaded {
        /// The photo's catalog id.
        #[serde(deserialize_with = "photo_id")]
        photo_id: i64,
    },
    /// The output job has finished, or does not exist.
    Job {
        /// The job's id, from the reply that started it.
        #[serde(deserialize_with = "job_id")]
        job_id: u64,
    },
}

/// The colour space an `export` writes in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ColorSpace {
    /// sRGB, as every export without a choice.
    #[default]
    Srgb,
    /// Adobe RGB (1998).
    AdobeRgb,
    /// ProPhoto RGB, 16-bit TIFF only.
    Prophoto,
}

/// The previews a `build_previews` command builds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreviewKind {
    /// Lightroom's Standard-Sized Previews, at the app's Standard Preview Size.
    #[default]
    Standard,
    /// Lightroom's 1:1 Previews, at full size.
    OneToOne,
}

/// The module a `module` command switches to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Module {
    /// Develop, editing the open photo.
    Develop,
    /// The Library, browsing the catalog.
    Library,
}

/// How a `note` presses its button.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Press {
    /// Down, then up.
    #[default]
    Click,
    /// Down only: held until an `up`.
    Down,
    /// Up only: lets a held button go.
    Up,
}

/// A value of any type, which a lenient field reads as left out unless it is a `T`.
#[derive(Deserialize)]
#[serde(untagged)]
enum Lenient<T> {
    Value(T),
    Other(IgnoredAny),
}

fn lenient<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Option<T>, D::Error> {
    Ok(match Lenient::deserialize(d)? {
        Lenient::Value(v) => Some(v),
        Lenient::Other(_) => None,
    })
}

/// A `T`, or the error `message`.
fn or<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D, message: &str) -> Result<T, D::Error> {
    T::deserialize(d).map_err(|_| D::Error::custom(message))
}

fn integer<'de, D: Deserializer<'de>>(
    d: D,
    key: &str,
    min: i64,
    max: i64,
) -> Result<i64, D::Error> {
    i64::deserialize(d)
        .ok()
        .filter(|n| (min..=max).contains(n))
        .ok_or_else(|| D::Error::custom(format!("{key} must be an integer from {min} to {max}")))
}

fn string<'de, D: Deserializer<'de>>(d: D, key: &str) -> Result<String, D::Error> {
    or(d, &format!("{key} must be a string"))
}

fn param<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    string(d, "param")
}
fn action<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    string(d, "action")
}
fn text<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    string(d, "text")
}
fn path<'de, D: Deserializer<'de>>(d: D) -> Result<PathBuf, D::Error> {
    string(d, "path").map(Into::into)
}
fn channel<'de, D: Deserializer<'de>>(d: D) -> Result<CurveChannel, D::Error> {
    or(d, "channel must be rgb, red, green or blue")
}
fn points<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<[f32; 2]>, D::Error> {
    or(d, "points must be an array of [input, output] pairs")
}
fn color_space<'de, D: Deserializer<'de>>(d: D) -> Result<Option<ColorSpace>, D::Error> {
    or(d, "color_space must be srgb, adobe-rgb or prophoto").map(Some)
}
fn module<'de, D: Deserializer<'de>>(d: D) -> Result<Module, D::Error> {
    or(d, "module must be develop or library")
}
fn finite<'de, D: Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
    f64::deserialize(d)
        .ok()
        .filter(|n| n.is_finite() && (*n as f32).is_finite())
        .ok_or_else(|| D::Error::custom("value must be a finite float"))
}
fn ticks<'de, D: Deserializer<'de>>(d: D) -> Result<i32, D::Error> {
    let max = i64::from(MAX_TICKS);
    integer(d, "ticks", -max, max).map(|n| n as i32)
}
fn step<'de, D: Deserializer<'de>>(d: D) -> Result<i32, D::Error> {
    match integer(d, "step", -1, 1)? {
        0 => Err(D::Error::custom("step must be -1 or 1")),
        n => Ok(n as i32),
    }
}
fn max_edge<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u32>, D::Error> {
    integer(d, "max_edge", 1, MAX_EDGE.into()).map(|n| Some(n as u32))
}
fn job_id<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    integer(d, "job_id", 1, i64::MAX).map(|n| n as u64)
}
fn photo_id<'de, D: Deserializer<'de>>(d: D) -> Result<i64, D::Error> {
    integer(d, "photo_id", 1, i64::MAX)
}
fn timeout_ms<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
    integer(d, "timeout_ms", 1, MAX_WAIT_MS as i64).map(|n| Some(n as u64))
}
fn cc<'de, D: Deserializer<'de>>(d: D) -> Result<u8, D::Error> {
    integer(d, "cc", 0, 127).map(|n| n as u8)
}
fn cc_value<'de, D: Deserializer<'de>>(d: D) -> Result<u8, D::Error> {
    integer(d, "value", 0, 127).map(|n| n as u8)
}
fn note<'de, D: Deserializer<'de>>(d: D) -> Result<u8, D::Error> {
    integer(d, "note", 0, 127).map(|n| n as u8)
}
/// A press that is not a string clicks; a string must name a press.
fn press<'de, D: Deserializer<'de>>(d: D) -> Result<Press, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Wire {
        Press(Press),
        // Read only to tell a string from other values.
        Text(#[allow(dead_code)] String),
        Other(IgnoredAny),
    }
    match Wire::deserialize(d)? {
        Wire::Press(press) => Ok(press),
        Wire::Text(_) => Err(D::Error::custom("press must be click, down or up")),
        Wire::Other(_) => Ok(Press::Click),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn parse(value: Value) -> Result<Request, String> {
        Request::deserialize(&value).map_err(|e| e.to_string())
    }

    #[test]
    fn representative_requests_parse_beside_the_envelope() {
        let envelope = |mut command: Value| {
            command["token"] = "secret".into();
            command["protocol"] = 1.into();
            command["request_id"] = "edit-42".into();
            command["target"] = json!({"photo_id":17,"generation":8,"revision":23});
            command
        };
        for (value, request) in [
            (json!({"cmd":"state"}), Request::State),
            (json!({"cmd":"capabilities"}), Request::Capabilities),
            (
                json!({"cmd":"set","param":"exposure","value":0.5}),
                Request::Set {
                    param: "exposure".into(),
                    value: 0.5,
                },
            ),
            (
                json!({"cmd":"set","param":"band3.sat","value":-20}),
                Request::Set {
                    param: "band3.sat".into(),
                    value: -20.0,
                },
            ),
            (
                json!({"cmd":"turn","param":"contrast","ticks":-1000}),
                Request::Turn {
                    param: "contrast".into(),
                    ticks: -1000,
                },
            ),
            (
                json!({"cmd":"curve","channel":"red","points":[[0,0],[0.5,0.6],[1,1]]}),
                Request::Curve {
                    channel: CurveChannel::Red,
                    points: vec![[0.0, 0.0], [0.5, 0.6], [1.0, 1.0]],
                },
            ),
            (
                json!({"cmd":"open","id":7}),
                Request::Open {
                    id: Some(7),
                    name: None,
                },
            ),
            (
                json!({"cmd":"module","module":"library"}),
                Request::Module {
                    module: Module::Library,
                },
            ),
            (
                json!({"cmd":"photo","step":-1}),
                Request::Photo { step: -1 },
            ),
            (
                json!({"cmd":"preview","path":"/tmp/p.jpg"}),
                Request::Preview {
                    path: "/tmp/p.jpg".into(),
                    max_edge: None,
                },
            ),
            (
                json!({"cmd":"export","path":"/tmp/e.tif","max_edge":16384}),
                Request::Export {
                    path: "/tmp/e.tif".into(),
                    max_edge: Some(16384),
                    color_space: None,
                },
            ),
            (
                json!({"cmd":"export","path":"/tmp/e.tif","color_space":"prophoto"}),
                Request::Export {
                    path: "/tmp/e.tif".into(),
                    max_edge: None,
                    color_space: Some(ColorSpace::Prophoto),
                },
            ),
            (
                json!({"cmd":"export","path":"/tmp/e.jpg","color_space":"adobe-rgb"}),
                Request::Export {
                    path: "/tmp/e.jpg".into(),
                    max_edge: None,
                    color_space: Some(ColorSpace::AdobeRgb),
                },
            ),
            (
                json!({"cmd":"wait","until":"loaded","photo_id":17}),
                Request::Wait {
                    until: Until::Loaded { photo_id: 17 },
                    timeout_ms: None,
                },
            ),
            (
                json!({"cmd":"wait","until":"job","job_id":3,"timeout_ms":5000}),
                Request::Wait {
                    until: Until::Job { job_id: 3 },
                    timeout_ms: Some(5000),
                },
            ),
            (
                json!({"cmd":"cc","cc":127,"value":0}),
                Request::Cc { cc: 127, value: 0 },
            ),
            (
                json!({"cmd":"note","note":5}),
                Request::Note {
                    note: 5,
                    press: Press::Click,
                },
            ),
        ] {
            assert_eq!(parse(value.clone()).unwrap(), request, "{value}");
            assert_eq!(parse(envelope(value.clone())).unwrap(), request, "{value}");
            let sent = serde_json::to_value(&request).unwrap();
            assert_eq!(parse(sent).unwrap(), request, "{value}");
        }
    }

    /// Optional fields of the wrong type count as left out, as they always have.
    #[test]
    fn lenient_fields_read_a_wrong_type_as_left_out() {
        assert_eq!(
            parse(json!({"cmd":"photos","query":3,"offset":-1,"limit":"5"})).unwrap(),
            Request::Photos {
                query: None,
                offset: None,
                limit: None
            }
        );
        assert_eq!(
            parse(json!({"cmd":"photos","limit":2.0})).unwrap(),
            Request::Photos {
                query: None,
                offset: None,
                limit: None
            }
        );
        assert_eq!(
            parse(json!({"cmd":"preset","id":null,"name":"Vivid","group":null})).unwrap(),
            Request::Preset {
                id: None,
                name: Some("Vivid".into()),
                group: None
            }
        );
        assert_eq!(
            parse(json!({"cmd":"open","id":"7","name":"DSC_1.NEF"})).unwrap(),
            Request::Open {
                id: None,
                name: Some("DSC_1.NEF".into())
            }
        );
        assert_eq!(
            parse(json!({"cmd":"note","note":1,"press":false})).unwrap(),
            Request::Note {
                note: 1,
                press: Press::Click
            }
        );
    }

    #[test]
    fn requests_out_of_contract_are_refused_with_the_apps_messages() {
        for (value, message) in [
            (json!({"cmd":"rename"}), "unknown variant `rename`"),
            (json!({"command":"state"}), "missing field `cmd`"),
            (json!({"cmd":3}), "invalid type"),
            (
                json!({"cmd":"set","param":"exposure","value":1e100}),
                "value must be a finite float",
            ),
            (
                json!({"cmd":"turn","param":"exposure","ticks":1.5}),
                "ticks must be an integer from -1000 to 1000",
            ),
            (
                json!({"cmd":"turn","param":"exposure","ticks":1001}),
                "ticks must be an integer from -1000 to 1000",
            ),
            (
                json!({"cmd":"turn","param":7,"ticks":1}),
                "param must be a string",
            ),
            (json!({"cmd":"turn","ticks":1}), "missing field `param`"),
            (
                json!({"cmd":"cc","cc":1.5,"value":1}),
                "cc must be an integer from 0 to 127",
            ),
            (json!({"cmd":"photo","step":0}), "step must be -1 or 1"),
            (
                json!({"cmd":"photo","step":2}),
                "step must be an integer from -1 to 1",
            ),
            (
                json!({"cmd":"module","module":"map"}),
                "module must be develop or library",
            ),
            (
                json!({"cmd":"export","path":"/tmp/e.tif","color_space":"display-p3"}),
                "color_space must be srgb, adobe-rgb or prophoto",
            ),
            (
                json!({"cmd":"curve","channel":"luma","points":[[0,0],[1,1]]}),
                "channel must be rgb, red, green or blue",
            ),
            (
                json!({"cmd":"curve","channel":"rgb","points":[0,1]}),
                "points must be an array of [input, output] pairs",
            ),
            (
                json!({"cmd":"preview","path":"/tmp/p.jpg","max_edge":null}),
                "max_edge must be an integer from 1 to 16384",
            ),
            (
                json!({"cmd":"wait","until":"job","job_id":1,"timeout_ms":0}),
                "timeout_ms must be an integer from 1 to 5000",
            ),
            (
                json!({"cmd":"wait","until":"loaded","photo_id":0}),
                "photo_id must be an integer from 1 to 9223372036854775807",
            ),
            (
                json!({"cmd":"wait","until":"render"}),
                "unknown variant `render`",
            ),
            (json!({"cmd":"job","job_id":0}), "job_id must be an integer"),
            (
                json!({"cmd":"note","note":1,"press":"hold"}),
                "press must be click, down or up",
            ),
        ] {
            let error = parse(value.clone()).unwrap_err();
            assert!(error.contains(message), "{value}: {error}");
        }
    }

    /// The fields each command has always had; extra fields are ignored.
    #[test]
    fn requests_serialize_as_the_wire_names_them() {
        let wait = Request::Wait {
            until: Until::Loaded { photo_id: 4 },
            timeout_ms: Some(1000),
        };
        assert_eq!(
            serde_json::to_value(wait).unwrap(),
            json!({"cmd":"wait","until":"loaded","photo_id":4,"timeout_ms":1000})
        );
        let note = Request::Note {
            note: 9,
            press: Press::Down,
        };
        assert_eq!(
            serde_json::to_value(note).unwrap(),
            json!({"cmd":"note","note":9,"press":"down"})
        );
        assert_eq!(
            serde_json::to_value(Request::Presets { group: None }).unwrap(),
            json!({"cmd":"presets"})
        );
        assert_eq!(
            parse(json!({"cmd":"save","verbose":true})).unwrap(),
            Request::Save
        );
        assert_eq!(
            parse(json!({"cmd":"build_previews"})).unwrap(),
            Request::BuildPreviews {
                kind: PreviewKind::Standard
            }
        );
        assert_eq!(
            parse(json!({"cmd":"build_previews","kind":"standard"})).unwrap(),
            Request::BuildPreviews {
                kind: PreviewKind::Standard
            }
        );
        assert_eq!(
            parse(json!({"cmd":"build_previews","kind":"one_to_one"})).unwrap(),
            Request::BuildPreviews {
                kind: PreviewKind::OneToOne
            }
        );
        assert!(parse(json!({"cmd":"build_previews","kind":"huge"})).is_err());
        assert_eq!(
            parse(json!({"cmd":"build_editable_previews"})).unwrap(),
            Request::BuildEditablePreviews
        );
        assert_eq!(
            parse(json!({"cmd":"discard_editable_previews"})).unwrap(),
            Request::DiscardEditablePreviews {
                include_offline: false
            }
        );
        assert_eq!(
            parse(json!({"cmd":"discard_editable_previews","include_offline":true})).unwrap(),
            Request::DiscardEditablePreviews {
                include_offline: true
            }
        );
        assert!(parse(json!({"cmd":"discard_editable_previews","include_offline":"yes"})).is_err());
    }
}
