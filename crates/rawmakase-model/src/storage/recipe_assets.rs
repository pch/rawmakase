//! Camera profiles and Lensfun calibrations a catalog stores once and recipes name
//! by content.
//!
//! In memory a recipe always holds its profile and calibration. Only a catalog's
//! stored form names them by id instead (`"profile": "sha256:…"`, see
//! [`crate::model::stored_recipe`]), and a catalog keeps each one once, as it keeps
//! mask rasters. An edit still renders bit for bit after its profile file is
//! removed or updated, or on another computer: the catalog holds the value.
//!
//! Reading a reference needs the catalog it was stored in. A caller parses stored
//! text inside [`reading`] with the [`AssetSource`] it read the text from; nothing
//! is registered globally, so two open catalogs, a closed one an export job still
//! reads from and a command-line run all resolve the same way, and a reference read
//! with no source fails with a clear error rather than rendering something else.
//!
//! A process-wide cache shares one value between every recipe that names it (a
//! weak map by id, and the most recently used ones held strongly). It is only an
//! accelerator: it holds values whose bytes a source verified against their id, or
//! values a write is storing, which name themselves.
use crate::camera_profiles::{CameraProfile, asset};
use crate::optics::lensfun::Calibration;
use std::collections::{HashMap, VecDeque};
use std::io::Read;
use std::sync::{Arc, Mutex, OnceLock, Weak};

/// Most bytes of an asset's canonical JSON. A camera profile with a full look table
/// is a few megabytes at most, so a larger one is damaged and is not inflated.
pub const MAX_ASSET_BYTES: usize = 64 << 20;
/// Most assets the cache holds strongly, besides the ones recipes in memory hold.
const RECENT: usize = 32;

/// What an asset is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AssetKind {
    Profile,
    Calibration,
}
impl AssetKind {
    pub const ALL: [Self; 2] = [Self::Profile, Self::Calibration];
    /// The name stored with it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Profile => "camera_profile",
            Self::Calibration => "lensfun_calibration",
        }
    }
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.name() == name)
    }
    /// What is hashed ahead of the canonical bytes. The version changes with the
    /// canonical form, so ids of different forms never meet.
    fn domain(self) -> &'static [u8] {
        match self {
            Self::Profile => b"rawmakase-camera-profile-v1\n",
            Self::Calibration => b"rawmakase-lensfun-calibration-v1\n",
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Profile => "Camera profile",
            Self::Calibration => "Lensfun calibration",
        }
    }
}

/// The id of an asset whose canonical bytes are `canonical`: `sha256:` and 64
/// lowercase hex digits.
pub fn asset_id(kind: AssetKind, canonical: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut sha = Sha256::new();
    sha.update(kind.domain());
    sha.update(canonical);
    let mut id = String::with_capacity(7 + 64);
    id.push_str("sha256:");
    for byte in sha.finalize() {
        id.push_str(&format!("{byte:02x}"));
    }
    id
}

/// Whether `text` has the form of an asset id. Mask rasters' ids have the same form.
pub fn is_asset_id(text: &str) -> bool {
    text.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    })
}

/// Why an asset could not be provided.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AssetError {
    /// The catalog has no asset with this id.
    Missing(AssetKind, String),
    /// The catalog's copy is damaged or is not what its id names.
    Damaged(AssetKind, String, String),
    /// The catalog could not be read.
    Unreadable(String, String),
    /// A stored reference was read with no catalog to resolve it.
    NoSource,
}
impl std::fmt::Display for AssetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing(kind, id) => {
                write!(f, "{} {id} is missing from the catalog", kind.label())
            }
            Self::Damaged(kind, id, why) => {
                write!(f, "{} {id} in the catalog is damaged: {why}", kind.label())
            }
            Self::Unreadable(id, why) => {
                write!(f, "The catalog's copy of {id} cannot be read: {why}")
            }
            Self::NoSource => write!(f, "a catalog recipe read outside its catalog"),
        }
    }
}
impl std::error::Error for AssetError {}

