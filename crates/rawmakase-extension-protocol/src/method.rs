//! The requests and events of protocol version 1, with their parameters.
//!
//! `mask.generate` and `mask.setup` serve `[[mask-generator]]`, which manifests
//! declare from `api = 2`.
//!
//! [`AppRequest`] and [`AppEvent`] go from the app to the program,
//! [`ProgramRequest`] and [`ProgramEvent`] the other way. Each reads from a
//! [`Message::Req`](crate::Message::Req)'s method and parameters with `parse`,
//! which tells a method this version doesn't know
//! ([`ErrorCode::UnknownMethod`]) from parameters that don't fit it
//! ([`ErrorCode::InvalidRequest`]). Parameters ignore fields they don't know.
use crate::buffer::{
    Access, Alpha, Buffer, MAX_REQUEST_BYTES, Meaning, Primaries, Region, Sample, Space, Transfer,
    White,
};
use crate::controls::Control;
use crate::message::{ErrorCode, RemoteError};
use crate::viewer::ViewerInput;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::path::PathBuf;

/// Requests the app sends a program.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "m", content = "p")]
pub enum AppRequest {
    /// The handshake, the first message of a session. Answered with
    /// [`HelloReply`].
    #[serde(rename = "hello")]
    Hello(Hello),
    /// Runs a command. Answered with whatever the command returns (`null` for
    /// nothing); its edits arrive meanwhile as `propose.settings`.
    #[serde(rename = "command.run")]
    CommandRun(CommandRun),
    /// A panel is shown. Answered with [`Controls`].
    #[serde(rename = "panel.open")]
    PanelOpen(PanelOpen),
    /// The user committed a control's new value. Answered with [`Controls`].
    #[serde(rename = "panel.changed")]
    PanelChanged(PanelChanged),
    /// The user clicked a button. Answered with [`Controls`].
    #[serde(rename = "panel.clicked")]
    PanelClicked(PanelClicked),
    /// Generates a mask's coverage. Answered with [`MaskGenerated`].
    #[serde(rename = "mask.generate")]
    MaskGenerate(MaskGenerate),
    /// Installs what a mask generator needs, such as its model, after it
    /// answered `needs_setup` and the user agreed. Answered with `null`.
    #[serde(rename = "mask.setup")]
    MaskSetup(MaskSetup),
}

/// Requests a program sends the app.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "m", content = "p")]
pub enum ProgramRequest {
    /// What the app's control layer can do: its parameters and actions, as
    /// `rawmakase control capabilities` prints them.
    #[serde(rename = "read.capabilities")]
    ReadCapabilities(Empty),
    /// Whether the app runs an action written in the control layer's
    /// notation (an action name, `cmd+shift+z`, `hold:shift`), so a program
    /// can check one a user typed. Answered with [`ActionSupport`].
    #[serde(rename = "read.action")]
    ReadAction(ReadAction),
    /// The Library's selection. Answered with [`Snapshot`].
    #[serde(rename = "read.selection")]
    ReadSelection(Empty),
    /// The photo open in Develop. Answered with [`Snapshot`] (no photo when
    /// none is open).
    #[serde(rename = "read.open-photo")]
    ReadOpenPhoto(Empty),
    /// Photos by id. Answered with [`Snapshot`].
    #[serde(rename = "read.photos")]
    ReadPhotos(ReadPhotos),
    /// Settings changes for the photos of a snapshot. Answered with
    /// [`Proposed`].
    #[serde(rename = "propose.settings")]
    ProposeSettings(ProposeSettings),
    /// A device of a control adapter connected, disconnected or went away.
    /// Answered with `null`.
    #[serde(rename = "adapter.device")]
    AdapterDevice(AdapterDevice),
    /// Operations from a device. Answered with [`Operated`].
    #[serde(rename = "adapter.operate")]
    AdapterOperate(AdapterOperate),
    /// A notification for the user. Answered with `null`.
    #[serde(rename = "notify")]
    Notify(Notify),
    /// The extension's entry in the status line, or none. Answered with `null`.
    #[serde(rename = "status")]
    Status(Status),
}

