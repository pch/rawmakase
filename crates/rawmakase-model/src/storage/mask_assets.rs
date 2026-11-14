//! The rasters recipes refer to by content ID: the coverage of bitmap mask
//! components, and the camera RGB of AI Remove patches.
//!
//! Recipes carry only IDs. The pixels live in one process-wide store: a generated
//! selection is registered here the moment it exists and is *unsaved* until a
//! catalog write has stored it; anything else is read through the catalog the app
//! installed as the [`AssetLoader`] the first time it is needed and may be evicted
//! again, so saved history never pins every decoded raster. Rendering, previews and
//! exports all read through [`resolve_coverage`] and [`resolve_patch`], which
//! check that what they find is the kind of raster asked for, so no job has to
//! carry rasters along.
//!
//! The store knows nothing of catalogs, renderers or inference: the model crate owns
//! only the values and their budget.
use super::bitmaps::Bitmap;
use anyhow::{Result, ensure};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

/// Most pixels along either axis of one raster.
pub const MAX_SIDE: u32 = 4096;
/// Most decoded bytes of one raster: a patch of [`MAX_PATCH_SIDE`]² pixels of
/// three half-precision samples, over 8-bit coverage's 16 MiB.
pub const MAX_RASTER_BYTES: usize = 24 << 20;
/// Most pixels along either side of an AI Remove patch.
pub const MAX_PATCH_SIDE: u32 = crate::model::retouch::MAX_PATCH_SIDE;
/// Most decoded bytes the store keeps of rasters that are saved and can be read
/// again. Unsaved rasters are never evicted.
const CACHE_BYTES: usize = 256 << 20;
/// Most decoded bytes of distinct rasters one generation may add to an edit.
pub const EDIT_BYTES: usize = 256 << 20;

/// Why a raster could not be provided.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AssetError {
    /// Nothing stores a raster with this ID.
    Missing(String),
    /// A raster is stored but is damaged or is not what the recipe describes.
    Corrupt(String, String),
    /// An AI Remove patch's pixels are missing.
    PatchMissing(String),
    /// An AI Remove patch's pixels are damaged or not what the spot describes.
    PatchCorrupt(String, String),
}
impl AssetError {
    /// The same failure, said of a patch.
    fn of_patch(self) -> Self {
        match self {
            Self::Missing(id) => Self::PatchMissing(id),
            Self::Corrupt(id, why) => Self::PatchCorrupt(id, why),
            other => other,
        }
    }
}
impl std::fmt::Display for AssetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing(id) => write!(f, "A mask's data ({}) is missing", short(id)),
            Self::Corrupt(id, why) => write!(f, "A mask's data ({}) is damaged: {why}", short(id)),
            Self::PatchMissing(id) => {
                write!(f, "An AI Remove spot's pixels ({}) are missing", short(id))
            }
            Self::PatchCorrupt(id, why) => write!(
                f,
                "An AI Remove spot's pixels ({}) are damaged: {why}",
                short(id)
            ),
        }
    }
}
impl std::error::Error for AssetError {}
fn short(id: &str) -> &str {
    &id[..id.len().min(15)]
}

/// Reads rasters that are not in the store, from where the app keeps them.
pub trait AssetLoader: Send + Sync {
    /// The raster stored under `id`, or `None` when nothing is.
    fn load(&self, id: &str) -> Result<Option<Bitmap>, AssetError>;
    /// What it reads from (a catalog's location): opening the same catalog again
    /// replaces its reader rather than adding one.
    fn source(&self) -> String;
}

