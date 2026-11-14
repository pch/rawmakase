//! Upgrading a catalog of an earlier format to the current one when it opens: a
//! verified backup beside it, then one transaction that brings every stored row up
//! to date and raises the version, then `VACUUM` to give the disk back the space.
//! A release that cannot read the result refuses it, the file left unchanged.
//!
//! Version 3 is the one engine's: every stored recipe (edits, History, snapshots)
//! loses the settings earlier releases chose an engine or operator with
//! (`saved_format::OBSOLETE_SETTINGS`). Versions 4 to 6 only marked what releases
//! before them could not render (Lensfun choices, AI Denoise, HDR). Version 7
//! ([`PROFILE_ASSETS`]) stores each camera profile and Lensfun calibration once, in
//! `recipe_assets`, and every stored recipe, History state and snapshot names them
//! by content id instead of holding a copy; `preset_settings`, a copy of an applied
//! XMP preset's settings that nothing read, is dropped.
//!
//! The rows are rewritten as JSON, so everything else in them stays as stored,
//! settings and History fields of a later release included, and read back in the
//! same transaction to check they still give the same recipes, through the
//! transaction alone (never the values this process holds). A row this release
//! cannot read is left as it is and counted.
use super::db::{NEWEST, PROFILE_ASSETS, Write};
use super::recipe_assets::ReadsSource;
use super::{Catalog, CatalogLocation};
use crate::camera_profiles::CameraProfile;
use crate::model::raw_json::{RawObject, raw, raw_of};
use crate::model::recipe::Recipe;
use crate::model::saved_format::is_obsolete;
use crate::model::saved_history::SavedHistory;
use crate::optics::lensfun::Calibration;
use crate::storage::recipe_assets::{Asset, Verified};
use anyhow::{Context, Result};
use serde_json::value::RawValue;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Rows read and rewritten at a time: what an upgrade holds in memory is bounded by
/// a batch (an edit with its profile inline is up to about 300 KB), however large
/// the catalog.
pub(super) const BATCH: i64 = 128;
/// Most profiles and calibrations an upgrade remembers by the text they were
/// stored as, so thousands of copies of one profile are read once while thousands
/// of distinct fitted looks don't accumulate.
const REMEMBERED: usize = 256;

/// How far an upgrade has got.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UpgradeProgress {
    pub done: usize,
    pub total: usize,
}
impl std::fmt::Display for UpgradeProgress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Upgrading catalog: {} / {} edits",
            thousands(self.done),
            thousands(self.total)
        )
    }
}
fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// What upgrading a catalog as it opened did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Upgraded {
    /// The format it had.
    pub from: i64,
    /// The verified copy of it as it was, beside it.
    pub backup: PathBuf,
    /// Stored edits, History and snapshots this release could not read, left as
    /// they were.
    pub unreadable: usize,
    /// Why giving the disk back the space failed, when it did: harmless, the
    /// catalog is upgraded.
    pub vacuum_error: Option<String>,
}

