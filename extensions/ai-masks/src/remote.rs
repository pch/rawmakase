//! Where selections run, when the user has said they run elsewhere: the
//! endpoint of a `rawmakase-remote-worker`, from `remote-inference.json` in
//! this computer's local data folder, where the app read it before this
//! extension took it over. Absent (the usual case) means the models run here,
//! as always. See docs/remote-inference.md.
//!
//! ```json
//! { "endpoint": "127.0.0.1:8765" }
//! ```
use std::path::Path;

/// Where the endpoint comes from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Remote {
    /// `RAWMAKASE_REMOTE_ENDPOINT`, else `remote-inference.json`, read at
    /// each request, so an edit to the file counts from the next one.
    #[default]
    Configured,
    /// This one, or none: for tests, which a developer's own configuration
    /// mustn't bend.
    Fixed(Option<String>),
}

impl Remote {
    /// The endpoint to send selections to, if there is one; `local_data`
    /// holds `remote-inference.json`.
    pub(crate) fn endpoint(&self, local_data: &Path) -> Option<String> {
        match self {
            Self::Configured => read_endpoint(local_data),
            Self::Fixed(endpoint) => endpoint.clone(),
        }
    }
}

/// The endpoint from the configuration file, when it is a usable one.
/// `RAWMAKASE_REMOTE_ENDPOINT` says the same thing for one launch, without
/// touching the file.
fn read_endpoint(local_data: &Path) -> Option<String> {
    if let Some(endpoint) = std::env::var_os("RAWMAKASE_REMOTE_ENDPOINT")
        .map(|v| v.to_string_lossy().trim().to_string())
        .filter(|text| !text.is_empty())
    {
        return Some(endpoint);
    }
    endpoint_in(&std::fs::read_to_string(local_data.join("remote-inference.json")).ok()?)
}

/// The endpoint `remote-inference.json`'s text names: a string without
/// whitespace inside, trimmed.
fn endpoint_in(text: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    value
        .get("endpoint")?
        .as_str()
        .map(str::trim)
        .filter(|text| !text.is_empty() && !text.chars().any(char::is_whitespace))
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_names_one_usable_endpoint_or_none() {
        assert_eq!(
            endpoint_in(r#"{ "endpoint": " 127.0.0.1:8765 " }"#).as_deref(),
            Some("127.0.0.1:8765")
        );
        for text in [
            r#"{ "endpoint": "" }"#,
            r#"{ "endpoint": "a b" }"#,
            r#"{ "endpoint": 8765 }"#,
            "{}",
            "not json",
        ] {
            assert_eq!(endpoint_in(text), None, "{text}");
        }
    }

    /// The file is read from the folder it is given, the app's local data
    /// folder in the program; a fixed endpoint ignores it.
    #[test]
    fn the_file_is_read_from_the_local_data_folder() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("remote-inference.json"),
            r#"{ "endpoint": "127.0.0.1:9" }"#,
        )
        .unwrap();
        // The variable would win; it is set only in a developer's shell.
        if std::env::var_os("RAWMAKASE_REMOTE_ENDPOINT").is_none() {
            assert_eq!(
                Remote::Configured.endpoint(dir.path()).as_deref(),
                Some("127.0.0.1:9")
            );
        }
        assert_eq!(Remote::Fixed(None).endpoint(dir.path()), None);
    }
}
