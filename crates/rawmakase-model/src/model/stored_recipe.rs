//! A recipe as a catalog stores it: its camera profile and its Lensfun lens's
//! calibration named by content id (`"profile": "sha256:…"`, and `"calibration"`
//! at `/lens_profile_choice/id/lensfun/calibration`), each value stored once in
//! the catalog ([`crate::storage::recipe_assets`]).
//!
//! Only a catalog writes this form; its edits, every History state and pooled
//! value, its snapshots and the legacy sidecars it takes in all go through
//! [`Storer`]. Everything else (native presets, legacy sidecars, the clipboard, the
//! session) keeps writing the profile inline through the derived `Serialize`, so no
//! file outside a catalog ever holds a reference. Reading accepts both forms
//! everywhere ([`crate::model::recipe::Recipe`]'s field deserializers).
use crate::camera_profiles::CameraProfile;
use crate::lens::choice::LensProfileRef;
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
    assets: BTreeMap<String, Asset>,
}
impl Storer {
    pub fn new() -> Self {
        Self::default()
    }
    /// `r` in the stored form.
    pub fn to_stored(&mut self, r: &Recipe) -> serde_json::Value {
        let mut bare = r.clone();
        let profile = bare.profile.take();
        let calibration = match &mut bare.lens_profile_choice.id {
            Some(LensProfileRef::Lensfun(choice)) => choice.calibration.take(),
            _ => None,
        };
        let mut stored = serde_json::to_value(&bare).expect("a recipe serializes");
        if let Some(p) = profile {
            stored["profile"] = self.profile_id(&p).into();
        }
        if let Some(c) = calibration {
            let id = self.calibration_id(&c);
            stored
                .pointer_mut("/lens_profile_choice/id/lensfun")
                .and_then(serde_json::Value::as_object_mut)
                .expect("a Lensfun choice is stored under lensfun")
                .insert("calibration".into(), id.into());
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
    /// Every asset the recipes stored so far name, once each, by id.
    pub fn assets(&self) -> impl Iterator<Item = &Asset> {
        self.assets.values()
    }
    pub fn into_assets(self) -> Vec<Asset> {
        self.assets.into_values().collect()
    }
}

/// `r` in the stored form, with the assets it names.
pub fn to_stored(r: &Recipe) -> (serde_json::Value, Vec<Asset>) {
    let mut storer = Storer::new();
    let stored = storer.to_stored(r);
    (stored, storer.into_assets())
}

/// `r`'s stored form as text: what a catalog keeps, and what identifies the edit
/// (a preview's tag) whichever form it was read from.
pub fn to_text(r: &Recipe) -> String {
    to_stored(r).0.to_string()
}

/// The recipe stored as `text`, its references resolved through `source`.
pub fn from_text(source: &dyn AssetSource, text: &str) -> serde_json::Result<Recipe> {
    recipe_assets::reading(source, text, || serde_json::from_str(text))
}

#[cfg(test)]
mod tests;