struct Entry {
    raster: Arc<Bitmap>,
    /// The compressed form, kept while the raster is unsaved so each write compresses
    /// it only once.
    blob: Option<Arc<Vec<u8>>>,
    saved: bool,
    used: u64,
}
#[derive(Default)]
struct Store {
    entries: HashMap<String, Entry>,
    /// Readers of saved rasters, one per catalog opened in this process, the last
    /// opened first. Earlier catalogs' readers stay: an export or preview build started
    /// under one keeps resolving its rasters after the app has moved on to another, and
    /// a content ID names the same pixels wherever they are read from. A reader holds a
    /// connection only once it has been asked for something.
    loaders: Vec<Arc<dyn AssetLoader>>,
    clock: u64,
}
fn store() -> &'static Mutex<Store> {
    static STORE: OnceLock<Mutex<Store>> = OnceLock::new();
    STORE.get_or_init(Default::default)
}
fn lock() -> std::sync::MutexGuard<'static, Store> {
    store()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// What a stored raster is, by its samples.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A mask's coverage: one 8-bit channel.
    Coverage,
    /// An AI Remove patch: three channels of little-endian f16, every sample
    /// finite, at most [`MAX_PATCH_SIDE`] along either side.
    Patch,
}
impl Kind {
    /// What `bitmap`'s samples say it is, whether or not it is valid.
    fn of(bitmap: &Bitmap) -> Option<Self> {
        match (bitmap.channels, bitmap.depth) {
            (1, 1) => Some(Self::Coverage),
            (3, 2) => Some(Self::Patch),
            _ => None,
        }
    }
}

/// Checks that `bitmap` is a raster of `kind` within the limits.
pub fn validate_kind(bitmap: &Bitmap, kind: Kind) -> Result<()> {
    let (channels, depth, side, name) = match kind {
        Kind::Coverage => (1, 1, MAX_SIDE, "A mask raster"),
        Kind::Patch => (3, 2, MAX_PATCH_SIDE, "An AI Remove patch"),
    };
    ensure!(
        bitmap.channels == channels && bitmap.depth == depth,
        match kind {
            Kind::Coverage => "A mask raster is 8-bit coverage",
            Kind::Patch => "An AI Remove patch is three channels of half-precision camera RGB",
        }
    );
    ensure!(
        (1..=side).contains(&bitmap.width) && (1..=side).contains(&bitmap.height),
        "{name} is at most {side} pixels along either side"
    );
    let len = bitmap.width as usize * bitmap.height as usize * channels as usize * depth as usize;
    ensure!(
        bitmap.data.len() == len && bitmap.data.len() <= MAX_RASTER_BYTES,
        "{name} has the wrong size"
    );
    if kind == Kind::Patch {
        ensure!(
            bitmap
                .data
                .as_chunks::<2>()
                .0
                .iter()
                .all(|s| u16::from_le_bytes(*s) & 0x7c00 != 0x7c00),
            "An AI Remove patch holds a sample that isn't a finite number"
        );
    }
    Ok(())
}

/// Checks that `bitmap` is a raster of either kind within the limits.
pub fn validate_raster(bitmap: &Bitmap) -> Result<()> {
    let kind = Kind::of(bitmap).unwrap_or(Kind::Coverage);
    validate_kind(bitmap, kind)
}

/// Registers a new raster as unsaved and returns its content ID. Registering the
/// same coverage again returns the same ID.
pub fn register(bitmap: Bitmap) -> Result<String> {
    validate_raster(&bitmap)?;
    let id = bitmap.content_id();
    let blob = Arc::new(bitmap.compress()?);
    let mut store = lock();
    store.clock += 1;
    let used = store.clock;
    let entry = store.entries.entry(id.clone()).or_insert_with(|| Entry {
        raster: Arc::new(bitmap),
        blob: Some(blob.clone()),
        saved: false,
        used,
    });
    entry.used = used;
    // Saved before, perhaps in another catalog: whichever catalog this edit is saved
    // to next must be able to store it, so it is unsaved again until it is.
    if entry.saved {
        entry.saved = false;
        entry.blob = Some(blob);
    }
    Ok(id)
}

/// Installs the reader for the catalog just opened; readers of catalogs opened
/// before it are asked after it, for jobs still running under them.
pub fn add_loader(loader: Arc<dyn AssetLoader>) {
    let mut store = lock();
    let source = loader.source();
    store.loaders.retain(|l| l.source() != source);
    store.loaders.insert(0, loader);
}