/// Events the app sends a program.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "e", content = "p")]
pub enum AppEvent {
    /// One of the extension's settings changed.
    #[serde(rename = "setting-changed")]
    SettingChanged(SettingChanged),
    /// The Library's selection changed; read it with `read.selection`.
    #[serde(rename = "selection-changed")]
    SelectionChanged(Empty),
    /// Another photo opened in Develop, or none.
    #[serde(rename = "photo-opened")]
    PhotoOpened(PhotoOpened),
    /// The catalog closed: tokens from it are refused from now on.
    #[serde(rename = "catalog-closed")]
    CatalogClosed(Empty),
    /// The app is quitting: the channel closes next, and the program is ended
    /// if it hasn't exited by the quit deadline.
    #[serde(rename = "quitting")]
    Quitting(Empty),
}

/// Events a program sends the app.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "e", content = "p")]
pub enum ProgramEvent {
    /// New controls for an open panel.
    #[serde(rename = "panel-update")]
    PanelUpdate(PanelUpdate),
}

/// Reads a request or an event from its name and parameters.
macro_rules! parse_tagged {
    ($type:ty, $tag:literal, $content:literal) => {
        impl $type {
            /// Reads it from its name and parameters (`null` for none).
            pub fn parse(name: &str, params: Value) -> Result<Self, RemoteError> {
                parse_tagged::<Self>(Self::NAMES, $tag, $content, name, params)
            }
            /// Its name and parameters.
            pub fn into_parts(self) -> (String, Value) {
                into_parts(self, $tag, $content)
            }
        }
    };
}
parse_tagged!(AppRequest, "m", "p");
parse_tagged!(ProgramRequest, "m", "p");
parse_tagged!(AppEvent, "e", "p");
parse_tagged!(ProgramEvent, "e", "p");

impl AppRequest {
    /// Every method name.
    pub const NAMES: &[&str] = &[
        "hello",
        "command.run",
        "panel.open",
        "panel.changed",
        "panel.clicked",
        "mask.generate",
        "mask.setup",
    ];
}
impl ProgramRequest {
    /// Every method name.
    pub const NAMES: &[&str] = &[
        "read.capabilities",
        "read.action",
        "read.selection",
        "read.open-photo",
        "read.photos",
        "propose.settings",
        "adapter.device",
        "adapter.operate",
        "notify",
        "status",
    ];
    /// Whether the request reports a lifecycle change the host applies on its
    /// reader thread, outside the queue of operations, so it is never held up
    /// behind them (a device's disconnection behind a burst of its input).
    pub fn is_lifecycle(method: &str) -> bool {
        method == "adapter.device"
    }
}
impl AppEvent {
    /// Every event name.
    pub const NAMES: &[&str] = &[
        "setting-changed",
        "selection-changed",
        "photo-opened",
        "catalog-closed",
        "quitting",
    ];
}
impl ProgramEvent {
    /// Every event name.
    pub const NAMES: &[&str] = &["panel-update"];
}

fn parse_tagged<T: DeserializeOwned>(
    names: &[&str],
    tag: &str,
    content: &str,
    name: &str,
    params: Value,
) -> Result<T, RemoteError> {
    if !names.contains(&name) {
        return Err(RemoteError::new(
            ErrorCode::UnknownMethod,
            format!("{name} is not part of protocol {}", crate::PROTOCOL),
        ));
    }
    // A method without parameters may be sent without them.
    let params = if params.is_null() {
        Value::Object(Map::new())
    } else {
        params
    };
    let mut tagged = Map::new();
    tagged.insert(tag.into(), Value::String(name.into()));
    tagged.insert(content.into(), params);
    serde_json::from_value(Value::Object(tagged))
        .map_err(|e| RemoteError::new(ErrorCode::InvalidRequest, format!("{name}: {e}")))
}