impl Catalog {
    #[cfg(any(test, feature = "test-support"))]
    /// The catalog's format version.
    pub fn format_version(&self) -> Result<i64> {
        self.db.version()
    }
    /// What upgrading the catalog as it opened did; `None` when it was current.
    pub fn upgraded(&self) -> Option<&Upgraded> {
        self.upgraded.as_ref()
    }
    /// Upgrades a catalog of an earlier format to the current one, as it opens:
    /// a verified backup beside it, one transaction, then `VACUUM`. When the backup
    /// or the transaction fails, the catalog is left unchanged and opening it fails
    /// with the reason.
    pub(super) fn upgrade_on_open(
        &mut self,
        progress: &mut dyn FnMut(UpgradeProgress),
    ) -> Result<()> {
        let from = self.db.version()?;
        if from >= NEWEST {
            return Ok(());
        }
        let CatalogLocation::File(path) = &self.location;
        let backup = backup_path(path)?;
        let mut upgrade = None;
        self.db.upgrade(
            NEWEST,
            &backup,
            |w, _| {
                let mut u = Upgrade::new(w, progress)?;
                migrate(w, &mut u)?;
                upgrade = Some(u.unreadable);
                Ok(())
            },
            |_| Ok(()),
        )?;
        let vacuum_error = self.db.vacuum().err().map(|e| format!("{e:#}"));
        self.upgraded = Some(Upgraded {
            from,
            backup,
            unreadable: upgrade.unwrap_or(0),
            vacuum_error,
        });
        Ok(())
    }
    /// Upgrades a catalog of an earlier format to the current one, after writing
    /// a consistent backup next to it. Returns the backup's path, or `None` when it
    /// already was current. Catalogs are upgraded as they open, so this finds them
    /// current.
    pub fn upgrade_format(&mut self) -> Result<Option<PathBuf>> {
        if self.db.version()? >= NEWEST {
            return Ok(None);
        }
        let CatalogLocation::File(path) = &self.location;
        let backup = backup_path(path)?;
        self.db.upgrade(
            NEWEST,
            &backup,
            |w, _| migrate(w, &mut Upgrade::new(w, &mut |_| {})?),
            |_| Ok(()),
        )?;
        Ok(Some(backup))
    }
    /// The write that stores a recipe (an edit, its History, a snapshot), checked
    /// beforehand by the caller. Catalogs are upgraded to the current format as they
    /// open, so this is a plain write; an older one would be upgraded in the same
    /// transaction, after its backup.
    pub(super) fn write_recipes<R>(
        &mut self,
        stores: Stores,
        write: impl FnOnce(&mut Write<'_>) -> Result<R>,
    ) -> Result<R> {
        let target = stores.version();
        if self.db.version()? >= target {
            return self.db.write(write);
        }
        let CatalogLocation::File(path) = &self.location;
        let backup = backup_path(path)?;
        self.db.upgrade(
            NEWEST,
            &backup,
            |w, _| migrate(w, &mut Upgrade::new(w, &mut |_| {})?),
            write,
        )
    }
}

/// What a write of recipes stores, for the format it needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Stores {
    Recipes,
    /// A Lensfun lens choice, in use or not, resolved or not.
    Lensfun,
    /// AI Denoise not at its default (on, or its Amount moved), with or without a
    /// Lensfun lens choice.
    AiDenoise,
    /// HDR on, with or without the others.
    Hdr,
}
impl Stores {
    /// For a write of `recipes`.
    pub(super) fn of<'r>(
        recipes: impl IntoIterator<Item = &'r crate::model::recipe::Recipe>,
    ) -> Self {
        recipes
            .into_iter()
            .map(|r| {
                if r.uses_hdr() {
                    Self::Hdr
                } else if !r.ai_denoise.is_default() {
                    Self::AiDenoise
                } else if r.uses_lensfun() {
                    Self::Lensfun
                } else {
                    Self::Recipes
                }
            })
            .max_by_key(|s| s.version())
            .unwrap_or(Self::Recipes)
    }
    /// Every stored recipe names its assets by id, which only the current format
    /// holds.
    pub(super) fn version(self) -> i64 {
        match self {
            Self::Recipes | Self::Lensfun | Self::AiDenoise | Self::Hdr => PROFILE_ASSETS,
        }
    }
}

/// A free name for the backup of the catalog at `path`.
fn backup_path(path: &Path) -> Result<PathBuf> {
    let name = path.file_name().map_or_else(
        || "catalog".to_string(),
        |name| name.to_string_lossy().into_owned(),
    );
    (0..1000)
        .map(|n| {
            let suffix = if n == 0 {
                String::new()
            } else {
                format!("-{n}")
            };
            path.with_file_name(format!("{name}.before-upgrade-backup{suffix}"))
        })
        .find(|candidate| !candidate.exists())
        .ok_or_else(|| anyhow::anyhow!("Too many backups next to the catalog"))
}

/// Brings every stored recipe up to date, inside the upgrade.
fn migrate(w: &mut Write<'_>, u: &mut Upgrade<'_>) -> Result<()> {
    super::edit_rows::migrate_edits(w, u)?;
    super::snapshots::migrate_snapshots(w, u)?;
    super::edit_rows::migrate_histories(w, u)
}

