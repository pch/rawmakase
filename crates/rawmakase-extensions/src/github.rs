//! Installing and updating from a GitHub repository's releases.
//!
//! [`inspect`] reads the repository's releases, newest first, and downloads
//! the first package for this target (or `any`) whose manifest's `rawmakase`
//! range includes this version. It returns an [`Offer`] holding the
//! downloaded, inspected package, so the app can show the manifest and the
//! "Not reviewed" trust warning before [`Store::install_offer`] installs it.
//! [`check_updates`] does the same for every extension installed from GitHub
//! and says when an update asks for more access.
use crate::package::{self, EXTENSION};
use crate::state::{Source, State};
use crate::store::{Installed, Store};
use crate::{Error, Manifest, Result};
use serde::Deserialize;
use std::io::Read;
use std::path::PathBuf;
use std::time::Duration;

/// The largest download, a package or the releases list.
pub const MAX_DOWNLOAD: u64 = package::MAX_UNPACKED;
/// The most releases looked at, and packages downloaded, for one inspect.
const MAX_CANDIDATES: usize = 10;
/// The largest releases list read.
const MAX_LISTING: u64 = 8 << 20;

/// Reads a URL; [`Https`] in the app, a table of answers in tests.
pub trait Fetch {
    /// The body at `url`, at most `limit` bytes, failing past it.
    fn get(&self, url: &str, limit: u64) -> Result<Vec<u8>>;
}

/// HTTPS only, through ureq with rustls and bundled roots.
pub struct Https;
impl Fetch for Https {
    fn get(&self, url: &str, limit: u64) -> Result<Vec<u8>> {
        let agent = ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(20)))
            .timeout_recv_response(Some(Duration::from_secs(30)))
            .timeout_recv_body(Some(Duration::from_secs(120)))
            .https_only(true)
            .user_agent("RAWmakase")
            .build()
            .new_agent();
        let mut response = agent
            .get(url)
            .header(
                "Accept",
                "application/vnd.github+json, application/octet-stream",
            )
            .call()
            .map_err(|e| Error::new(format!("Could not reach {url}: {e}")))?;
        if response
            .body()
            .content_length()
            .is_some_and(|len| len > limit)
        {
            return Err(Error::new(format!("{url} is larger than expected")));
        }
        let mut out = Vec::new();
        response
            .body_mut()
            .as_reader()
            .take(limit + 1)
            .read_to_end(&mut out)
            .map_err(|e| Error::new(format!("Downloading {url}: {e}")))?;
        if out.len() as u64 > limit {
            return Err(Error::new(format!("{url} is larger than expected")));
        }
        Ok(out)
    }
}

/// `https://github.com/<owner>/<repo>` (with or without `.git` or a trailing
/// slash) as `(owner, repo)`.
pub fn repository(url: &str) -> Result<(String, String)> {
    let bad = || {
        Error::new(format!(
            "{url} is not a GitHub repository; paste one like https://github.com/owner/repository"
        ))
    };
    let rest = url
        .trim()
        .strip_prefix("https://github.com/")
        .ok_or_else(bad)?;
    let rest = rest.trim_end_matches('/');
    let rest = rest.strip_suffix(".git").unwrap_or(rest);
    let (owner, repo) = rest.split_once('/').ok_or_else(bad)?;
    let plain = |s: &str| {
        !s.is_empty()
            && s != "."
            && s != ".."
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    if plain(owner) && plain(repo) {
        Ok((owner.to_string(), repo.to_string()))
    } else {
        Err(bad())
    }
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<Asset>,
}
#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
}

/// A downloaded package, inspected and ready to install. Its download is
/// removed when the offer is dropped, installed or not.
#[derive(Debug)]
pub struct Offer {
    /// The package's manifest.
    pub manifest: Manifest,
    /// The repository, `https://github.com/<owner>/<repo>`.
    pub repository: String,
    /// The release's tag.
    pub release: String,
    /// The asset's name.
    pub asset: String,
    file: PathBuf,
}
impl Drop for Offer {
    fn drop(&mut self) {
        if let Some(folder) = self.file.parent() {
            let _ = std::fs::remove_dir_all(folder);
        }
    }
}