/// A stored asset's value, shared by every recipe in memory that names it.
#[derive(Clone, Debug)]
pub enum AssetValue {
    Profile(Arc<CameraProfile>),
    Calibration(Arc<Calibration>),
}
impl AssetValue {
    pub fn kind(&self) -> AssetKind {
        match self {
            Self::Profile(_) => AssetKind::Profile,
            Self::Calibration(_) => AssetKind::Calibration,
        }
    }
    fn downgrade(&self) -> WeakValue {
        match self {
            Self::Profile(p) => WeakValue::Profile(Arc::downgrade(p)),
            Self::Calibration(c) => WeakValue::Calibration(Arc::downgrade(c)),
        }
    }
}

/// One asset as a write stores it: its id, its value and its canonical bytes.
#[derive(Clone, Debug)]
pub struct Asset {
    pub id: String,
    pub value: AssetValue,
    canonical: Arc<Vec<u8>>,
}
impl Asset {
    /// `p` as stored: its normalized value, named by that value's bytes. A
    /// profile already normalized is kept as it is, shared.
    pub fn profile(p: &Arc<CameraProfile>) -> Self {
        let value = if asset::is_normalized(p) {
            p.clone()
        } else {
            Arc::new(asset::normalized(p.as_ref().clone()))
        };
        let canonical = asset::canonical_bytes(&value);
        Self {
            id: asset_id(AssetKind::Profile, &canonical),
            value: AssetValue::Profile(value),
            canonical: Arc::new(canonical),
        }
    }
    /// `c` as stored. A calibration is stored as it is read.
    pub fn calibration(c: &Arc<Calibration>) -> Self {
        let canonical = serde_json::to_vec(c.as_ref()).expect("a calibration serializes");
        Self {
            id: asset_id(AssetKind::Calibration, &canonical),
            value: AssetValue::Calibration(c.clone()),
            canonical: Arc::new(canonical),
        }
    }
    pub fn kind(&self) -> AssetKind {
        self.value.kind()
    }
    /// The bytes it is named by.
    pub fn canonical(&self) -> &[u8] {
        &self.canonical
    }
    /// The canonical bytes as a catalog stores them: zlib-compressed.
    pub fn compressed(&self) -> Vec<u8> {
        compress(&self.canonical)
    }
    /// The asset a catalog stores as `kind` and `stored` (compressed) under `id`:
    /// inflated within [`MAX_ASSET_BYTES`], checked against `id` before it is
    /// parsed, then normalized as every reader holds it.
    pub fn from_stored(id: &str, kind: AssetKind, stored: &[u8]) -> Result<Self, AssetError> {
        let damaged = |why: String| AssetError::Damaged(kind, id.to_string(), why);
        let canonical = inflate(stored).map_err(damaged)?;
        if asset_id(kind, &canonical) != id {
            return Err(damaged("its data is not what its id names".into()));
        }
        Ok(Self {
            id: id.to_string(),
            value: parse(kind, &canonical).map_err(damaged)?,
            canonical: Arc::new(canonical),
        })
    }
}

/// zlib-compresses `canonical`.
pub fn compress(canonical: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    z.write_all(canonical)
        .and_then(|()| z.finish())
        .expect("compressing into memory cannot fail")
}

/// Inflates a stored asset, refusing one larger than [`MAX_ASSET_BYTES`].
pub fn inflate(stored: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    flate2::read::ZlibDecoder::new(stored)
        .take(MAX_ASSET_BYTES as u64 + 1)
        .read_to_end(&mut out)
        .map_err(|e| format!("it cannot be read ({e})"))?;
    if out.len() > MAX_ASSET_BYTES {
        return Err(format!("it is larger than {MAX_ASSET_BYTES} bytes"));
    }
    Ok(out)
}