/// The raster stored under `id`: from the store, else read through the loader.
/// A raster read must be what its ID names and what a mask describes.
pub fn resolve(id: &str) -> Result<Arc<Bitmap>, AssetError> {
    let loader = {
        let mut store = lock();
        store.clock += 1;
        let used = store.clock;
        if let Some(entry) = store.entries.get_mut(id) {
            entry.used = used;
            return Ok(entry.raster.clone());
        }
        store.loaders.clone()
    };
    // The first reader that has it; a reader that fails does not hide one that has it.
    let mut failure = None;
    let mut found = None;
    for loader in &loader {
        match loader.load(id) {
            Ok(Some(bitmap)) => {
                found = Some(bitmap);
                break;
            }
            Ok(None) => {}
            Err(e) => {
                failure.get_or_insert(e);
            }
        }
    }
    let bitmap = match (found, failure) {
        (Some(bitmap), _) => bitmap,
        (None, Some(e)) => return Err(e),
        (None, None) => return Err(AssetError::Missing(id.to_string())),
    };
    validate_raster(&bitmap).map_err(|e| AssetError::Corrupt(id.to_string(), e.to_string()))?;
    if !bitmap.matches_id(id) {
        return Err(AssetError::Corrupt(
            id.to_string(),
            "its content does not match its ID".into(),
        ));
    }
    let raster = Arc::new(bitmap);
    let mut store = lock();
    store.clock += 1;
    let used = store.clock;
    let entry = store
        .entries
        .entry(id.to_string())
        .or_insert_with(|| Entry {
            raster: raster.clone(),
            blob: None,
            saved: true,
            used,
        });
    let out = entry.raster.clone();
    evict(&mut store);
    Ok(out)
}

/// The raster stored under `id`, checked to be `kind`: a raster of another
/// kind is as damaged as a missing one.
fn resolve_kind(id: &str, kind: Kind) -> Result<Arc<Bitmap>, AssetError> {
    let raster = resolve(id)?;
    validate_kind(&raster, kind).map_err(|e| AssetError::Corrupt(id.to_string(), e.to_string()))?;
    Ok(raster)
}

/// A mask's coverage stored under `id`: 8-bit, one channel.
pub fn resolve_coverage(id: &str) -> Result<Arc<Bitmap>, AssetError> {
    resolve_kind(id, Kind::Coverage)
}

/// An AI Remove patch stored under `id`: three channels of finite f16, at most
/// [`MAX_PATCH_SIDE`] along either side.
pub fn resolve_patch(id: &str) -> Result<Arc<Bitmap>, AssetError> {
    resolve_kind(id, Kind::Patch).map_err(AssetError::of_patch)
}

/// Makes sure every raster of `refs` (`(id, width, height)` as a mask describes it)
/// can be provided, is coverage and is the size the mask says: a stored raster of
/// another size is as damaged as a missing one, since the mask would render wrongly
/// placed or empty.
pub fn ensure_shaped<'a>(
    refs: impl IntoIterator<Item = (&'a str, u32, u32)>,
) -> Result<(), AssetError> {
    for (id, width, height) in refs {
        let raster = resolve_coverage(id)?;
        check_shape(id, &raster, width, height, "the mask")?;
    }
    Ok(())
}

/// Makes sure every patch of `refs` (`(id, width, height)` as a spot describes
/// it) can be provided, is a patch and is the size the spot says.
pub fn ensure_patches<'a>(
    refs: impl IntoIterator<Item = (&'a str, u32, u32)>,
) -> Result<(), AssetError> {
    for (id, width, height) in refs {
        let raster = resolve_patch(id)?;
        check_shape(id, &raster, width, height, "the spot").map_err(AssetError::of_patch)?;
    }
    Ok(())
}

fn check_shape(
    id: &str,
    raster: &Bitmap,
    width: u32,
    height: u32,
    what: &str,
) -> Result<(), AssetError> {
    if (raster.width, raster.height) != (width, height) {
        return Err(AssetError::Corrupt(
            id.to_string(),
            format!(
                "it is {}x{} but {what} describes {width}x{height}",
                raster.width, raster.height
            ),
        ));
    }
    Ok(())
}

/// Forgets an unsaved raster nothing refers to any more (a selection the user did
/// not keep). A saved one stays cached; a raster still referred to must not be passed.
pub fn discard_unsaved(id: &str) {
    let mut store = lock();
    if store.entries.get(id).is_some_and(|e| !e.saved) {
        store.entries.remove(id);
    }
}

/// Whether the raster is in the store without reading anything.
pub fn is_held(id: &str) -> bool {
    lock().entries.contains_key(id)
}

/// Makes sure every raster of `ids` can be provided, reading the ones that are not
/// held. The first failure is returned.
pub fn ensure_all<'a>(ids: impl IntoIterator<Item = &'a str>) -> Result<(), AssetError> {
    ids.into_iter().try_for_each(|id| resolve(id).map(drop))
}

