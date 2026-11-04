//! How long renaming and moving a large batch of photos takes on a real
//! volume, with the catalog as it ships (`synchronous=FULL`), as #692
//! measured: ignored by default, as the time is the drive's. Run it with
//! `TMPDIR` on the drive to measure (not tmpfs, whose syncs cost nothing):
//!
//! ```text
//! TMPDIR=~/.cache/throughput cargo test -p rawmakase-catalog --release \
//!     --test file_ops_throughput -- --ignored --nocapture
//! ```
use rawmakase_cancel::Cancel;
use rawmakase_catalog::catalog::Catalog;
use rawmakase_catalog::catalog::file_ops::{
    self, FileOp, ItemResult, Trash, TrashError, TrashReceipt,
};
use std::path::Path;
use std::time::Instant;

/// The batch #692 measured.
const PHOTOS: usize = 1500;

/// Each photo's share of the run, at most, in milliseconds. Before #692
/// a rename took ~120 ms a photo on #692's USB disk and ~128 ms on btrfs on
/// an SSD (moves ~93 ms), most of it checks folding every name in the
/// folder again for each photo; ~1.9 ms after (moves ~1.3 ms).
const BUDGET_MS: f64 = 5.0;

struct NoTrash;
impl Trash for NoTrash {
    fn trash(&self, _: &Path) -> Result<TrashReceipt, TrashError> {
        panic!("nothing should be trashed")
    }
}

/// A catalog of folder `trip` holding `PHOTOS` photos, every tenth with
/// an XMP sidecar, and folder `trip/sub`.
fn fixture(dir: &Path) -> (Catalog, std::path::PathBuf) {
    let root = dir.canonicalize().unwrap().join("trip");
    std::fs::create_dir_all(root.join("sub")).unwrap();
    for i in 0..PHOTOS {
        std::fs::write(root.join(format!("IMG_{i:04}.NEF")), format!("photo {i}")).unwrap();
        if i % 10 == 0 {
            std::fs::write(root.join(format!("IMG_{i:04}.xmp")), "<x:xmpmeta/>").unwrap();
        }
    }
    let mut cat = Catalog::create(&dir.join("Photos.rawmakase")).unwrap();
    cat.add_folder_with(&root, &Default::default()).unwrap();
    (cat, root)
}

/// Runs `op` in `cat`, returning milliseconds per photo; every item done.
fn timed(cat: &mut Catalog, op: FileOp, what: &str) -> f64 {
    let plan = file_ops::plan(cat, op).unwrap();
    assert!(plan.skipped().is_empty(), "{what}: {:?}", plan.skipped());
    let start = Instant::now();
    let outcome = file_ops::execute(cat, plan.confirm(), &NoTrash, Cancel::NEVER).unwrap();
    let ms = start.elapsed().as_secs_f64() * 1000.0 / PHOTOS as f64;
    assert!(outcome.stopped.is_none(), "{what}: {:?}", outcome.stopped);
    let done = outcome
        .items
        .iter()
        .filter(|i| {
            matches!(
                i.result,
                ItemResult::Renamed { .. } | ItemResult::Moved { .. }
            )
        })
        .count();
    assert_eq!(done, PHOTOS, "{what}");
    eprintln!("{what}: {PHOTOS} photos, {ms:.2} ms a photo");
    ms
}

#[test]
#[ignore = "measures the drive TMPDIR is on; run with --ignored"]
fn renaming_and_moving_a_large_batch_takes_little_per_photo() {
    let dir = tempfile::tempdir().unwrap();
    let (mut cat, root) = fixture(dir.path());
    let masters = |cat: &Catalog| -> Vec<_> {
        let mut photos: Vec<_> = cat
            .photos()
            .unwrap()
            .into_iter()
            .filter(|p| p.master.is_none() && p.path.extension().is_some_and(|e| e == "NEF"))
            .collect();
        photos.sort_by(|a, b| a.path.cmp(&b.path));
        photos
    };
    let renames = masters(&cat)
        .iter()
        .enumerate()
        .map(|(i, p)| (p.id, format!("Trip {i:04}")))
        .collect();
    let rename = timed(&mut cat, FileOp::RenamePhotos { renames }, "rename");
    let root_id = cat.roots().unwrap()[0].0;
    let photos = masters(&cat).iter().map(|p| p.id).collect();
    let moved = timed(
        &mut cat,
        FileOp::MovePhotos {
            photos,
            root: root_id,
            to: "sub".into(),
        },
        "move",
    );
    assert!(root.join("sub/Trip 0000.NEF").is_file());
    assert!(root.join("sub/Trip 0000.xmp").is_file());
    assert!(rename <= BUDGET_MS, "rename: {rename:.2} ms a photo");
    assert!(moved <= BUDGET_MS, "move: {moved:.2} ms a photo");
}