fn parse(kind: AssetKind, canonical: &[u8]) -> Result<AssetValue, String> {
    let why = |e: serde_json::Error| format!("it cannot be read ({e})");
    Ok(match kind {
        AssetKind::Profile => AssetValue::Profile(Arc::new(asset::normalized(
            serde_json::from_slice(canonical).map_err(why)?,
        ))),
        AssetKind::Calibration => {
            AssetValue::Calibration(Arc::new(serde_json::from_slice(canonical).map_err(why)?))
        }
    })
}

/// Where stored assets are read from: a catalog, or a transaction in one.
pub trait AssetSource {
    /// The kind and stored (compressed) bytes of the asset `id`; `Ok(None)` when it
    /// has none. Verifying them is [`reading`]'s.
    fn stored(&self, id: &str) -> Result<Option<(AssetKind, Vec<u8>)>, String>;
}

/// A source with no assets: stored text read through it resolves no reference.
pub struct NoAssets;
impl AssetSource for NoAssets {
    fn stored(&self, _: &str) -> Result<Option<(AssetKind, Vec<u8>)>, String> {
        Ok(None)
    }
}

/// Runs `parse`, which deserializes recipes from `text`, with the references in
/// `text` resolved through `source`. Every id in `text` is resolved before `parse`
/// runs (from the cache when it holds it), so the deserializers find them without
/// reaching the source; an id `source` doesn't have is reported by the deserializer
/// that reads it as a reference, and ids that are not references (a mask raster's)
/// are left alone.
pub fn reading<R>(source: &dyn AssetSource, text: &str, parse: impl FnOnce() -> R) -> R {
    with_scope(resolve_all(source, text, true), parse)
}

/// [`reading`] without the cache: every reference is read from `source` and
/// verified there, whatever this process holds. The catalog's upgrade checks what
/// it wrote this way, so a damaged row is caught even when the right value is in
/// memory.
pub fn reading_uncached<R>(source: &dyn AssetSource, text: &str, parse: impl FnOnce() -> R) -> R {
    with_scope(resolve_all(source, text, false), parse)
}

type Scope = HashMap<String, Result<AssetValue, AssetError>>;

fn resolve_all(source: &dyn AssetSource, text: &str, cached: bool) -> Scope {
    let mut scope = Scope::new();
    for id in ids_in(text) {
        if scope.contains_key(&id) {
            continue;
        }
        if cached && let Some(value) = cache_get(&id) {
            scope.insert(id, Ok(value));
            continue;
        }
        let found = match source.stored(&id) {
            Ok(None) => continue,
            Ok(Some((kind, stored))) => Asset::from_stored(&id, kind, &stored).map(|asset| {
                if cached {
                    // Another read may have cached it meanwhile: share its value.
                    remember(&asset)
                } else {
                    asset.value
                }
            }),
            Err(why) => Err(AssetError::Unreadable(id.clone(), why)),
        };
        scope.insert(id, found);
    }
    scope
}

/// Every JSON string in `text` that has an asset id's form, escapes decoded, once
/// each.
fn ids_in(text: &str) -> Vec<String> {
    const ID: usize = 7 + 64;
    let bytes = text.as_bytes();
    let mut found = Vec::new();
    let mut at = 0;
    while let Some(start) = bytes[at..].iter().position(|b| *b == b'"').map(|p| at + p) {
        let mut end = start + 1;
        let mut escaped = false;
        while end < bytes.len() && bytes[end] != b'"' {
            if bytes[end] == b'\\' {
                escaped = true;
                end += 1;
            }
            end += 1;
        }
        if end >= bytes.len() {
            break;
        }
        let id = if escaped {
            serde_json::from_str::<String>(&text[start..=end]).ok()
        } else {
            (end - start - 1 == ID).then(|| text[start + 1..end].to_string())
        };
        if let Some(id) = id
            && is_asset_id(&id)
            && !found.contains(&id)
        {
            found.push(id);
        }
        at = end + 1;
    }
    found
}

