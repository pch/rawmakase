//! A photo's Develop History, kept with its edit so it survives switching photos and
//! restarting. Stored zlib-compressed in `develop_history`; each step holds the full
//! state it leaves, with large settings (masks, spots, curves) stored once per
//! History and referred to from every step that has them. Each state is a recipe in
//! the catalog's stored form ([`crate::model::stored_recipe`]): its camera profile
//! and Lensfun calibration are named by id and stored once in the catalog.
use super::db::{Reads, sql};
use super::{Catalog, PhotoId};
use crate::model::raw_json::RawObject;
use crate::model::recipe::Recipe;
use crate::model::stored_recipe::Storer;
use crate::storage::recipe_assets::{self, AssetSource};
use anyhow::{Context, Result, ensure};
use flate2::{Compression, read::ZlibDecoder, write::ZlibEncoder};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
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

/// One recipe as [`encode`] writes it: its settings as the text the recipe's
/// stored form gives them, the large ones as indexes into `Encoded::pool`.
#[derive(Serialize)]
struct EncodedState {
    fields: RawObject,
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pooled: std::collections::BTreeMap<String, usize>,
}
#[derive(Serialize)]
struct EncodedStep {
    name: String,
    value: String,
    state: EncodedState,
}
#[derive(Serialize)]
struct Encoded {
    version: u32,
    pool: Vec<Box<RawValue>>,
    origin: EncodedState,
    steps: Vec<EncodedStep>,
    applied: usize,
}

/// Interns large setting values so each is stored once.
#[derive(Default)]
struct Pool {
    values: Vec<Box<RawValue>>,
    index: std::collections::HashMap<String, usize>,
}
impl Pool {
    fn state(&mut self, storer: &mut Storer, r: &Recipe) -> EncodedState {
        let mut state = EncodedState {
            fields: RawObject::default(),
            pooled: Default::default(),
        };
        for (key, value) in storer.to_stored(r).iter() {
            let text = value.get();
            if text.len() <= POOLED_BYTES {
                state.fields.set(key, value.to_owned());
                continue;
            }
            let n = *self.index.entry(text.to_string()).or_insert_with(|| {
                self.values.push(value.to_owned());
                self.values.len() - 1
            });
            state.pooled.insert(key.to_string(), n);
        }
        state
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

/// `history` as stored, its profiles and calibrations named by `storer`, which
/// collects them for the write.
pub(super) fn encode(history: &SavedHistory, storer: &mut Storer) -> Result<Vec<u8>> {
    ensure!(
        history.applied <= history.steps.len(),
        "Invalid History position"
    );
    let mut pool = Pool::default();
    let origin = pool.state(storer, &history.origin);
    let steps = history
        .steps
        .iter()
        .map(|s| EncodedStep {
            name: s.name.clone(),
            value: s.value.clone(),
            state: pool.state(storer, &s.recipe),
        })
        .collect();
    let stored = Encoded {
        version: VERSION,
        pool: pool.values,
        origin,
        steps,
        applied: history.applied,
    };
    compress(&stored)
}

pub(super) fn compress(stored: &impl Serialize) -> Result<Vec<u8>> {
    let mut z = ZlibEncoder::new(Vec::new(), Compression::default());
    serde_json::to_writer(&mut z, stored)?;
    Ok(z.finish()?)
}

/// `text`, a History's JSON, zlib-compressed as it is stored.
pub(super) fn compress_text(text: &str) -> Result<Vec<u8>> {
    use std::io::Write;
    let mut z = ZlibEncoder::new(Vec::new(), Compression::default());
    z.write_all(text.as_bytes())?;
    Ok(z.finish()?)
}

/// The stored History in `data` as the JSON text it was written as, whatever
/// release wrote it.
pub(super) fn text(data: &[u8]) -> Result<Vec<u8>> {
    let mut text = Vec::new();
    ZlibDecoder::new(data)
        .take(MAX_BYTES + 1)
        .read_to_end(&mut text)?;
    ensure!(text.len() as u64 <= MAX_BYTES, "Develop History too large");
    Ok(text)
}

/// The History stored as `text`; `None` for a format from a newer release. A
/// reference to a stored profile or calibration resolves through the
/// [`recipe_assets::reading`] around the call.
pub(super) fn parse(text: &[u8]) -> Result<Option<SavedHistory>> {
    let value: Value = serde_json::from_slice(text)?;
    let version = value
        .get("version")
        .and_then(Value::as_u64)
        .context("Develop History without a version")?;
    if version > u64::from(VERSION) {
        return Ok(None);
    }
    let stored: Stored = serde_json::from_value(value)?;
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

/// Whether any JSON object in `text` gives a key twice.
pub(super) fn has_duplicate_keys(text: &[u8]) -> bool {
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

/// The History in `data`, its references resolved through `source`; `None` for a
/// format from a newer release.
pub(super) fn decode(data: &[u8], source: &dyn AssetSource) -> Result<Option<SavedHistory>> {
    let text = text(data)?;
    let utf8 = std::str::from_utf8(&text).context("Develop History is not text")?;
    recipe_assets::reading(source, utf8, || parse(&text))
}

impl Catalog {
    /// The photo's saved Develop History. `None` without one, or when it cannot be
    /// read (from a newer release, or damaged): the edit itself stays usable.
    pub fn load_history(&self, id: PhotoId) -> Result<Option<SavedHistory>> {
        let data: Option<Vec<u8>> = self.db.read_optional(
            sql!("SELECT data FROM develop_history WHERE photo=?"),
            &[&id],
        )?;
        let source = super::recipe_assets::ReadsSource(&self.db);
        Ok(data.and_then(|d| decode(&d, &source).ok().flatten()))
    }
    /// Whether the photo has a stored History, readable here or not.
    pub fn has_history(&self, id: PhotoId) -> Result<bool> {
        Ok(self
            .db
            .read_optional::<i64>(sql!("SELECT 1 FROM develop_history WHERE photo=?"), &[&id])?
            .is_some())
    }
}