/// Finds and downloads the newest package of `url`'s releases that runs here.
pub fn inspect(
    store: &Store,
    fetch: &dyn Fetch,
    url: &str,
    app: &semver::Version,
) -> Result<Offer> {
    let (owner, repo) = repository(url)?;
    let repository = format!("https://github.com/{owner}/{repo}");
    let listing = fetch.get(
        &format!("https://api.github.com/repos/{owner}/{repo}/releases?per_page=30"),
        MAX_LISTING,
    )?;
    let releases: Vec<Release> = serde_json::from_slice(&listing)
        .map_err(|e| Error::new(format!("GitHub's list of releases can't be read: {e}")))?;
    let target = crate::current_target();
    let mut refusals = Vec::new();
    let candidates = releases
        .iter()
        .filter(|r| !r.draft && !r.prerelease)
        .filter_map(|r| {
            // This target's package over one for any target.
            let ours = |suffix: &str| {
                r.assets
                    .iter()
                    .find(|a| a.name.ends_with(&format!("-{suffix}.{EXTENSION}")))
            };
            ours(&target)
                .or_else(|| ours(crate::ANY_TARGET))
                .map(|a| (r, a))
        })
        .take(MAX_CANDIDATES);
    for (release, asset) in candidates {
        match download(store, fetch, asset).and_then(|file| offer(store, file, app)) {
            Ok((manifest, file)) => {
                return Ok(Offer {
                    manifest,
                    repository,
                    release: release.tag_name.clone(),
                    asset: asset.name.clone(),
                    file,
                });
            }
            Err(e) => refusals.push(format!("{}: {e}", release.tag_name)),
        }
    }
    let mut message = format!(
        "No release of {repository} has a package for {target} that supports RAWmakase {app}"
    );
    if !refusals.is_empty() {
        message += &format!(" ({})", refusals.join("; "));
    }
    Err(Error::new(message))
}

fn download(store: &Store, fetch: &dyn Fetch, asset: &Asset) -> Result<PathBuf> {
    if !asset
        .browser_download_url
        .starts_with("https://github.com/")
    {
        return Err(Error::new(format!(
            "{} is not downloaded from github.com",
            asset.name
        )));
    }
    if asset.size > MAX_DOWNLOAD {
        return Err(Error::new(format!("{} is too large", asset.name)));
    }
    if !crate::protocol::manifest::is_package_path(&asset.name) || asset.name.contains('/') {
        return Err(Error::new(format!(
            "{} is not a package's name",
            asset.name
        )));
    }
    let bytes = fetch.get(&asset.browser_download_url, MAX_DOWNLOAD)?;
    // The asset's own name, which states its target, in a folder of its own.
    let folder = store.download_dir()?;
    let file = folder.join(&asset.name);
    if let Err(e) = std::fs::write(&file, bytes) {
        let _ = std::fs::remove_dir_all(&folder);
        return Err(e.into());
    }
    Ok(file)
}

/// Inspects a downloaded package; a refused one is removed.
fn offer(store: &Store, file: PathBuf, app: &semver::Version) -> Result<(Manifest, PathBuf)> {
    match store.inspect(&file, app) {
        Ok(inspected) => Ok((inspected.manifest, file)),
        Err(e) => {
            if let Some(folder) = file.parent() {
                let _ = std::fs::remove_dir_all(folder);
            }
            Err(e)
        }
    }
}

impl Offer {
    /// The downloaded package.
    pub fn file(&self) -> &std::path::Path {
        &self.file
    }
}

impl Store {
    /// Installs an offer the user accepted.
    pub fn install_offer(&self, offer: Offer, app: &semver::Version) -> Result<Installed> {
        let source = Source::Github {
            repository: offer.repository.clone(),
        };
        self.install(&offer.file, &source, app)
    }
}

/// A newer compatible version of an installed extension.
#[derive(Debug)]
pub struct Update {
    /// The extension's id.
    pub id: String,
    /// The version installed and active.
    pub installed: semver::Version,
    /// The newer version, downloaded and inspected.
    pub offer: Offer,
    /// What the newer version asks for that the installed one didn't
    /// ([`Access::more_than`](crate::protocol::manifest::Access::more_than)).
    pub more_access: Vec<String>,
}