thread_local! {
    static SCOPES: std::cell::RefCell<Vec<Scope>> = const { std::cell::RefCell::new(Vec::new()) };
}
fn with_scope<R>(scope: Scope, parse: impl FnOnce() -> R) -> R {
    /// Leaves the scope however `parse` ends, a panic included.
    struct Leave;
    impl Drop for Leave {
        fn drop(&mut self) {
            SCOPES.with(|s| s.borrow_mut().pop());
        }
    }
    SCOPES.with(|s| s.borrow_mut().push(scope));
    let _leave = Leave;
    parse()
}

/// The asset `id` of `kind`, as the [`reading`] around this read resolved it.
fn lookup(kind: AssetKind, id: &str) -> Result<AssetValue, AssetError> {
    SCOPES.with(|s| {
        let scopes = s.borrow();
        let scope = scopes.last().ok_or(AssetError::NoSource)?;
        let value = scope
            .get(id)
            .cloned()
            .ok_or_else(|| AssetError::Missing(kind, id.to_string()))??;
        if value.kind() != kind {
            return Err(AssetError::Damaged(
                kind,
                id.to_string(),
                format!("it is a {}", value.kind().name()),
            ));
        }
        Ok(value)
    })
}

/// The camera profile a stored recipe names by `id`.
pub fn profile(id: &str) -> Result<Arc<CameraProfile>, AssetError> {
    match lookup(AssetKind::Profile, id)? {
        AssetValue::Profile(p) => Ok(p),
        AssetValue::Calibration(_) => unreachable!("kind checked by lookup"),
    }
}

/// The Lensfun calibration a stored recipe names by `id`.
pub fn calibration(id: &str) -> Result<Arc<Calibration>, AssetError> {
    match lookup(AssetKind::Calibration, id)? {
        AssetValue::Calibration(c) => Ok(c),
        AssetValue::Profile(_) => unreachable!("kind checked by lookup"),
    }
}

enum WeakValue {
    Profile(Weak<CameraProfile>),
    Calibration(Weak<Calibration>),
}
impl WeakValue {
    fn upgrade(&self) -> Option<AssetValue> {
        match self {
            Self::Profile(p) => p.upgrade().map(AssetValue::Profile),
            Self::Calibration(c) => c.upgrade().map(AssetValue::Calibration),
        }
    }
}
#[derive(Default)]
struct Cache {
    weak: HashMap<String, WeakValue>,
    recent: VecDeque<(String, AssetValue)>,
}
fn cache() -> std::sync::MutexGuard<'static, Cache> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    CACHE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
fn cache_get(id: &str) -> Option<AssetValue> {
    let mut cache = cache();
    let value = cache.weak.get(id)?.upgrade()?;
    touch(&mut cache, id, &value);
    Some(value)
}
fn touch(cache: &mut Cache, id: &str, value: &AssetValue) {
    cache.recent.retain(|(held, _)| held != id);
    cache.recent.push_front((id.to_string(), value.clone()));
    cache.recent.truncate(RECENT);
}

/// Shares `asset`'s value with later reads of its id, and returns the value to
/// hold: the one recipes in memory already share, when there is one. Callers pass
/// only assets whose bytes a source verified or that name their own value (a
/// write's).
pub fn remember(asset: &Asset) -> AssetValue {
    let mut cache = cache();
    if let Some(held) = cache.weak.get(&asset.id).and_then(WeakValue::upgrade) {
        // Recipes in memory already share a value: keep it, so they stay shared.
        touch(&mut cache, &asset.id, &held);
        return held;
    }
    if cache.weak.len() >= 1024 {
        cache.weak.retain(|_, v| v.upgrade().is_some());
    }
    cache.weak.insert(asset.id.clone(), asset.value.downgrade());
    touch(&mut cache, &asset.id, &asset.value);
    asset.value.clone()
}

/// Forgets every cached value, as a new process starts. Values recipes in memory
/// hold stay theirs.
pub fn clear_cache() {
    let mut cache = cache();
    cache.weak.clear();
    cache.recent.clear();
}

#[cfg(test)]
mod tests;