/// The compressed rasters among `ids` that a catalog write must store because they
/// were generated here and not yet saved.
pub fn unsaved<'a>(ids: impl IntoIterator<Item = &'a str>) -> Vec<(String, Arc<Vec<u8>>)> {
    let store = lock();
    ids.into_iter()
        .filter_map(|id| {
            let entry = store.entries.get(id)?;
            let blob = entry.blob.clone().filter(|_| !entry.saved)?;
            Some((id.to_string(), blob))
        })
        .collect()
}

/// Notes that a catalog stored these rasters: they can be evicted and read again.
pub fn mark_saved<'a>(ids: impl IntoIterator<Item = &'a str>) {
    let mut store = lock();
    for id in ids {
        if let Some(entry) = store.entries.get_mut(id) {
            entry.saved = true;
            entry.blob = None;
        }
    }
    evict(&mut store);
}

/// Decoded bytes of the distinct rasters among `ids`.
pub fn decoded_bytes<'a>(ids: impl IntoIterator<Item = &'a str>) -> usize {
    let mut seen = std::collections::HashSet::new();
    let store = lock();
    ids.into_iter()
        .filter(|id| seen.insert(*id))
        .map(|id| store.entries.get(id).map_or(0, |e| e.raster.data.len()))
        .sum()
}

/// Bytes the store holds of saved rasters, which it keeps for reuse.
#[cfg(test)]
fn cached_bytes() -> usize {
    lock()
        .entries
        .values()
        .filter(|e| e.saved)
        .map(|e| e.raster.data.len())
        .sum()
}

/// Drops the least recently used saved rasters beyond the budget.
fn evict(store: &mut Store) {
    let mut total: usize = store
        .entries
        .values()
        .filter(|e| e.saved)
        .map(|e| e.raster.data.len())
        .sum();
    if total <= CACHE_BYTES {
        return;
    }
    let mut saved: Vec<(u64, String)> = store
        .entries
        .iter()
        .filter(|(_, e)| e.saved)
        .map(|(id, e)| (e.used, id.clone()))
        .collect();
    saved.sort();
    for (_, id) in saved {
        if total <= CACHE_BYTES {
            break;
        }
        // A render still holding the raster keeps it alive by its own Arc.
        if let Some(entry) = store.entries.remove(&id) {
            total -= entry.raster.data.len();
        }
    }
}