fn into_parts<T: Serialize>(value: T, tag: &str, content: &str) -> (String, Value) {
    let Ok(Value::Object(mut tagged)) = serde_json::to_value(value) else {
        unreachable!("an adjacently tagged enum serializes as an object");
    };
    let name = match tagged.remove(tag) {
        Some(Value::String(name)) => name,
        _ => unreachable!("the tag is a string"),
    };
    (name, tagged.remove(content).unwrap_or(Value::Null))
}

/// No parameters. Reads from `{}` and ignores any fields.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Empty {}

/// `hello`'s parameters.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Hello {
    /// The app's [`crate::PROTOCOL`].
    pub protocol: u32,
    /// The app's version, for example `0.5.0`.
    pub app_version: String,
    /// The extension's id, as its manifest states it.
    pub extension: String,
    /// The extension's settings, by id, with their defaults filled in.
    #[serde(default)]
    pub settings: Map<String, Value>,
    /// The contributions its manifest declares, as `<table>/<id>` (for example
    /// `command/auto-film-look`, `control-adapter/midi`).
    #[serde(default)]
    pub contributions: Vec<String>,
    /// The extension's private data folder, kept across versions.
    pub data_dir: PathBuf,
    /// The extension's cache folder, which the app may empty.
    pub cache_dir: PathBuf,
    /// This program start's number, issued by the app, so that a restarted
    /// program's devices are never taken for the previous run's.
    pub session: u64,
}

/// What a program answers `hello` with.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloReply {
    /// The program's [`crate::PROTOCOL`].
    pub protocol: u32,
    /// The declared contributions it implements, as `<table>/<id>`.
    #[serde(default)]
    pub implements: Vec<String>,
}

/// `command.run`'s parameters.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CommandRun {
    /// The command's id within its extension.
    pub command: String,
    /// What the command applies to (its manifest's `applies-to`), read when it
    /// was run: none for `none`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<Snapshot>,
    /// Viewer input (its manifest's `input`); `null` for none.
    #[serde(default)]
    pub input: Value,
}

/// The longest [`MaskGenerated::model`] and [`MaskGenerated::model_key`], in
/// bytes.
pub const MAX_MODEL_LEN: usize = 128;

/// `mask.generate`'s parameters.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MaskGenerate {
    /// The mask generator's id within its extension.
    pub generator: String,
    /// The viewer input, or none for an automatic mask.
    #[serde(default)]
    pub input: Option<ViewerInput>,
    /// The photo as the model sees it: the whole image frame in 8-bit sRGB,
    /// three channels, read ([`MaskGenerate::IMAGE`]).
    pub image: Buffer,
    /// Where the program writes the coverage: 8-bit, one channel, the image's
    /// size and region, written.
    pub coverage: Buffer,
    /// The app's digest of the image's size, pixels and how it was rendered,
    /// so the program can keep its analysis of a photo without hashing.
    pub image_key: String,
}
impl MaskGenerate {
    /// What a mask request's image is: 8-bit sRGB, as the selection models
    /// take it.
    pub const IMAGE: Meaning = Meaning::Color {
        primaries: Primaries::Srgb,
        white: White::D65,
        transfer: Transfer::Srgb,
        alpha: Alpha::None,
    };

