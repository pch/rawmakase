//! A photo's Develop History, kept with its edit so it survives switching photos and
//! restarting. Stored zlib-compressed in `develop_history`; each step holds the full
//! state it leaves, with large settings (camera profile, masks, spots, curves) stored
//! once per History and referred to from every step that has them.
use super::db::{Reads, sql};
use super::{Catalog, PhotoId};
use crate::model::recipe::Recipe;
use anyhow::{Context, Result, ensure};
use flate2::{Compression, read::ZlibDecoder, write::ZlibEncoder};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::io::Read;

/// Format of the stored History; a newer one is ignored, not misread.
const VERSION: u32 = 1;
/// Largest stored History accepted, decompressed.
const MAX_BYTES: u64 = 256 << 20;
/// Settings longer than this (as JSON) are stored once and referred to.
const POOLED_BYTES: usize = 256;

pub use crate::model::saved_history::{SavedHistory, SavedStep};

/// What saving an edit does to its stored History.
#[derive(Clone, Copy, Debug)]
pub enum HistoryUpdate<'a> {
    /// Leave it: the edit changed outside Develop's History (Undo after moving to
    /// another photo, a sidecar import), which the History notices when the photo
    /// next opens.
    Keep,
    Replace(&'a SavedHistory),
}

/// One recipe with its large settings replaced by indexes into `Stored::pool`.
#[derive(Serialize, Deserialize)]
struct StoredState {
    fields: Map<String, Value>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pooled: std::collections::BTreeMap<String, usize>,
}
#[derive(Serialize, Deserialize)]
struct StoredStep {
    name: String,
    value: String,
    state: StoredState,
}
#[derive(Serialize, Deserialize)]
struct Stored {
    version: u32,
    pool: Vec<Value>,
    origin: StoredState,
    steps: Vec<StoredStep>,
    applied: usize,
}

/// Interns large setting values so each is stored once.
#[derive(Default)]
struct Pool {
    values: Vec<Value>,
    index: std::collections::HashMap<String, usize>,
}
impl Pool {
    fn state(&mut self, r: &Recipe) -> Result<StoredState> {
        let Value::Object(all) = serde_json::to_value(r)? else {
            anyhow::bail!("Recipe is not an object");
        };
        let mut state = StoredState {
            fields: Map::new(),
            pooled: Default::default(),
        };
        for (key, value) in all {
            let text = value.to_string();
            if text.len() <= POOLED_BYTES {
                state.fields.insert(key, value);
                continue;
            }
            let n = *self.index.entry(text).or_insert_with(|| {
                self.values.push(value);
                self.values.len() - 1
            });
            state.pooled.insert(key, n);
        }
        Ok(state)
    }
}
impl StoredState {
    fn recipe(self, pool: &[Value]) -> Result<Recipe> {
        let mut fields = self.fields;
        for (key, n) in self.pooled {
            let value = pool.get(n).context("Missing pooled History setting")?;
            fields.insert(key, value.clone());
        }
        let recipe: Recipe = serde_json::from_value(Value::Object(fields))?;
        recipe.validate()?;
        Ok(recipe)
    }
}

impl<'a> HistoryUpdate<'a> {
    /// How saving the edit treats the stored History, given `history`. Without steps
    /// nothing was recorded here, so a stored one (even one this release cannot read,
    /// from a newer release) is kept.
    pub fn of(history: &'a SavedHistory) -> Self {
        if history.steps.is_empty() {
            Self::Keep
        } else {
            Self::Replace(history)
        }
    }
}

/// `history` as stored.
pub fn encode(history: &SavedHistory) -> Result<Vec<u8>> {
    ensure!(
        history.applied <= history.steps.len(),
        "Invalid History position"
    );
    let mut pool = Pool::default();
    let origin = pool.state(&history.origin)?;
    let steps = history
        .steps
        .iter()
        .map(|s| {
            Ok(StoredStep {
                name: s.name.clone(),
                value: s.value.clone(),
                state: pool.state(&s.recipe)?,
            })
        })
        .collect::<Result<_>>()?;
    let stored = Stored {
        version: VERSION,
        pool: pool.values,
        origin,
        steps,
        applied: history.applied,
    };
    compress(&stored)
}

fn compress(stored: &impl Serialize) -> Result<Vec<u8>> {
    let mut z = ZlibEncoder::new(Vec::new(), Compression::default());
    serde_json::to_writer(&mut z, stored)?;
    Ok(z.finish()?)
}

/// The stored History in `data` as JSON; `None` for a format from a newer release.
fn stored_json(data: &[u8]) -> Result<Option<(Vec<u8>, Value)>> {
    let mut text = Vec::new();
    ZlibDecoder::new(data)
        .take(MAX_BYTES + 1)
        .read_to_end(&mut text)?;
    ensure!(text.len() as u64 <= MAX_BYTES, "Develop History too large");
    let value: Value = serde_json::from_slice(&text)?;
    let version = value
        .get("version")
        .and_then(Value::as_u64)
        .context("Develop History without a version")?;
    if version > u64::from(VERSION) {
        return Ok(None);
    }
    Ok(Some((text, value)))
}