/// The state of one upgrade: what it has stored, what it remembers, and how far it
/// has got.
pub(super) struct Upgrade<'p> {
    /// A profile's or calibration's id by a digest of the text it was stored as,
    /// once its asset is stored.
    remembered: HashMap<[u8; 32], String>,
    /// Those the row being rewritten names, until its assets are stored: a row
    /// left as it was leaves none behind.
    pending: Vec<([u8; 32], String)>,
    /// Assets stored by this upgrade.
    stored: HashSet<String>,
    /// Assets read back through the transaction for the batch being checked.
    verified: Verified,
    unreadable: usize,
    done: usize,
    total: usize,
    progress: &'p mut dyn FnMut(UpgradeProgress),
    #[cfg(test)]
    rewritten: usize,
}

/// A row as an upgrade leaves it.
pub(super) enum Rewrite<T, O> {
    /// As it was.
    Kept,
    /// Rewritten to this, which must read back as the original (`O`) once written
    /// ([`Upgrade::check_recipe`], [`Upgrade::check_history`]).
    To(T, O),
}

impl<'p> Upgrade<'p> {
    fn new(w: &Write<'_>, progress: &'p mut dyn FnMut(UpgradeProgress)) -> Result<Self> {
        let total = super::edit_rows::stored_recipes(w)? + super::snapshots::stored_recipes(w)?;
        let u = Self {
            remembered: HashMap::new(),
            pending: Vec::new(),
            stored: HashSet::new(),
            verified: Verified::new(),
            unreadable: 0,
            done: 0,
            total: total as usize,
            progress,
            #[cfg(test)]
            rewritten: 0,
        };
        (u.progress)(UpgradeProgress {
            done: 0,
            total: u.total,
        });
        Ok(u)
    }

    /// Notes that a batch of `rows` was rewritten and checked: what it read back is
    /// let go.
    pub(super) fn batch_done(&mut self, rows: usize) {
        #[cfg(test)]
        faults::MOST_VERIFIED.with(|m| m.set(m.get().max(self.verified.len())));
        self.verified = Verified::new();
        self.done = (self.done + rows).min(self.total);
        (self.progress)(UpgradeProgress {
            done: self.done,
            total: self.total,
        });
    }

    /// A row this upgrade leaves as it was: counted, and nothing it began for the
    /// row stays.
    fn left<T, O>(&mut self) -> Result<Rewrite<T, O>> {
        self.unreadable += 1;
        self.pending.clear();
        Ok(Rewrite::Kept)
    }

    /// A stored recipe (an edit or a snapshot) as the current format stores it,
    /// with the assets it names stored; the caller writes it and checks it
    /// ([`Self::check_recipe`]). One this release cannot read, or that gives a key
    /// twice (which reading would collapse), is left as it is and counted.
    pub(super) fn recipe(
        &mut self,
        w: &mut Write<'_>,
        text: &str,
    ) -> Result<Rewrite<String, Recipe>> {
        // Read inline: no catalog below this format holds a reference.
        let (Ok(original), Ok(mut fields)) =
            (serde_json::from_str::<Recipe>(text), RawObject::parse(text))
        else {
            return self.left();
        };
        if super::develop_history::has_duplicate_keys(text.as_bytes()) {
            return self.left();
        }
        let Ok(assets) = self.rewrite_fields(&mut fields) else {
            return self.left();
        };
        let rewritten = fields.to_text();
        if rewritten == text {
            self.pending.clear();
            return Ok(Rewrite::Kept);
        }
        self.store(w, &assets)?;
        Ok(Rewrite::To(rewritten, original))
    }

    /// Checks that the recipe stored now as `stored` reads, through the
    /// transaction alone, as `original` did.
    pub(super) fn check_recipe(
        &mut self,
        w: &Write<'_>,
        stored: &str,
        original: &Recipe,
    ) -> Result<()> {
        self.verified.read(&ReadsSource(w), stored);
        let back: Recipe = self
            .verified
            .reading(|| serde_json::from_str(stored))
            .context("An upgraded recipe could not be read back; the catalog was not upgraded")?;
        anyhow::ensure!(
            back == *original,
            "An upgraded recipe reads back differently; the catalog was not upgraded"
        );
        self.rewrote()
    }

