//! Reading editable previews without writing one, which is export's: a record's
//! missing fields read as their defaults and one it doesn't know is ignored, a
//! value or version this release doesn't know is refused, and a file that isn't
//! a copy is refused without a panic. Export's tests write copies and open them
//! here (`rawmakase_export::export::editable_preview`).
use super::record::{MAGIC, MAX_RECORD, VERSION};
use super::*;
use std::path::PathBuf;

/// DNGPrivateData of a record of `version` holding `json`.
fn private(version: u16, json: &serde_json::Value) -> Vec<u8> {
    let mut bytes = MAGIC.to_vec();
    bytes.extend(version.to_be_bytes());
    bytes.extend(serde_json::to_vec(json).unwrap());
    bytes
}

/// The fields every record needs: the original's size and the copy's.
fn sizes() -> serde_json::Value {
    serde_json::json!({"width": 6, "height": 4, "copy_width": 3, "copy_height": 2})
}

#[test]
fn missing_fields_read_as_their_defaults_and_unknown_ones_are_ignored() {
    let mut json = sizes();
    json["a_later_fact"] = true.into();
    let record = Record::parse(&private(VERSION, &json)).unwrap();
    let m = record.metadata().unwrap();
    assert_eq!((m.width, m.height), (6, 4));
    assert_eq!((m.crop_width, m.crop_height, m.flip), (0, 0, 0));
    assert!(m.lateral_ca.get().is_none(), "not measured");
    assert!(record.default_look(&m).unwrap().is_none());
}

#[test]
fn a_value_or_version_this_release_doesnt_know_is_refused() {
    let mut json = sizes();
    json["matrix_source"] = "from_the_future".into();
    assert!(Record::parse(&private(VERSION, &json)).is_err());
    let later = format!(
        "{:#}",
        Record::parse(&private(VERSION + 1, &sizes())).unwrap_err()
    );
    assert!(later.contains("later release"), "{later}");
    assert!(Record::parse(&private(0, &sizes())).is_err());
    // A size missing, damaged JSON, another maker's data, nothing after the magic.
    let mut no_copy = sizes();
    no_copy.as_object_mut().unwrap().remove("copy_width");
    assert!(Record::parse(&private(VERSION, &no_copy)).is_err());
    let mut damaged = private(VERSION, &sizes());
    damaged[MAGIC.len() + 2] = b'[';
    assert!(Record::parse(&damaged).is_err());
    assert!(Record::parse(b"Adobe\0MakN").is_err());
    assert!(Record::parse(MAGIC).is_err());
    // One whose length is beyond any record's.
    let huge = vec![b' '; MAX_RECORD as usize + 1];
    assert!(Record::parse(&huge).is_err());
}

/// A file that isn't RAWmakase's copy is an error for [`open`], never a panic:
/// an ordinary DNG (another maker's private data or none), a file that isn't a
/// TIFF, an empty file, a missing one.
#[test]
fn a_file_that_isnt_a_copy_is_refused() {
    let chart =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/charts/synthetic-d65.dng");
    let error = format!(
        "{:#}",
        open(&chart, &crate::photo::LensProfiles::none(), Cancel::NEVER)
            .err()
            .unwrap()
    );
    assert!(error.contains("no RAWmakase record"), "{error}");
    let dir = tempfile::tempdir().unwrap();
    let file = |name: &str, bytes: &[u8]| -> PathBuf {
        let path = dir.path().join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    };
    assert!(
        open(
            &file("text.dng", b"not a DNG"),
            &crate::photo::LensProfiles::none(),
            Cancel::NEVER
        )
        .is_err()
    );
    assert!(
        open(
            &file("empty.dng", b""),
            &crate::photo::LensProfiles::none(),
            Cancel::NEVER
        )
        .is_err()
    );
    assert!(
        open(
            &dir.path().join("missing.dng"),
            &crate::photo::LensProfiles::none(),
            Cancel::NEVER
        )
        .is_err()
    );
}