    /// Checks the input and both buffers ([`ViewerInput::validate`],
    /// [`Buffer::validate`]), and that they have the mask contract's shape,
    /// which is narrower than what a [`Buffer`] can describe:
    ///
    /// - the image is 3-channel `u8` [`MaskGenerate::IMAGE`] for reading, over
    ///   the whole image frame (`[0, 0, 1, 1]`, no overlap);
    /// - the coverage is 1-channel `u8` [`Meaning::Coverage`] for writing,
    ///   with the image's width, height and region;
    /// - they are in two handles, together within [`MAX_REQUEST_BYTES`].
    pub fn validate(&self) -> Result<(), RemoteError> {
        let fail = |why: &str| Err(RemoteError::new(ErrorCode::InvalidRequest, why));
        if let Some(input) = &self.input {
            input.validate()?;
        }
        self.image.validate()?;
        self.coverage.validate()?;
        let (image, coverage) = (&self.image, &self.coverage);
        let whole = |region: &Region| {
            region.space == Space::ImageFrame
                && region.rect == [0.0, 0.0, 1.0, 1.0]
                && region.overlap == 0
        };
        if image.channels != 3
            || image.sample != Sample::U8
            || image.meaning != Self::IMAGE
            || image.access != Access::Read
        {
            return fail("the image must be 3-channel 8-bit sRGB, for reading");
        }
        if !whole(&image.region) {
            return fail("the image must cover the whole image frame, without overlap");
        }
        if coverage.channels != 1
            || coverage.sample != Sample::U8
            || coverage.meaning != Meaning::Coverage
            || coverage.access != Access::Write
        {
            return fail("the coverage must be 1-channel 8-bit coverage, for writing");
        }
        if (image.width, image.height) != (coverage.width, coverage.height)
            || image.region != coverage.region
        {
            return fail("the coverage must match the image's size and region");
        }
        if image.handle == coverage.handle {
            return fail("the image and the coverage share a handle");
        }
        if image.bytes.saturating_add(coverage.bytes) > MAX_REQUEST_BYTES {
            return fail("the buffers are over the request's budget");
        }
        Ok(())
    }
}

/// What a program answers `mask.generate` with, once it has written the
/// coverage.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MaskGenerated {
    /// The model that made the mask, saved with it as its provenance, for
    /// example `sam2.1-hiera-small@<16 hex digits>`. At most
    /// [`MAX_MODEL_LEN`] bytes.
    pub model: String,
    /// Everything besides the pixels that the coverage depends on: the
    /// model's identity and weights, and the versions of its contract and
    /// processing. Saved in the mask's input identity, so a change here names
    /// a change in results. At most [`MAX_MODEL_LEN`] bytes.
    pub model_key: String,
    /// Set when nothing was found; the coverage is then not used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub empty: Option<EmptyMask>,
}
impl MaskGenerated {
    /// Checks that `model` and `model_key` are not empty and at most
    /// [`MAX_MODEL_LEN`] bytes.
    pub fn validate(&self) -> Result<(), RemoteError> {
        for (name, value) in [("model", &self.model), ("model_key", &self.model_key)] {
            if value.is_empty() || value.len() > MAX_MODEL_LEN {
                return Err(RemoteError::new(
                    ErrorCode::InvalidRequest,
                    format!("{name} is {} bytes, not 1 to {MAX_MODEL_LEN}", value.len()),
                ));
            }
        }
        Ok(())
    }
}

/// Why a mask generator found nothing, for the message the app shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmptyMask {
    /// No subject stands out.
    NoSubject,
    /// No sky is in the photo.
    NoSky,
    /// Nothing is where the user pointed.
    NothingThere,
}

/// `mask.setup`'s parameters.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MaskSetup {
    /// The mask generator's id within its extension.
    pub generator: String,
}

/// `panel.open`'s parameters.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PanelOpen {
    /// The panel's id within its extension.
    pub panel: String,
}

/// `panel.changed`'s parameters.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PanelChanged {
    /// The panel's id.
    pub panel: String,
    /// The control's id.
    pub control: String,
    /// The control's new value, of its kind's type.
    pub value: Value,
}

/// `panel.clicked`'s parameters.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PanelClicked {
    /// The panel's id.
    pub panel: String,
    /// The button's id.
    pub control: String,
}

/// What a program answers a `panel.*` request with, and `panel-update`'s
/// contents.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Controls {
    /// The panel's controls, whole.
    pub controls: Vec<Control>,
}

/// `panel-update`'s parameters.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PanelUpdate {
    /// The panel's id.
    pub panel: String,
    /// Its controls, whole.
    pub controls: Vec<Control>,
}

/// `setting-changed`'s parameters.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SettingChanged {
    /// The setting's id.
    pub id: String,
    /// Its new value.
    pub value: Value,
}

/// `photo-opened`'s parameters.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhotoOpened {
    /// The catalog id of the photo now open, or none.
    #[serde(default)]
    pub photo: Option<i64>,
}