    /// A stored History as the current format stores it, rewritten as JSON text (so
    /// what a later release added to it stays, to the byte), with the assets it
    /// names stored; the caller writes it and checks it ([`Self::check_history`]).
    /// One this release cannot read (damaged, or giving a key twice) is left as it
    /// is and counted; one from a newer release is left as it is.
    pub(super) fn history(
        &mut self,
        w: &mut Write<'_>,
        data: &[u8],
    ) -> Result<Rewrite<Vec<u8>, SavedHistory>> {
        use super::develop_history as h;
        let readable = h::text(data).ok().and_then(|text| {
            let original = h::parse(&text).ok()?;
            (!h::has_duplicate_keys(&text)).then_some((text, original))
        });
        let (text, original) = match readable {
            Some((text, Some(original))) => (text, original),
            Some((_, None)) => return Ok(Rewrite::Kept),
            None => return self.left(),
        };
        let rewritten = std::str::from_utf8(&text)
            .map_err(anyhow::Error::from)
            .and_then(|text| self.rewrite_history(text));
        let (rewritten, assets) = match rewritten {
            Ok(Some(rewritten)) => rewritten,
            Ok(None) => {
                self.pending.clear();
                return Ok(Rewrite::Kept);
            }
            Err(_) => return self.left(),
        };
        self.store(w, &assets)?;
        Ok(Rewrite::To(h::compress_text(&rewritten)?, original))
    }

    /// Checks that the History stored now as `data` reads, through the
    /// transaction alone, as `original` did.
    pub(super) fn check_history(
        &mut self,
        w: &Write<'_>,
        data: &[u8],
        original: &SavedHistory,
    ) -> Result<()> {
        use super::develop_history as h;
        let text = h::text(data)?;
        self.verified.read(
            &ReadsSource(w),
            std::str::from_utf8(&text).unwrap_or_default(),
        );
        let back = self
            .verified
            .reading(|| h::parse(&text))
            .context("An upgraded History could not be read back; the catalog was not upgraded")?;
        anyhow::ensure!(
            back.as_ref() == Some(original),
            "An upgraded History reads back differently; the catalog was not upgraded"
        );
        self.rewrote()
    }

    fn rewrote(&mut self) -> Result<()> {
        #[cfg(test)]
        {
            self.rewritten += 1;
            if faults::FAIL_AFTER_ROWS.with(|f| f.get()) == Some(self.rewritten) {
                anyhow::bail!("the upgrade failed (injected)");
            }
        }
        Ok(())
    }

    /// Stores the assets a rewritten row names, once each per upgrade.
    fn store(&mut self, w: &mut Write<'_>, assets: &[Asset]) -> Result<()> {
        let new: Vec<Asset> = assets
            .iter()
            .filter(|a| !self.stored.contains(&a.id))
            .cloned()
            .collect();
        super::recipe_assets::write(w, &new)?;
        #[cfg(test)]
        super::recipe_assets::damage_for_tests(w, &new)?;
        self.stored.extend(new.into_iter().map(|a| a.id));
        // Stored now: later rows may name them by what they were stored as.
        for (digest, id) in std::mem::take(&mut self.pending) {
            self.remember(digest, &id);
        }
        Ok(())
    }

    /// A recipe's fields as the current format stores them: the obsolete settings
    /// (and `preset_settings`) dropped, the profile and the Lensfun calibration
    /// replaced by their ids, every other value as it was. Returns the assets they
    /// name.
    fn rewrite_fields(&mut self, fields: &mut RawObject) -> Result<Vec<Asset>> {
        // Below the one engine these are what the upgrade to it removes; above it,
        // `preset_settings` is the only one a stored recipe still holds.
        fields.retain(|key| !is_obsolete(key));
        let mut assets = Vec::new();
        for key in ["profile", "lens_profile_choice"] {
            let Some(value) = fields.get(key) else {
                continue;
            };
            let (value, asset) = self.rewrite_setting(key, value)?;
            assets.extend(asset);
            if let Some(value) = value {
                fields.set(key, value);
            }
        }
        Ok(assets)
    }

    /// The setting `key` (`profile` or `lens_profile_choice`) as the current format
    /// stores it, when that changes it, and the asset it names when this upgrade
    /// has not stored it yet.
    fn rewrite_setting(
        &mut self,
        key: &str,
        value: &RawValue,
    ) -> Result<(Option<Box<RawValue>>, Option<Asset>)> {
        match key {
            "profile" => self.rewrite_profile(value),
            _ => self.rewrite_choice(value),
        }
    }