/// What [`check_updates`] found.
#[derive(Debug, Default)]
pub struct Checked {
    /// The updates found.
    pub updates: Vec<Update>,
    /// The extensions that couldn't be checked, by id, with why.
    pub failed: Vec<(String, Error)>,
}

/// Reads the releases of every extension installed from GitHub.
pub fn check_updates(store: &Store, fetch: &dyn Fetch, app: &semver::Version) -> Result<Checked> {
    let state = State::read(store.data_dir())?;
    let mut updates = Vec::new();
    let mut failed = Vec::new();
    for (id, entry) in &state.extensions {
        let (Some(Source::Github { repository }), Some(active)) = (entry.source(), &entry.active)
        else {
            continue;
        };
        let checked = (|| -> Result<Option<Update>> {
            let current = crate::read_manifest(&store.version_dir(id, active))?;
            let offer = inspect(store, fetch, &repository, app)?;
            if offer.manifest.extension.id != *id {
                return Err(Error::new(format!(
                    "{repository} now publishes {}, not {id}",
                    offer.manifest.extension.id
                )));
            }
            if offer.manifest.extension.version <= current.extension.version {
                return Ok(None);
            }
            Ok(Some(Update {
                id: id.clone(),
                installed: current.extension.version.clone(),
                more_access: offer.manifest.access.more_than(&current.access),
                offer,
            }))
        })();
        match checked {
            Ok(Some(update)) => updates.push(update),
            Ok(None) => {}
            Err(e) => failed.push((id.clone(), e)),
        }
    }
    Ok(Checked { updates, failed })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::tests::files;
    use serde_json::json;
    use std::collections::HashMap;

    /// Answers from a table; counts what it was asked.
    struct Table(HashMap<String, Vec<u8>>, std::cell::RefCell<Vec<String>>);
    impl Fetch for Table {
        fn get(&self, url: &str, limit: u64) -> Result<Vec<u8>> {
            self.1.borrow_mut().push(url.to_string());
            let body = self
                .0
                .get(url)
                .cloned()
                .ok_or_else(|| Error::new(format!("404 {url}")))?;
            assert!(body.len() as u64 <= limit);
            Ok(body)
        }
    }

    fn package_bytes(dir: &std::path::Path, version: &str, range: &str, network: bool) -> Vec<u8> {
        let mut files = files(version);
        let mut manifest = String::from_utf8(files[0].1.clone())
            .unwrap()
            .replace(">=0.1", range);
        if network {
            manifest = manifest.replace("[program]", "[access]\nnetwork = true\n[program]");
        }
        files[0].1 = manifest.into_bytes();
        let path = dir.join(format!("p-{version}.rmext"));
        package::write(&path, &files, None).unwrap();
        std::fs::read(path).unwrap()
    }

    fn releases(dir: &std::path::Path) -> Table {
        let target = crate::current_target();
        let base = "https://github.com/jane/tool/releases/download";
        let asset = |v: &str, t: &str| {
            json!({"name": format!("dev.test.tool-{v}-{t}.rmext"),
                   "browser_download_url": format!("{base}/v{v}/dev.test.tool-{v}-{t}.rmext"),
                   "size": 100})
        };
        let listing = json!([
            {"tag_name": "v3.0.0-beta", "prerelease": true, "assets": [asset("3.0.0", &target)]},
            {"tag_name": "v2.0.0", "assets": [asset("2.0.0", &target), asset("2.0.0", "plan9-mips")]},
            {"tag_name": "v1.1.0", "assets": [asset("1.1.0", &target)]},
            {"tag_name": "v1.0.0", "assets": [asset("1.0.0", &target)]},
        ]);
        let mut table = HashMap::new();
        table.insert(
            "https://api.github.com/repos/jane/tool/releases?per_page=30".to_string(),
            listing.to_string().into_bytes(),
        );
        for (v, range, network) in [
            ("2.0.0", ">=9.0", false),
            ("1.1.0", ">=0.1", true),
            ("1.0.0", ">=0.1", false),
        ] {
            table.insert(
                format!("{base}/v{v}/dev.test.tool-{v}-{target}.rmext"),
                package_bytes(dir, v, range, network),
            );
        }
        Table(table, Default::default())
    }

    #[test]
    fn repository_urls_are_read_strictly() {
        let ok = ("jane".to_string(), "tool".to_string());
        for url in [
            "https://github.com/jane/tool",
            "https://github.com/jane/tool/",
            "https://github.com/jane/tool.git",
        ] {
            assert_eq!(repository(url).unwrap(), ok, "{url}");
        }
        for url in [
            "http://github.com/jane/tool",
            "https://gitlab.com/jane/tool",
            "https://github.com/jane",
            "https://github.com/jane/tool/releases",
            "https://github.com/../tool",
        ] {
            assert!(repository(url).is_err(), "{url}");
        }
    }

    #[test]
    fn the_newest_compatible_release_is_offered_then_installed() {
        let data = tempfile::tempdir().unwrap();
        let store = Store::new(data.path());
        let fetch = releases(data.path());
        let app = semver::Version::new(0, 4, 0);
        // The beta is skipped, 2.0.0 needs RAWmakase 9, so 1.1.0 is offered.
        let offer = inspect(&store, &fetch, "https://github.com/jane/tool", &app).unwrap();
        assert_eq!(offer.release, "v1.1.0");
        assert_eq!(offer.manifest.extension.version.to_string(), "1.1.0");
        assert!(offer.file().is_file());
        let asked = fetch.1.borrow().clone();
        assert_eq!(asked.len(), 3, "the listing, 2.0.0 and 1.1.0: {asked:?}");
        let installed = store.install_offer(offer, &app).unwrap();
        assert_eq!(installed.manifest.extension.version.to_string(), "1.1.0");
        let entry = &store.list().unwrap()[0].entry;
        assert_eq!(
            entry.source(),
            Some(Source::Github {
                repository: "https://github.com/jane/tool".into()
            })
        );
        // Nothing downloaded is left behind.
        let left: Vec<_> = std::fs::read_dir(store.root())
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(left, ["dev.test.tool"]);
    }

    #[test]
    fn an_offer_not_accepted_leaves_nothing() {
        let data = tempfile::tempdir().unwrap();
        let store = Store::new(data.path());
        let fetch = releases(data.path());
        let app = semver::Version::new(0, 4, 0);
        let offer = inspect(&store, &fetch, "https://github.com/jane/tool", &app).unwrap();
        let file = offer.file().to_path_buf();
        drop(offer);
        assert!(!file.exists());
        assert!(State::read(data.path()).unwrap().extensions.is_empty());
        let none = inspect(&store, &fetch, "https://github.com/jane/other", &app);
        assert!(none.is_err());
        let too_old = inspect(
            &store,
            &fetch,
            "https://github.com/jane/tool",
            &semver::Version::new(0, 0, 1),
        );
        assert!(too_old.unwrap_err().0.contains("No release"));
    }

    #[test]
    fn an_update_says_when_it_asks_for_more_access() {
        let data = tempfile::tempdir().unwrap();
        let store = Store::new(data.path());
        let fetch = releases(data.path());
        let app = semver::Version::new(0, 4, 0);
        // 1.0.0 installed from the repository.
        let path = data.path().join(format!(
            "dev.test.tool-1.0.0-{}.rmext",
            crate::current_target()
        ));
        std::fs::write(&path, package_bytes(data.path(), "1.0.0", ">=0.1", false)).unwrap();
        let source = Source::Github {
            repository: "https://github.com/jane/tool".into(),
        };
        store.install(&path, &source, &app).unwrap();
        let Checked { updates, failed } = check_updates(&store, &fetch, &app).unwrap();
        assert!(failed.is_empty(), "{failed:?}");
        assert_eq!(updates.len(), 1);
        let update = &updates[0];
        assert_eq!(update.installed.to_string(), "1.0.0");
        assert_eq!(update.offer.manifest.extension.version.to_string(), "1.1.0");
        assert_eq!(update.more_access, ["the network"]);
        let update = updates.into_iter().next().unwrap();
        store.install_offer(update.offer, &app).unwrap();
        assert!(
            check_updates(&store, &fetch, &app)
                .unwrap()
                .updates
                .is_empty()
        );
    }
}
