//! Updates from GitHub releases, through fastframe-update: the check, whether
//! this copy may replace itself, the signed download and the helper that
//! installs it with rollback. RAWmakase keeps its configuration, its HTTP
//! client and the notice (`app::updates`).
//!
//! The Apple Silicon disk image and the Windows installer update themselves.
//! Intel Macs, the Linux tarball (which bundles libraries beside the
//! executable), the Windows archive and package-managed installs are told
//! about a release and pointed at its page.
use anyhow::Result;
pub use fastframe_update::{Launch, Prepared, Release, Unsupported, Updater};
use fastframe_update::{MacConfig, MacTarget, Request, Response, Transport, UpdateConfig};
use std::sync::OnceLock;
use std::time::Duration;

/// Checks after launch and then hourly while the app runs.
pub const INTERVAL: Duration = Duration::from_secs(60 * 60);

const CONFIG: UpdateConfig = UpdateConfig {
    macos: MacConfig {
        bundle_ids: &["io.github.pch.rawmakase"],
        executable_names: &[],
        legacy_bundle_names: &[],
    },
    mac_target: MacTarget::Arm64Only,
    publisher_key: Some(include_str!("../assets/update-public-key.hex")),
    ..UpdateConfig::new(
        "pch/rawmakase",
        "RAWmakase",
        "rawmakase",
        env!("CARGO_PKG_VERSION"),
    )
};

/// The configuration, with `RAWMAKASE_PRETEND_VERSION` in place of this
/// build's version when it is older, to try the notice against a real release.
pub fn config() -> UpdateConfig {
    static CONFIG_WITH_VERSION: OnceLock<UpdateConfig> = OnceLock::new();
    *CONFIG_WITH_VERSION.get_or_init(|| {
        let mut config = CONFIG;
        if let Ok(version) = std::env::var("RAWMAKASE_PRETEND_VERSION")
            && fastframe_update::is_newer(CONFIG.current_version, &version)
        {
            config.current_version = Box::leak(version.into_boxed_str());
        }
        config
    })
}

/// Runs the update helper when this process was started as one (it then
/// exits), and takes the update flags off the command line. First in `main`.
pub fn intercept() -> Launch {
    fastframe_update::intercept(&config())
}

pub fn updater() -> Updater {
    Updater::new(config(), UreqTransport::new())
}

/// fastframe-update's HTTP client over ureq. It must not follow redirects:
/// the updater follows them itself, only to GitHub's release hosts.
struct UreqTransport(ureq::Agent);
impl UreqTransport {
    fn new() -> Self {
        Self(
            ureq::Agent::config_builder()
                .max_redirects(0)
                .http_status_as_error(false)
                .timeout_connect(Some(Duration::from_secs(15)))
                .timeout_global(Some(Duration::from_secs(15 * 60)))
                .build()
                .into(),
        )
    }
}
impl Transport for UreqTransport {
    fn get(&self, request: &Request<'_>) -> Result<Response> {
        let response = self
            .0
            .get(request.url)
            .header("Accept", request.accept)
            .header("User-Agent", request.user_agent)
            .call()?;
        let status = response.status().as_u16();
        let location = response
            .headers()
            .get("location")
            .and_then(|v| v.to_str().ok())
            .map(String::from);
        // The updater bounds what it reads: manifests by size, packages by
        // their checksum.
        let body = response.into_body().into_with_config().limit(u64::MAX);
        Ok(Response {
            status,
            location,
            body: Box::new(body.reader()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_update_configuration_is_valid() {
        CONFIG.validate().unwrap();
        assert_eq!(CONFIG.current_version, env!("CARGO_PKG_VERSION"));
        assert!(updater().source().is_github());
    }
}