/// Forgets every raster and the readers (tests only).
#[cfg(any(test, feature = "test-support"))]
pub fn reset() {
    *lock() = Store::default();
}
/// Forgets every raster but keeps the readers (tests only).
#[cfg(test)]
fn reset_cache_only() {
    lock().entries.clear();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn raster(w: u32, h: u32, v: u8) -> Bitmap {
        Bitmap {
            width: w,
            height: h,
            channels: 1,
            depth: 1,
            data: vec![v; (w * h) as usize],
        }
    }
    struct Loader(Mutex<HashMap<String, Bitmap>>, AtomicUsize);
    impl AssetLoader for Loader {
        fn load(&self, id: &str) -> Result<Option<Bitmap>, AssetError> {
            self.1.fetch_add(1, Ordering::Relaxed);
            Ok(self.0.lock().unwrap().get(id).cloned())
        }
        fn source(&self) -> String {
            format!("{:p}", self)
        }
    }

    // One test: the store is shared by the process.
    #[test]
    fn the_store_holds_unsaved_rasters_and_reads_saved_ones_through_the_loader() {
        reset();
        let a = register(raster(4, 3, 200)).unwrap();
        assert!(a.starts_with("sha256:") && Bitmap::is_valid_id(&a));
        assert_eq!(register(raster(4, 3, 200)).unwrap(), a);
        assert_ne!(register(raster(3, 4, 200)).unwrap(), a);
        assert_eq!(resolve(&a).unwrap().data, vec![200; 12]);
        // Unsaved rasters are what a catalog write must store; saved ones are not.
        assert_eq!(unsaved([a.as_str()]).len(), 1);
        mark_saved([a.as_str()]);
        assert!(unsaved([a.as_str()]).is_empty());
        assert_eq!(decoded_bytes([a.as_str(), a.as_str()]), 12);
        // Not malformed rasters.
        assert!(
            register(Bitmap {
                channels: 3,
                ..raster(2, 2, 1)
            })
            .is_err()
        );
        assert!(register(raster(MAX_SIDE + 1, 1, 0)).is_err());

        // A raster that is not held is read, checked against its ID and kept.
        let stored = raster(5, 5, 9);
        let stored_again = || raster(5, 5, 9);
        let id = stored.content_id();
        let loader = Arc::new(Loader(
            Mutex::new(HashMap::from([
                (id.clone(), stored.clone()),
                ("sha256:".to_string() + &"0".repeat(64), stored),
            ])),
            AtomicUsize::new(0),
        ));
        assert_eq!(resolve(&id), Err(AssetError::Missing(id.clone())));
        add_loader(loader.clone());
        assert!(resolve(&id).is_ok() && resolve(&id).is_ok());
        assert_eq!(loader.1.load(Ordering::Relaxed), 1);
        assert_eq!(
            resolve("sha256:ffff").unwrap_err(),
            AssetError::Missing("sha256:ffff".into())
        );
        let wrong = "sha256:".to_string() + &"0".repeat(64);
        assert!(matches!(resolve(&wrong), Err(AssetError::Corrupt(..))));
        assert!(ensure_all([id.as_str(), "sha256:ffff"]).is_err());
        assert_eq!(cached_bytes(), 12 + 25);
        // The size a mask describes must be the raster's.
        assert!(ensure_shaped([(id.as_str(), 5, 5)]).is_ok());
        assert!(matches!(
            ensure_shaped([(id.as_str(), 5, 4)]),
            Err(AssetError::Corrupt(..))
        ));
        // A later catalog's reader comes first; an earlier one still serves its rasters.
        let later = Arc::new(Loader(Mutex::new(HashMap::new()), AtomicUsize::new(0)));
        add_loader(later.clone());
        reset_cache_only();
        assert!(resolve(&id).is_ok());
        assert_eq!(later.1.load(Ordering::Relaxed), 1);
        // Generated again (in another catalog, say), a saved raster is unsaved until
        // that catalog stores it.
        mark_saved([id.as_str()]);
        register(stored_again()).unwrap();
        assert_eq!(unsaved([id.as_str()]).len(), 1);
        mark_saved([id.as_str()]);
        // An unsaved raster the user did not keep is forgotten; a saved one is kept.
        let unkept = register(raster(2, 2, 77)).unwrap();
        assert!(is_held(&unkept));
        discard_unsaved(&unkept);
        assert!(!is_held(&unkept));
        discard_unsaved(&id);
        assert!(is_held(&id));

        // A patch is three channels of finite f16; each kind resolves only as
        // itself.
        let samples: Vec<f32> = (0..2 * 2 * 3).map(|i| i as f32 * 0.25).collect();
        let patch = super::super::patches::encode(2, 2, &samples).unwrap();
        let patch_id = register(patch.clone()).unwrap();
        assert!(resolve_patch(&patch_id).is_ok());
        assert!(ensure_patches([(patch_id.as_str(), 2, 2)]).is_ok());
        assert!(matches!(
            ensure_patches([(patch_id.as_str(), 2, 3)]),
            Err(AssetError::PatchCorrupt(..))
        ));
        assert!(matches!(
            resolve_coverage(&patch_id),
            Err(AssetError::Corrupt(..))
        ));
        assert!(matches!(
            resolve_patch(&id),
            Err(AssetError::PatchCorrupt(..))
        ));
        assert!(matches!(
            resolve_patch("sha256:ffff"),
            Err(AssetError::PatchMissing(..))
        ));
        assert!(
            AssetError::PatchMissing(patch_id)
                .to_string()
                .starts_with("An AI Remove spot's pixels")
        );
        let mut infinite = patch;
        infinite.data[0..2].copy_from_slice(&0x7c00u16.to_le_bytes());
        assert!(register(infinite).is_err());
        let wide = Bitmap {
            width: MAX_PATCH_SIDE + 1,
            height: 1,
            channels: 3,
            depth: 2,
            data: vec![0; (MAX_PATCH_SIDE as usize + 1) * 6],
        };
        assert!(register(wide).is_err());
        reset();
    }
}
