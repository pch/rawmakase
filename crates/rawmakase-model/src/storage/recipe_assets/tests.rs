use super::*;
use crate::camera_data::Metadata;
use std::cell::Cell;

/// A catalog's assets in memory, counting its reads.
#[derive(Default)]
struct Assets {
    rows: HashMap<String, (AssetKind, Vec<u8>)>,
    reads: Cell<usize>,
}
impl Assets {
    fn of(assets: &[Asset]) -> Self {
        Self {
            rows: assets
                .iter()
                .map(|a| (a.id.clone(), (a.kind(), a.compressed())))
                .collect(),
            ..Default::default()
        }
    }
}
impl AssetSource for Assets {
    fn stored(&self, id: &str) -> Result<Option<(AssetKind, Vec<u8>)>, String> {
        self.reads.set(self.reads.get() + 1);
        Ok(self.rows.get(id).cloned())
    }
}

/// A profile no other test makes, so the shared cache never holds it already.
fn unique_profile(seed: f32) -> Arc<CameraProfile> {
    let m = Metadata {
        cam_xyz: [[0.9, -0.2, seed], [-0.4, 1.2, 0.15], [-0.05, 0.1, 0.6]],
        ..Default::default()
    };
    Arc::new(CameraProfile::camera_matrix_default(&m).unwrap())
}

#[test]
fn ids_have_one_form_and_name_their_kind() {
    let id = asset_id(AssetKind::Profile, b"{}");
    assert!(is_asset_id(&id), "{id}");
    assert_ne!(id, asset_id(AssetKind::Calibration, b"{}"));
    assert!(!is_asset_id("sha256:ABC"));
    assert!(!is_asset_id(&id.to_uppercase()));
    for kind in AssetKind::ALL {
        assert_eq!(AssetKind::from_name(kind.name()), Some(kind));
    }
}

/// A stored reference read with no catalog in scope fails clearly, never renders
/// without its profile.
#[test]
fn a_reference_read_outside_its_catalog_fails() {
    let error = profile(&asset_id(AssetKind::Profile, b"x")).unwrap_err();
    assert_eq!(error, AssetError::NoSource);
    assert_eq!(
        error.to_string(),
        "a catalog recipe read outside its catalog"
    );
}

#[test]
fn a_missing_or_damaged_asset_is_reported_by_id() {
    let p = unique_profile(0.011);
    let asset = Asset::profile(&p);
    let text = format!("\"{}\"", asset.id);
    let missing = reading(&NoAssets, &text, || profile(&asset.id)).unwrap_err();
    assert_eq!(
        missing.to_string(),
        format!("Camera profile {} is missing from the catalog", asset.id)
    );
    // Another profile's bytes under this id.
    let mut assets = Assets::of(std::slice::from_ref(&asset));
    let other = Asset::profile(&unique_profile(0.012));
    assets.rows.get_mut(&asset.id).unwrap().1 = other.compressed();
    let damaged = reading_uncached(&assets, &text, || profile(&asset.id)).unwrap_err();
    assert!(
        matches!(damaged, AssetError::Damaged(AssetKind::Profile, ..)),
        "{damaged}"
    );
    // Bytes that are not compressed.
    assets.rows.get_mut(&asset.id).unwrap().1 = b"not zlib".to_vec();
    assert!(reading_uncached(&assets, &text, || profile(&asset.id)).is_err());
    // A profile asked for as a calibration.
    let assets = Assets::of(std::slice::from_ref(&asset));
    let wrong = reading_uncached(&assets, &text, || calibration(&asset.id)).unwrap_err();
    assert!(
        matches!(wrong, AssetError::Damaged(AssetKind::Calibration, ..)),
        "{wrong}"
    );
}

#[test]
fn an_asset_inflating_past_the_bound_is_refused() {
    let huge = compress(&vec![b' '; MAX_ASSET_BYTES + 1]);
    let id = asset_id(AssetKind::Profile, b"irrelevant");
    let error = Asset::from_stored(&id, AssetKind::Profile, &huge).unwrap_err();
    assert!(error.to_string().contains("larger than"), "{error}");
}

/// Recipes naming one asset share one value, read from the catalog once.
#[test]
fn reads_of_one_asset_share_its_value() {
    let p = unique_profile(0.013);
    let asset = Asset::profile(&p);
    let assets = Assets::of(std::slice::from_ref(&asset));
    let text = format!("[\"{}\", \"{}\"]", asset.id, asset.id);
    let a = reading(&assets, &text, || profile(&asset.id)).unwrap();
    let b = reading(&assets, &text, || profile(&asset.id)).unwrap();
    assert!(Arc::ptr_eq(&a, &b));
    assert_eq!(*a, *p);
    assert_eq!(assets.reads.get(), 1, "the second read came from the cache");
}