/// `read.action`'s parameters.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadAction {
    /// The action, as an [`Operation::Action`] would carry it.
    pub spec: String,
}

/// What `read.action` answers.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionSupport {
    /// Whether a device's button may be bound to it: an action the app runs,
    /// a shortcut that stands for one, or a held modifier.
    pub supported: bool,
}

/// `read.photos`' parameters.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadPhotos {
    /// The photos' catalog ids.
    pub ids: Vec<i64>,
}

/// What a read returns: photos as they were when read, and the token that
/// names that state for a later proposal.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    /// Opaque to the program; sent back unchanged with a proposal, which the
    /// app refuses photo by photo where the state it names moved on.
    pub token: String,
    /// The photos read.
    pub photos: Vec<Photo>,
}

/// One photo of a [`Snapshot`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Photo {
    /// The catalog id.
    pub id: i64,
    /// The file's path on this computer.
    pub path: PathBuf,
    /// The photo's develop settings, in the control layer's names and units.
    #[serde(default)]
    pub settings: Map<String, Value>,
    /// Why its settings couldn't be read (an edit that can't be read, a file
    /// that can't be opened): `settings` is then empty, and a proposal for
    /// the photo is refused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// `propose.settings`' parameters.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProposeSettings {
    /// The id of the app's `command.run` request this proposal belongs to,
    /// which must still be in flight: a proposal after that request was
    /// answered, cancelled or timed out is refused with
    /// [`ErrorCode::NotAdmitted`] and changes nothing.
    pub op: u64,
    /// The token of the snapshot the changes were computed from.
    pub token: String,
    /// The changes, per photo.
    pub changes: Vec<SettingsChange>,
}

/// One photo's changes in a proposal.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SettingsChange {
    /// The catalog id, one of the snapshot's photos.
    pub photo: i64,
    /// The settings to set, in the control layer's names and units.
    pub settings: Map<String, Value>,
}

/// What `propose.settings` answers.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Proposed {
    /// The photos changed, each as one History step.
    pub applied: Vec<i64>,
    /// The photos refused, with why.
    pub refused: Vec<Refused>,
}

/// A photo a proposal didn't change.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Refused {
    /// The catalog id.
    pub photo: i64,
    /// Why: [`ErrorCode::StaleTarget`], or the control layer's code.
    pub code: ErrorCode,
}

/// `adapter.device`'s parameters.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdapterDevice {
    /// The control adapter's id within its extension.
    pub adapter: String,
    /// The device's id, stable while it stays connected.
    pub device: String,
    /// What happened to it.
    pub state: DeviceState,
    /// Counts up each time the device connects, so operations from before a
    /// disconnection are told apart from those after it.
    pub epoch: u64,
}

/// A device's lifecycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeviceState {
    /// The device is there and its operations are admitted.
    Connected,
    /// The device is gone for now; operations of its epoch are refused.
    Disconnected,
    /// The device is gone for good.
    Removed,
}

/// `adapter.operate`'s parameters.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AdapterOperate {
    /// The control adapter's id.
    pub adapter: String,
    /// The device the operations come from.
    pub device: String,
    /// The device's epoch they were made in; refused unless it is current.
    pub epoch: u64,
    /// The operations, applied in order with the app's device semantics.
    pub ops: Vec<Operation>,
}

/// What a device does.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum Operation {
    /// A relative turn of a parameter's dial, in ticks.
    Adjust {
        /// The control layer's parameter name.
        param: String,
        /// Signed ticks.
        ticks: i32,
    },
    /// An absolute position of a parameter's fader, mapped onto its range.
    Value {
        /// The control layer's parameter name.
        param: String,
        /// 0 to 127.
        value: u8,
    },
    /// A turn of the photo dial: the app accumulates `detent` ticks for one
    /// photo, and drops ticks where the dial does nothing (the Library grid).
    Photo {
        /// Signed ticks.
        ticks: i32,
        /// Ticks per photo.
        detent: u32,
    },
    /// An action in the control layer's notation (for example
    /// `undo`, `cmd+shift+z`).
    Action {
        /// The action.
        spec: String,
        /// Move to the next photo afterwards, as Shift does.
        #[serde(default)]
        advance: bool,
    },
}