/// The stored History in `data`; `None` for a format from a newer release.
fn stored(data: &[u8]) -> Result<Option<Stored>> {
    stored_json(data)?
        .map(|(_, value)| Ok(serde_json::from_value(value)?))
        .transpose()
}

/// `data` with every step's recipe rid of the settings that chose an engine or
/// operator (`saved_format::OBSOLETE_SETTINGS`), for the catalog's upgrade: only
/// the recipes' objects change, every other JSON value (including fields of a
/// later History format) stays as stored. `None` when there were none, or the
/// History is not one this release reads (from a newer release, damaged, or with a
/// key given twice, which reading would silently collapse): it is then left as it is.
pub(super) fn without_obsolete_settings(data: &[u8]) -> Option<Vec<u8>> {
    use crate::model::saved_format::{drop_obsolete_settings, is_obsolete};
    decode(data).ok().flatten()?;
    let (text, mut value) = stored_json(data).ok().flatten()?;
    if has_duplicate_keys(&text) {
        return None;
    }
    // Each state: `fields`, and `pooled` naming settings stored once in `pool`.
    let strip = |state: &mut Value| {
        let mut changed = false;
        if let Some(fields) = state.get_mut("fields").and_then(Value::as_object_mut) {
            changed |= drop_obsolete_settings(fields);
        }
        if let Some(pooled) = state.get_mut("pooled").and_then(Value::as_object_mut) {
            let before = pooled.len();
            pooled.retain(|key, _| !is_obsolete(key));
            changed |= pooled.len() != before;
        }
        changed
    };
    let mut changed = value.get_mut("origin").is_some_and(strip);
    if let Some(steps) = value.get_mut("steps").and_then(Value::as_array_mut) {
        for state in steps.iter_mut().filter_map(|step| step.get_mut("state")) {
            changed |= strip(state);
        }
    }
    changed.then(|| compress(&value).ok()).flatten()
}

/// Whether any JSON object in `text` gives a key twice.
fn has_duplicate_keys(text: &[u8]) -> bool {
    struct Unique;
    impl<'de> serde::de::DeserializeSeed<'de> for Unique {
        type Value = ();
        fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
            d.deserialize_any(self)
        }
    }
    impl<'de> serde::de::Visitor<'de> for Unique {
        type Value = ();
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("JSON")
        }
        fn visit_bool<E>(self, _: bool) -> Result<(), E> {
            Ok(())
        }
        fn visit_i64<E>(self, _: i64) -> Result<(), E> {
            Ok(())
        }
        fn visit_u64<E>(self, _: u64) -> Result<(), E> {
            Ok(())
        }
        fn visit_f64<E>(self, _: f64) -> Result<(), E> {
            Ok(())
        }
        fn visit_str<E>(self, _: &str) -> Result<(), E> {
            Ok(())
        }
        fn visit_unit<E>(self) -> Result<(), E> {
            Ok(())
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
            while seq.next_element_seed(Unique)?.is_some() {}
            Ok(())
        }
        fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
            let mut keys = std::collections::HashSet::new();
            while let Some(key) = map.next_key::<String>()? {
                if !keys.insert(key) {
                    return Err(serde::de::Error::custom("duplicate key"));
                }
                map.next_value_seed(Unique)?;
            }
            Ok(())
        }
    }
    let mut d = serde_json::Deserializer::from_slice(text);
    serde::de::DeserializeSeed::deserialize(Unique, &mut d).is_err()
}

/// The History in `data`; `None` for a format from a newer release.
pub fn decode(data: &[u8]) -> Result<Option<SavedHistory>> {
    let Some(stored) = stored(data)? else {
        return Ok(None);
    };
    ensure!(
        stored.applied <= stored.steps.len(),
        "Invalid History position"
    );
    let pool = stored.pool;
    Ok(Some(SavedHistory {
        origin: stored.origin.recipe(&pool)?,
        steps: stored
            .steps
            .into_iter()
            .map(|s| {
                Ok(SavedStep {
                    name: s.name,
                    value: s.value,
                    recipe: s.state.recipe(&pool)?,
                })
            })
            .collect::<Result<_>>()?,
        applied: stored.applied,
    }))
}

impl Catalog {
    /// The photo's saved Develop History. `None` without one, or when it cannot be
    /// read (from a newer release, or damaged): the edit itself stays usable.
    pub fn load_history(&self, id: PhotoId) -> Result<Option<SavedHistory>> {
        let data: Option<Vec<u8>> = self.db.read_optional(
            sql!("SELECT data FROM develop_history WHERE photo=?"),
            &[&id],
        )?;
        Ok(data.and_then(|d| decode(&d).ok().flatten()))
    }
    /// Whether the photo has a stored History, readable here or not.
    pub fn has_history(&self, id: PhotoId) -> Result<bool> {
        Ok(self
            .db
            .read_optional::<i64>(sql!("SELECT 1 FROM develop_history WHERE photo=?"), &[&id])?
            .is_some())
    }
}