/// The upgrade's self-check reads past the cache: a damaged row is caught even
/// while the right value is in memory.
#[test]
fn an_uncached_read_checks_the_catalog_whatever_the_cache_holds() {
    let p = unique_profile(0.014);
    let asset = Asset::profile(&p);
    remember(&asset);
    let text = format!("\"{}\"", asset.id);
    assert!(reading(&NoAssets, &text, || profile(&asset.id)).is_ok());
    let error = reading_uncached(&NoAssets, &text, || profile(&asset.id)).unwrap_err();
    assert!(matches!(error, AssetError::Missing(..)), "{error}");
    drop(p);
}

#[test]
fn only_quoted_ids_are_resolved() {
    let id = asset_id(AssetKind::Profile, b"y");
    let text =
        format!("{{\"a\": \"{id}\", \"b\": \"{id}x\", \"c\": \"x{id}\", \"d\": \"sha256:12\"}}");
    assert_eq!(ids_in(&text), std::slice::from_ref(&id));
}

/// A scope ends with its parse, a panic included, and scopes nest.
#[test]
fn scopes_end_with_their_parse() {
    let p = unique_profile(0.015);
    let asset = Asset::profile(&p);
    let assets = Assets::of(std::slice::from_ref(&asset));
    let text = format!("\"{}\"", asset.id);
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        reading(&assets, &text, || panic!("parse failed"))
    }));
    assert!(caught.is_err());
    assert_eq!(profile(&asset.id).unwrap_err(), AssetError::NoSource);
    reading(&assets, &text, || {
        let inner = reading(&NoAssets, "", || profile(&asset.id));
        assert!(matches!(inner, Err(AssetError::Missing(..))));
        assert!(profile(&asset.id).is_ok(), "the outer scope is back");
    });
}

/// A profile a write stores is cached as readers hold it: an Embedded profile with
/// an old tone curve is cached with today's, as reading its stored bytes gives it.
#[test]
fn a_stored_profile_is_cached_normalized() {
    let m = Metadata {
        source: crate::camera_data::Source::Rendered,
        make: "Normalized".into(),
        ..Default::default()
    };
    let mut v = serde_json::to_value(CameraProfile::embedded(&m)).unwrap();
    v["tone"] = serde_json::json!([[0., 0.], [1., 1.]]);
    let old: CameraProfile = serde_json::from_value(v).unwrap();
    let asset = Asset::profile(&Arc::new(old));
    let AssetValue::Profile(held) = &asset.value else {
        unreachable!()
    };
    assert_eq!(held.as_ref(), &CameraProfile::embedded(&m));
    remember(&asset);
    let assets = Assets::of(std::slice::from_ref(&asset));
    let text = format!("\"{}\"", asset.id);
    let cached = reading(&assets, &text, || profile(&asset.id)).unwrap();
    let uncached = reading_uncached(&assets, &text, || profile(&asset.id)).unwrap();
    assert_eq!(cached, uncached);
}

/// A reference written with JSON escapes is the same reference.
#[test]
fn an_escaped_reference_resolves() {
    let p = unique_profile(0.016);
    let asset = Asset::profile(&p);
    let assets = Assets::of(std::slice::from_ref(&asset));
    let escaped = format!("\"\\u0073{}\"", &asset.id[1..]);
    let id: String = serde_json::from_str(&escaped).unwrap();
    assert_eq!(id, asset.id);
    let read = reading_uncached(&assets, &escaped, || {
        serde_json::from_str::<String>(&escaped).map(|id| profile(&id))
    })
    .unwrap()
    .unwrap();
    assert_eq!(*read, *p);
}

/// Two reads of one asset that both miss the cache still share one value.
#[test]
fn concurrent_cold_reads_share_one_value() {
    use std::sync::Barrier;
    struct Slow<'a>(Assets, &'a Barrier);
    impl AssetSource for Slow<'_> {
        fn stored(&self, id: &str) -> Result<Option<(AssetKind, Vec<u8>)>, String> {
            // Both readers have missed the cache before either is answered.
            self.1.wait();
            self.0.stored(id)
        }
    }
    let p = unique_profile(0.017);
    let asset = Asset::profile(&p);
    drop(p);
    let barrier = Barrier::new(2);
    let text = format!("\"{}\"", asset.id);
    let read = || {
        let source = Slow(Assets::of(std::slice::from_ref(&asset)), &barrier);
        reading(&source, &text, || profile(&asset.id)).unwrap()
    };
    let (a, b) = std::thread::scope(|s| {
        let a = s.spawn(read);
        let b = s.spawn(read);
        (a.join().unwrap(), b.join().unwrap())
    });
    assert!(Arc::ptr_eq(&a, &b));
}