/// What `adapter.operate` answers.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Operated {
    /// How many operations ran.
    pub applied: u32,
}

/// `notify`'s parameters.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notify {
    /// How it is shown.
    pub level: Level,
    /// The text.
    pub text: String,
}

/// A notification's weight.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    /// Something done.
    Info,
    /// Something the user should look at.
    Warning,
    /// Something that failed.
    Error,
}

/// `status`' parameters.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    /// The text the status line shows while the program works; none removes
    /// it.
    pub text: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn requests_read_from_their_method_and_parameters_and_back() {
        let request = AppRequest::parse(
            "panel.changed",
            json!({"panel": "midi", "control": "port", "value": "a", "later": 1}),
        )
        .unwrap();
        assert_eq!(
            request,
            AppRequest::PanelChanged(PanelChanged {
                panel: "midi".into(),
                control: "port".into(),
                value: json!("a"),
            })
        );
        let (name, params) = request.into_parts();
        assert_eq!(name, "panel.changed");
        assert_eq!(
            params,
            json!({"panel": "midi", "control": "port", "value": "a"})
        );
        assert_eq!(
            ProgramRequest::parse("read.selection", Value::Null).unwrap(),
            ProgramRequest::ReadSelection(Empty {})
        );
        assert_eq!(
            AppEvent::parse("quitting", Value::Null).unwrap(),
            AppEvent::Quitting(Empty {})
        );
    }

    #[test]
    fn an_unknown_method_is_told_from_parameters_that_do_not_fit() {
        let unknown = ProgramRequest::parse("read.everything", json!({})).unwrap_err();
        assert_eq!(unknown.code, ErrorCode::UnknownMethod);
        let invalid =
            ProgramRequest::parse("adapter.operate", json!({"adapter": "midi"})).unwrap_err();
        assert_eq!(invalid.code, ErrorCode::InvalidRequest);
        let range = ProgramRequest::parse(
            "adapter.operate",
            json!({"adapter": "midi", "device": "d", "epoch": 1,
                   "ops": [{"op": "value", "param": "exposure", "value": 300}]}),
        )
        .unwrap_err();
        assert_eq!(range.code, ErrorCode::InvalidRequest);
    }

    fn mask_request() -> Value {
        let buffer = |handle: u32, channels: u32, meaning: Value, access: &str| {
            json!({
                "handle": handle, "bytes": 64 * 48 * channels, "width": 64, "height": 48,
                "stride": 64 * channels, "channels": channels, "sample": "u8",
                "meaning": meaning,
                "region": {"space": "image_frame", "rect": [0, 0, 1, 1], "scale": 0.05,
                           "overlap": 0},
                "access": access
            })
        };
        let color = json!({"color": {"primaries": "srgb", "white": "d65",
                                     "transfer": "srgb", "alpha": "none"}});
        json!({
            "generator": "subject",
            "input": {"kind": "prompt", "clicks": [{"x": 0.5, "y": 0.5, "positive": true}]},
            "image": buffer(0, 3, color, "read"),
            "coverage": buffer(1, 1, json!("coverage"), "write"),
            "image_key": "0123456789abcdef"
        })
    }

    #[test]
    fn a_mask_request_reads_and_is_checked_whole() {
        let AppRequest::MaskGenerate(request) =
            AppRequest::parse("mask.generate", mask_request()).unwrap()
        else {
            panic!("a mask request");
        };
        request.validate().unwrap();
        let automatic = {
            let mut params = mask_request();
            params.as_object_mut().unwrap().remove("input");
            let AppRequest::MaskGenerate(request) =
                AppRequest::parse("mask.generate", params).unwrap()
            else {
                panic!("a mask request");
            };
            request
        };
        assert_eq!(automatic.input, None);
        automatic.validate().unwrap();
        let broken = |change: &dyn Fn(&mut MaskGenerate)| {
            let mut request = request.clone();
            change(&mut request);
            let error = request.validate().unwrap_err();
            assert_eq!(error.code, ErrorCode::InvalidRequest);
        };
        broken(&|r| r.input = Some(ViewerInput::Point { x: 2.0, y: 0.5 }));
        broken(&|r| r.image.stride = 1);
        broken(&|r| r.coverage.handle = 0);
        broken(&|r| r.coverage.width = 32);
        broken(&|r| r.coverage.region.rect = [0.0, 0.0, 0.5, 1.0]);
        broken(&|r| r.coverage.access = Access::Read);
        broken(&|r| r.image.access = Access::Write);
        broken(&|r| std::mem::swap(&mut r.image.meaning, &mut r.coverage.meaning));
        broken(&|r| {
            r.image.bytes = MAX_REQUEST_BYTES;
            r.coverage.bytes = MAX_REQUEST_BYTES;
        });
        let setup = AppRequest::parse("mask.setup", json!({"generator": "subject"})).unwrap();
        assert_eq!(
            setup,
            AppRequest::MaskSetup(MaskSetup {
                generator: "subject".into()
            })
        );
    }

    #[test]
    fn a_mask_result_names_its_model_in_bounded_strings() {
        let result: MaskGenerated = serde_json::from_value(
            json!({"model": "sam2.1-hiera-small@0123456789abcdef", "model_key": "k",
                   "empty": "no_subject"}),
        )
        .unwrap();
        assert_eq!(result.empty, Some(EmptyMask::NoSubject));
        result.validate().unwrap();
        let found: MaskGenerated =
            serde_json::from_value(json!({"model": "m", "model_key": "k"})).unwrap();
        assert_eq!(found.empty, None);
        assert_eq!(
            serde_json::to_value(&found).unwrap(),
            json!({"model": "m", "model_key": "k"})
        );
        let at_most = "x".repeat(MAX_MODEL_LEN);
        let long = MaskGenerated {
            model: at_most.clone(),
            model_key: at_most.clone(),
            empty: Some(EmptyMask::NothingThere),
        };
        long.validate().unwrap();
        let over = format!("{at_most}x");
        for result in [
            MaskGenerated {
                model: over.clone(),
                ..long.clone()
            },
            MaskGenerated {
                model_key: over,
                ..long.clone()
            },
            MaskGenerated {
                model: String::new(),
                ..long.clone()
            },
            MaskGenerated {
                model_key: String::new(),
                ..long
            },
        ] {
            assert_eq!(
                result.validate().unwrap_err().code,
                ErrorCode::InvalidRequest
            );
        }
        let unknown = serde_json::from_value::<MaskGenerated>(
            json!({"model": "m", "model_key": "k", "empty": "no_cat"}),
        );
        assert!(unknown.is_err());
    }

    #[test]
    fn every_listed_name_is_a_variant() {
        for name in AppRequest::NAMES {
            let e = AppRequest::parse(name, json!({})).err();
            assert_ne!(e.map(|e| e.code), Some(ErrorCode::UnknownMethod), "{name}");
        }
        let operate = ProgramRequest::AdapterOperate(AdapterOperate {
            adapter: "midi".into(),
            device: "loupedeck".into(),
            epoch: 2,
            ops: vec![
                Operation::Adjust {
                    param: "exposure".into(),
                    ticks: -3,
                },
                Operation::Photo {
                    ticks: 1,
                    detent: 2,
                },
                Operation::Action {
                    spec: "history.undo".into(),
                    advance: false,
                },
            ],
        });
        let (name, params) = operate.clone().into_parts();
        assert!(ProgramRequest::NAMES.contains(&name.as_str()));
        assert_eq!(ProgramRequest::parse(&name, params).unwrap(), operate);
        assert!(ProgramRequest::is_lifecycle("adapter.device"));
        assert!(!ProgramRequest::is_lifecycle("adapter.operate"));
    }
}
