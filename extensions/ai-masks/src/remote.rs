//! Where selections run: here, or on the remote worker the user configured
//! (docs/remote-inference.md).
use std::time::Duration;

/// Where selections run, when the user has said they run elsewhere: the
/// endpoint of a `rawmakase-remote-worker`, from `remote-inference.json` in
/// this computer's data folder. Absent — the usual case — means the models run
/// here, as always. See docs/remote-inference.md.
///
/// ```json
/// { "endpoint": "127.0.0.1:8765" }
/// ```
#[derive(Default)]
pub(crate) struct RemoteConfig {
    /// What the file said, and when: read again at most once a second, so the
    /// drawer notices an edit without stat on every frame. Tests set this
    /// directly.
    seen: std::cell::Cell<Option<(std::time::Instant, Option<String>)>>,
}

impl RemoteConfig {
    /// The endpoint to send selections to, if there is one.
    pub(crate) fn endpoint(&self) -> Option<String> {
        match self.seen.take() {
            Some((at, endpoint)) if at.elapsed() < Duration::from_secs(1) => {
                self.seen.set(Some((at, endpoint.clone())));
                endpoint
            }
            _ => {
                let endpoint = read_endpoint();
                self.seen
                    .set(Some((std::time::Instant::now(), endpoint.clone())));
                endpoint
            }
        }
    }
    /// Pretends the file said this, for the tests. It is dated an hour ahead,
    /// so a test that stalls past the one-second reread on a loaded runner
    /// still sees it rather than the developer's own file.
    #[cfg(test)]
    pub(crate) fn force(&self, endpoint: Option<&str>) {
        self.seen.set(Some((
            std::time::Instant::now() + Duration::from_secs(3600),
            endpoint.map(str::to_string),
        )));
    }
}

/// The endpoint from the configuration file, when it is a usable one.
/// `RAWMAKASE_REMOTE_ENDPOINT` says the same thing for one launch, without
/// touching the file.
fn read_endpoint() -> Option<String> {
    if let Some(endpoint) = std::env::var_os("RAWMAKASE_REMOTE_ENDPOINT")
        .map(|v| v.to_string_lossy().trim().to_string())
        .filter(|text| !text.is_empty())
    {
        return Some(endpoint);
    }
    let path = rawmakase_protocol::paths::local_data_dir().join("remote-inference.json");
    let text = std::fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    value
        .get("endpoint")?
        .as_str()
        .map(str::trim)
        .filter(|text| !text.is_empty() && !text.chars().any(char::is_whitespace))
        .map(str::to_string)
}