    /// An inline profile replaced by its id.
    fn rewrite_profile(
        &mut self,
        profile: &RawValue,
    ) -> Result<(Option<Box<RawValue>>, Option<Asset>)> {
        if !profile.get().starts_with('{') {
            return Ok((None, None));
        }
        let digest = digest(profile.get());
        if let Some(id) = self.known(&digest) {
            return Ok((Some(raw_of(&id)), None));
        }
        let p = crate::camera_profiles::asset::normalized(serde_json::from_str::<CameraProfile>(
            profile.get(),
        )?);
        let asset = Asset::profile(&Arc::new(p));
        self.pending.push((digest, asset.id.clone()));
        Ok((Some(raw_of(&asset.id)), Some(asset)))
    }

    /// A lens profile choice with its inline Lensfun calibration replaced by its id.
    fn rewrite_choice(
        &mut self,
        choice: &RawValue,
    ) -> Result<(Option<Box<RawValue>>, Option<Asset>)> {
        let object = |raw: Option<&RawValue>| {
            raw.filter(|r| r.get().starts_with('{'))
                .map(|r| RawObject::parse(r.get()))
                .transpose()
        };
        let Some(mut choice) = object(Some(choice))? else {
            return Ok((None, None));
        };
        let Some(mut id) = object(choice.get("id"))? else {
            return Ok((None, None));
        };
        let Some(mut lensfun) = object(id.get("lensfun"))? else {
            return Ok((None, None));
        };
        let Some(calibration) = lensfun
            .get("calibration")
            .filter(|c| c.get().starts_with('{'))
        else {
            return Ok((None, None));
        };
        let digest = digest(calibration.get());
        let (reference, asset) = match self.known(&digest) {
            Some(id) => (id, None),
            None => {
                let c: Calibration = serde_json::from_str(calibration.get())?;
                let asset = Asset::calibration(&Arc::new(c));
                self.pending.push((digest, asset.id.clone()));
                (asset.id.clone(), Some(asset))
            }
        };
        lensfun.set("calibration", raw_of(&reference));
        id.set("lensfun", lensfun.to_raw());
        choice.set("id", id.to_raw());
        Ok((Some(choice.to_raw()), asset))
    }

    /// The id of what was stored as the text `digest` names: stored by an earlier
    /// row, or named earlier in this one.
    fn known(&self, digest: &[u8; 32]) -> Option<String> {
        self.remembered.get(digest).cloned().or_else(|| {
            self.pending
                .iter()
                .find(|(d, _)| d == digest)
                .map(|(_, id)| id.clone())
        })
    }

    fn remember(&mut self, digest: [u8; 32], id: &str) {
        if self.remembered.len() >= REMEMBERED {
            self.remembered.clear();
        }
        self.remembered.insert(digest, id.to_string());
    }

