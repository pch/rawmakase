//! Opening a photo reads the RAW decoder's facts, then what the file says beyond them: a
//! DNG's embedded profile, baseline exposure and default crop, and the lens
//! tables a camera embeds. Moving that interpretation between modules must not
//! change what an opened photo holds, so this pins a digest of every corpus chart
//! as `photo::open` returns it. Rerun with RAWMAKASE_BLESS=1 only for an intended change.
//!
//! Imported lens profiles are left out: they come from the user's library, which
//! differs between machines.
use rawmakase::photo;
use std::{collections::BTreeMap, path::Path};

/// FNV-1a, stable across Rust releases unlike the standard hasher.
fn digest(bytes: &[u8]) -> String {
    let hash = bytes.iter().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3)
    });
    format!("{hash:016x}")
}

fn opened(path: &Path) -> String {
    let raw = photo::open(path, &rawmakase::photo::LensProfiles::none())
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let metadata = serde_json::to_vec(&raw.metadata).unwrap();
    let profile = serde_json::to_vec(&rawmakase::camera_profiles::builtin(&raw.metadata)).unwrap();
    digest(&[metadata, profile].concat())
}

#[test]
fn every_chart_opens_as_it_did() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let charts = root.join("tests/corpus/charts");
    let mut actual = BTreeMap::new();
    for entry in std::fs::read_dir(&charts).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "dng") {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            actual.insert(name, opened(&path));
        }
    }
    let expected_path = root.join("tests/data/opened-photos.json");
    if std::env::var_os("RAWMAKASE_BLESS").is_some() {
        let json = serde_json::to_string_pretty(&actual).unwrap() + "\n";
        std::fs::write(&expected_path, json).unwrap();
        return;
    }
    let expected: BTreeMap<String, String> =
        serde_json::from_str(&std::fs::read_to_string(&expected_path).unwrap()).unwrap();
    assert_eq!(
        actual, expected,
        "Opened photos changed (rerun with RAWMAKASE_BLESS=1 to accept)"
    );
}
