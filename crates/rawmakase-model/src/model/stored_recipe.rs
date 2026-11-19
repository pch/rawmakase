//! A recipe as a catalog stores it: its camera profile and its Lensfun lens's
//! calibration named by content id (`"profile": "sha256:…"`, and `"calibration"`
//! at `/lens_profile_choice/id/lensfun/calibration`), each value stored once in
//! the catalog ([`crate::storage::recipe_assets`]). A profile that is a look with
//! known Lightroom settings also gets `"look_source": "sha256:…"`, so the catalog
//! keeps those settings for writing the look out
//! ([`crate::camera_profiles::look_source`]); reading drops the key, and the next
//! save writes it again from the profile.
//!
//! Only a catalog writes this form; its edits, every History state and pooled
//! value, its snapshots and the legacy sidecars it takes in all go through
//! [`Storer`]. Everything else (native presets, legacy sidecars, the clipboard, the
//! session) keeps writing the profile inline through the derived `Serialize`, so no
//! file outside a catalog ever holds a reference. Reading accepts both forms
//! everywhere ([`crate::model::recipe::Recipe`]'s field deserializers).
use crate::camera_profiles::{CameraProfile, look_source::LookSource};
use crate::lens::choice::LensProfileRef;
use crate::model::raw_json::{RawObject, raw_of};
use crate::model::recipe::Recipe;
use crate::optics::lensfun::Calibration;
use crate::storage::recipe_assets::{self, Asset, AssetSource};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

/// Turns recipes into their stored form and collects the assets they name. One
/// storer serves one write: a profile shared by many History states is hashed
/// once (values are memoized by their `Arc`'s address; the storer holds the `Arc`,
/// so an address is never reused while it runs).
#[derive(Default)]
pub struct Storer {
    profiles: HashMap<usize, (Arc<CameraProfile>, String)>,
    calibrations: HashMap<usize, (Arc<Calibration>, String)>,
    /// The id of each profile's look settings, if it has known ones.
    look_sources: HashMap<usize, Option<String>>,
    /// Whether to name looks' settings: not for an edit's identity, which leaves them out.
    without_look_sources: bool,
    assets: BTreeMap<String, Asset>,
}
impl Storer {
    pub fn new() -> Self {
        Self::default()
    }
    /// `r` in the stored form, as an object whose values keep the text the
    /// recipe serializes them as.
    pub fn to_stored(&mut self, r: &Recipe) -> RawObject {
        let mut bare = r.clone();
        let profile = bare.profile.take();
        // Written again below from the profile; a stale one is never kept.
        bare.unknown.remove(LOOK_SOURCE);
        let calibration = match &mut bare.lens_profile_choice.id {
            Some(LensProfileRef::Lensfun(choice)) => choice.calibration.take(),
            _ => None,
        };
        let text = serde_json::to_string(&bare).expect("a recipe serializes");
        let mut stored = RawObject::parse(&text).expect("a recipe is an object");
        if let Some(p) = profile {
            stored.set("profile", raw_of(&self.profile_id(&p)));
            if let Some(id) = self.look_source_id(&p) {
                stored.set(LOOK_SOURCE, raw_of(&id));
            }
        }
        if let Some(c) = calibration {
            let id = self.calibration_id(&c);
            let at = |o: &RawObject, key: &str| {
                RawObject::parse(o.get(key).expect("a Lensfun choice").get())
                    .expect("a Lensfun choice is an object")
            };
            let mut choice = at(&stored, "lens_profile_choice");
            let mut lens_id = at(&choice, "id");
            let mut lensfun = at(&lens_id, "lensfun");
            lensfun.set("calibration", raw_of(&id));
            lens_id.set("lensfun", lensfun.to_raw());
            choice.set("id", lens_id.to_raw());
            stored.set("lens_profile_choice", choice.to_raw());
        }
        stored
    }
    /// The id `p` is stored under, noting it as an asset of this write.
    pub fn profile_id(&mut self, p: &Arc<CameraProfile>) -> String {
        let (_, id) = self
            .profiles
            .entry(Arc::as_ptr(p) as usize)
            .or_insert_with(|| {
                let asset = Asset::profile(p);
                let id = asset.id.clone();
                self.assets.entry(id.clone()).or_insert(asset);
                (p.clone(), id)
            });
        id.clone()
    }
    /// The id `c` is stored under, noting it as an asset of this write.
    pub fn calibration_id(&mut self, c: &Arc<Calibration>) -> String {
        let (_, id) = self
            .calibrations
            .entry(Arc::as_ptr(c) as usize)
            .or_insert_with(|| {
                let asset = Asset::calibration(c);
                let id = asset.id.clone();
                self.assets.entry(id.clone()).or_insert(asset);
                (c.clone(), id)
            });
        id.clone()
    }
    /// The id `p`'s look settings are stored under, noting them as an asset of this
    /// write; `None` when `p` has no known ones.
    fn look_source_id(&mut self, p: &Arc<CameraProfile>) -> Option<String> {
        if self.without_look_sources {
            return None;
        }
        self.look_sources
            .entry(Arc::as_ptr(p) as usize)
            .or_insert_with(|| {
                let source: Arc<LookSource> = crate::camera_profiles::look_source::find(p)?;
                let asset = Asset::look_source(&source);
                let id = asset.id.clone();
                self.assets.entry(id.clone()).or_insert(asset);
                Some(id)
            })
            .clone()
    }
    /// Every asset the recipes stored so far name, once each, by id.
    pub fn assets(&self) -> impl Iterator<Item = &Asset> {
        self.assets.values()
    }
    pub fn into_assets(self) -> Vec<Asset> {
        self.assets.into_values().collect()
    }
}

/// `r` in the stored form, with the assets it names.
pub fn to_stored(r: &Recipe) -> (RawObject, Vec<Asset>) {
    let mut storer = Storer::new();
    let stored = storer.to_stored(r);
    (stored, storer.into_assets())
}

/// `r`'s stored form as text: what identifies the edit (a preview's tag) whichever
/// form it was read from. Without its look's settings, which don't change how it
/// renders and which this process may or may not know.
pub fn to_text(r: &Recipe) -> String {
    let mut storer = Storer {
        without_look_sources: true,
        ..Storer::default()
    };
    storer.to_stored(r).to_text()
}

/// The stored recipe's key naming its look's Lightroom settings.
pub const LOOK_SOURCE: &str = "look_source";

/// The recipe stored as `text`, its references resolved through `source`.
pub fn from_text(source: &dyn AssetSource, text: &str) -> serde_json::Result<Recipe> {
    recipe_assets::reading(source, text, || serde_json::from_str(text))
}

#[cfg(test)]
mod tests;