    /// A stored History's text as the current format stores it, and the assets it
    /// names; `None` when nothing in it changes. Each state's own `fields` are
    /// rewritten in place. A pooled value is rewritten for the field that refers to
    /// it, never in place: the History's encoder pools values by content alone, so
    /// one entry may serve the profile and a setting this release doesn't know that
    /// holds the same object, and that setting must keep it. Entries nothing refers
    /// to any more are dropped and the rest renumbered; every other value stays as
    /// it was written.
    fn rewrite_history(&mut self, text: &str) -> Result<Option<(String, Vec<Asset>)>> {
        let mut top = RawObject::parse(text)?;
        let pool: Vec<Box<RawValue>> = match top.get("pool") {
            Some(pool) => serde_json::from_str(pool.get())?,
            None => Vec::new(),
        };
        let mut origin = top
            .get("origin")
            .map(|o| RawObject::parse(o.get()))
            .transpose()?;
        let mut steps: Option<Vec<RawObject>> = top
            .get("steps")
            .map(|s| serde_json::from_str(s.get()))
            .transpose()?;
        let mut states: Vec<RawObject> = Vec::new();
        if let Some(origin) = &origin {
            states.push(origin.clone());
        }
        for step in steps.iter().flatten() {
            states.push(match step.get("state") {
                Some(state) => RawObject::parse(state.get())?,
                None => RawObject::default(),
            });
        }
        let mut added: Vec<Box<RawValue>> = Vec::new();
        // (original index, key) → index of its rewritten entry.
        let mut rewritten: HashMap<(usize, &'static str), usize> = HashMap::new();
        let mut assets = Vec::new();
        let mut changed = false;
        // Each state's pooled indexes, as numbers, keyed as stored.
        let mut pooled_of: Vec<Option<Vec<(String, usize)>>> = Vec::new();
        for state in &mut states {
            if let Some(fields) = state.get("fields") {
                let mut fields = RawObject::parse(fields.get())?;
                let before = fields.to_text();
                assets.extend(self.rewrite_fields(&mut fields)?);
                let after = fields.to_text();
                if after != before {
                    changed = true;
                    state.set("fields", raw(after));
                }
            }
            let Some(pooled) = state.get("pooled") else {
                pooled_of.push(None);
                continue;
            };
            let mut pooled: Vec<(String, usize)> = RawObject::parse(pooled.get())?
                .iter()
                .map(|(k, v)| Ok((k.to_string(), serde_json::from_str::<usize>(v.get())?)))
                .collect::<Result<_>>()?;
            let before = pooled.len();
            pooled.retain(|(key, _)| !is_obsolete(key));
            changed |= pooled.len() != before;
            for (key, index) in &mut pooled {
                let key: &'static str = match key.as_str() {
                    "profile" => "profile",
                    "lens_profile_choice" => "lens_profile_choice",
                    _ => continue,
                };
                let at = match rewritten.get(&(*index, key)) {
                    Some(at) => *at,
                    None => {
                        let value = pool.get(*index).context("Missing pooled History setting")?;
                        let (value, asset) = self.rewrite_setting(key, value)?;
                        assets.extend(asset);
                        let Some(value) = value else {
                            continue;
                        };
                        added.push(value);
                        let at = pool.len() + added.len() - 1;
                        rewritten.insert((*index, key), at);
                        at
                    }
                };
                *index = at;
                changed = true;
            }
            pooled_of.push(Some(pooled));
        }
        if !changed {
            return Ok(None);
        }
        // The pool without entries nothing refers to, the references renumbered.
        let mut all = pool;
        all.extend(added);
        let mut used = vec![false; all.len()];
        for (_, index) in pooled_of.iter().flatten().flatten() {
            if let Some(u) = used.get_mut(*index) {
                *u = true;
            }
        }
        let mut renumbered = vec![0; all.len()];
        let mut kept = Vec::new();
        for (index, value) in all.into_iter().enumerate() {
            if used[index] {
                renumbered[index] = kept.len();
                kept.push(value);
            }
        }
        for (state, pooled) in states.iter_mut().zip(&pooled_of) {
            if let Some(pooled) = pooled {
                let mut object = RawObject::default();
                for (key, index) in pooled {
                    object.set(
                        key,
                        raw_of(&renumbered.get(*index).copied().unwrap_or(*index)),
                    );
                }
                state.set("pooled", object.to_raw());
            }
        }
        let mut states = states.into_iter();
        if let Some(origin) = &mut origin {
            *origin = states.next().expect("the origin's state");
            top.set("origin", origin.to_raw());
        }
        if let Some(steps) = &mut steps {
            for step in steps.iter_mut() {
                let state = states.next().expect("a step's state");
                if step.get("state").is_some() {
                    step.set("state", state.to_raw());
                }
            }
            top.set("steps", raw_of(&steps));
        }
        if top.get("pool").is_some() {
            top.set("pool", raw_of(&kept));
        }
        Ok(Some((top.to_text(), assets)))
    }
}

/// A digest of a JSON value's text.
fn digest(text: &str) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    Sha256::digest(text.as_bytes()).into()
}

/// Failures tests inject into the upgrade, on this thread.
#[cfg(test)]
pub(super) mod faults {
    use std::cell::Cell;
    thread_local! {
        /// The upgrade fails once it has rewritten this many rows.
        pub(in crate::catalog) static FAIL_AFTER_ROWS: Cell<Option<usize>> = const { Cell::new(None) };
        /// Every asset the upgrade stores is damaged right after, as a bad disk would.
        pub(in crate::catalog) static DAMAGE_ASSETS: Cell<bool> = const { Cell::new(false) };
        /// Every asset the upgrade stores is deleted right after.
        pub(in crate::catalog) static DROP_ASSETS: Cell<bool> = const { Cell::new(false) };
        /// The most assets the upgrade's check held at once, for the memory tests.
        pub(in crate::catalog) static MOST_VERIFIED: Cell<usize> = const { Cell::new(0) };
    }
}
