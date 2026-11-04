//! The contract between RAWmakase and the clients that control it: the
//! `rawmakase control` command, `rawmakase-ctl` and the MCP server.
//!
//! The app listens on a loopback socket and announces it in [`Endpoint`], a
//! `control.json` file in its data folder ([`paths::data_dir`]). Clients read
//! that file, connect, and send one JSON request per line carrying
//! [`PROTOCOL`] and the endpoint's token, beside one [`Request`].
//!
//! This crate depends on nothing of the app's, so a client builds without its
//! GUI, GPU or native decoding libraries.
// Every public item says what it is for: this crate is a contract other crates
// and clients build on (#808 step 4.13).
#![warn(missing_docs)]
pub mod paths;
pub mod request;

pub use request::Request;

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The version of the request and reply format. Both sides refuse any other.
pub const PROTOCOL: u32 = 1;

/// The name of the file announcing the socket, in the app's data folder.
pub const ENDPOINT_FILE: &str = "control.json";

/// Which document state a command applies to. Each field a client sets must
/// match the app's state, or the command is refused as stale: read the state
/// first and send its `photo_id`, `generation` and `revision` back.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    /// The catalog photo the command is meant for.
    pub photo_id: Option<i64>,
    /// The document generation: it changes whenever another photo is opened.
    pub generation: Option<u64>,
    /// The edit's revision: it changes with every edit.
    pub revision: Option<u64>,
    /// A mask index from the state. Requires `generation` and `revision`, since
    /// masks have no lasting ids and can be removed or reordered meanwhile.
    pub mask: Option<usize>,
}

/// The tone curve a `curve` command sets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CurveChannel {
    /// The composite curve, of the three channels together.
    Rgb,
    /// The red channel's own curve.
    Red,
    /// The green channel's own curve.
    Green,
    /// The blue channel's own curve.
    Blue,
}

/// What a `curve` command's points must satisfy.
pub mod curve {
    /// The fewest and most points a curve has.
    pub const POINT_COUNT: [usize; 2] = [2, 32];
    /// How far apart consecutive inputs must be, strictly increasing.
    pub const MINIMUM_INPUT_SPACING: f32 = 0.00049;
}

/// Where the running app listens, as `control.json` holds it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Endpoint {
    /// The [`PROTOCOL`] the app speaks; a client refuses another.
    pub protocol: u32,
    /// A loopback TCP port.
    pub port: u16,
    /// Sent with every request; only someone who can read the file knows it.
    pub token: String,
    /// The app's process, so a stale file can be told from a live one. Clients
    /// need only the port and token, so they accept a file without it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
}
impl Endpoint {
    /// The endpoint file in `data_dir`.
    pub fn path(data_dir: &Path) -> PathBuf {
        data_dir.join(ENDPOINT_FILE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_target_omits_nothing_and_refuses_unknown_fields() {
        let target = Target {
            photo_id: Some(7),
            generation: Some(3),
            revision: Some(12),
            mask: None,
        };
        let json = serde_json::to_value(target).unwrap();
        assert_eq!(serde_json::from_value::<Target>(json).unwrap(), target);
        assert!(serde_json::from_str::<Target>(r#"{"photo":7}"#).is_err());
        assert_eq!(
            serde_json::to_string(&CurveChannel::Rgb).unwrap(),
            r#""rgb""#
        );
    }

    /// The field names `control.json` has always had: older clients read them.
    #[test]
    fn the_endpoint_file_keeps_its_format() {
        let endpoint = Endpoint {
            protocol: PROTOCOL,
            port: 51234,
            token: "secret".into(),
            pid: Some(42),
        };
        let json = serde_json::to_string(&endpoint).unwrap();
        assert_eq!(
            json,
            r#"{"protocol":1,"port":51234,"token":"secret","pid":42}"#
        );
        assert_eq!(serde_json::from_str::<Endpoint>(&json).unwrap(), endpoint);
        let without_pid = r#"{"protocol":1,"port":51234,"token":"secret"}"#;
        assert_eq!(
            serde_json::from_str::<Endpoint>(without_pid).unwrap().pid,
            None
        );
    }
}
