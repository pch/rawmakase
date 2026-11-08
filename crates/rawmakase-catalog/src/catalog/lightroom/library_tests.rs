use super::import_lightroom_library;
use crate::catalog::{Catalog, CollectionKind};
use anyhow::Result;
use std::path::{Path, PathBuf};

/// MessagePack of a document: a map of strings, numbers and nested maps.
enum V {
    S(&'static str),
    Owned(String),
    I(i64),
    M(Vec<(&'static str, V)>),
}
fn encode(v: &V, out: &mut Vec<u8>) {
    let text = |s: &str, out: &mut Vec<u8>| {
        out.push(0xd9);
        out.push(s.len() as u8);
        out.extend(s.as_bytes());
    };
    match v {
        V::S(s) => text(s, out),
        V::Owned(s) => text(s, out),
        V::I(n) => {
            out.push(0xd3);
            out.extend(n.to_be_bytes());
        }
        V::M(entries) => {
            out.push(0xde);
            out.extend((entries.len() as u16).to_be_bytes());
            for (k, v) in entries {
                text(k, out);
                encode(v, out);
            }
        }
    }
}
fn id(key: &'static str, of: &str) -> (&'static str, V) {
    (key, V::M(vec![("id", V::Owned(of.into()))]))
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest as _, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

const EDIT: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" crs:ProcessVersion="15.4" crs:Exposure2012="+0.27"/></rdf:RDF></x:xmpmeta>"#;

/// The edit of `later`, not in the library until Lightroom downloads it.
const LATER_EDIT: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" crs:ProcessVersion="15.4" crs:Exposure2012="-0.50"/></rdf:RDF></x:xmpmeta>"#;

struct Library {
    _dir: tempfile::TempDir,
    root: PathBuf,
    library: PathBuf,
    nas: PathBuf,
}

/// A library of one account with five photos, as Lightroom keeps one:
/// - `kept.dng` on the NAS and in the library, edited, rated 4 and picked;
/// - `local.dng` only in the library;
/// - `cloud.dng` nowhere on this computer;
/// - `twin.dng`, two files of its name and size on the NAS, one its original;
/// - `odd.dng`, two of its name and size and neither its original;
/// - `wrap.dng` twice, two photos of one name and size (numbered again after
///   9999), one file on the NAS the first's original;
/// - `same.dng` twice, without SHA-256, one file for both;
///
/// and an album of `kept` and `cloud` in a folder of albums.
fn library() -> Result<Library> {
    let dir = tempfile::tempdir()?;
    let root = dir.path().canonicalize()?;
    let library = root.join("Lightroom Library.lrlibrary");
    let account = library.join("b67b");
    let originals = account.join("originals/2025/2025-08-14");
    let nas = root.join("NAS");
    for d in [
        &originals,
        &nas.join("2025/a"),
        &nas.join("2025/b"),
        &account.join("settings"),
    ] {
        std::fs::create_dir_all(d)?;
    }
    let kept = b"synthetic raw kept".as_slice();
    std::fs::write(nas.join("2025/a/kept.dng"), kept)?;
    std::fs::write(originals.join("kept.dng"), kept)?;
    std::fs::write(originals.join("local.dng"), b"synthetic raw local")?;
    std::fs::write(nas.join("2025/a/twin.dng"), b"synthetic raw twin A")?;
    std::fs::write(nas.join("2025/b/twin.dng"), b"synthetic raw twin B")?;
    std::fs::write(nas.join("2025/a/odd.dng"), b"synthetic raw odd 1")?;
    std::fs::write(nas.join("2025/b/odd.dng"), b"synthetic raw odd 2")?;
    let wrap = b"synthetic raw wrap 1".as_slice();
    std::fs::write(nas.join("2025/b/wrap.dng"), wrap)?;
    std::fs::write(nas.join("2025/b/same.dng"), b"synthetic raw same")?;
    // The second photo of a day and name, as Lightroom keeps it.
    std::fs::write(originals.join("pair-2.dng"), b"synthetic raw pair two")?;
    // Lightroom's preview of `cloud`, and one it names outside its folder.
    std::fs::create_dir_all(account.join("previews/ab"))?;
    let preview = account.join("previews/ab/cloud640");
    let jpeg = {
        let mut out = std::io::Cursor::new(Vec::new());
        image::RgbImage::from_pixel(4, 3, image::Rgb([200, 30, 30]))
            .write_to(&mut out, image::ImageFormat::Jpeg)?;
        out.into_inner()
    };
    std::fs::write(&preview, &jpeg)?;
    std::fs::write(account.join("previews/ab/cloud320"), &jpeg)?;
    std::fs::write(root.join("elsewhere.jpg"), &jpeg)?;
    let edit_sha = sha256_hex(EDIT.as_bytes());
    std::fs::write(account.join("settings").join(&edit_sha), EDIT)?;

    let image = |name: &'static str, size: usize, sha: String, extra: Vec<(&'static str, V)>| {
        let mut entries = vec![
            ("type", V::S("asset")),
            ("subtype", V::S("image")),
            ("captureDate", V::S("2025-08-14T17:48:44.08+01:00")),
            (
                "importSource",
                V::M(vec![
                    ("fileName", V::S(name)),
                    ("fileSize", V::I(size as i64)),
                    ("sha256", V::Owned(sha)),
                ]),
            ),
        ];
        entries.extend(extra);
        V::M(entries)
    };
    let docs = vec![
        (
            "kept",
            "asset",
            "image",
            image(
                "kept.dng",
                kept.len(),
                sha256_hex(kept),
                vec![
                    (
                        "develop",
                        V::M(vec![(
                            "xmpCameraRaw",
                            V::M(vec![("sha256", V::Owned(edit_sha))]),
                        )]),
                    ),
                    (
                        "ratings",
                        V::M(vec![(
                            "u",
                            V::M(vec![
                                ("date", V::S("2025-01-01T00:00:00Z")),
                                ("rating", V::I(4)),
                            ]),
                        )]),
                    ),
                    (
                        "reviews",
                        V::M(vec![(
                            "u",
                            V::M(vec![
                                ("date", V::S("2025-01-01T00:00:00Z")),
                                ("flag", V::S("pick")),
                            ]),
                        )]),
                    ),
                ],
            ),
        ),
        (
            "local",
            "asset",
            "image",
            image(
                "local.dng",
                b"synthetic raw local".len(),
                sha256_hex(b"synthetic raw local"),
                vec![],
            ),
        ),
        (
            "cloud",
            "asset",
            "image",
            image("cloud.dng", 1234, String::new(), vec![]),
        ),
        (
            "twin",
            "asset",
            "image",
            image(
                "twin.dng",
                b"synthetic raw twin B".len(),
                sha256_hex(b"synthetic raw twin B"),
                vec![],
            ),
        ),
        (
            "odd",
            "asset",
            "image",
            image(
                "odd.dng",
                b"synthetic raw odd 1".len(),
                sha256_hex(b"neither"),
                vec![],
            ),
        ),
        (
            "wrap1",
            "asset",
            "image",
            image(
                "wrap.dng",
                wrap.len(),
                sha256_hex(wrap),
                vec![(
                    "ratings",
                    V::M(vec![(
                        "u",
                        V::M(vec![
                            ("date", V::S("2025-01-01T00:00:00Z")),
                            ("rating", V::I(2)),
                        ]),
                    )]),
                )],
            ),
        ),
        (
            "wrap2",
            "asset",
            "image",
            image(
                "wrap.dng",
                wrap.len(),
                sha256_hex(b"synthetic raw wrap 2"),
                vec![],
            ),
        ),
        (
            "same1",
            "asset",
            "image",
            image(
                "same.dng",
                b"synthetic raw same".len(),
                String::new(),
                vec![],
            ),
        ),
        (
            "same2",
            "asset",
            "image",
            image(
                "same.dng",
                b"synthetic raw same".len(),
                String::new(),
                vec![],
            ),
        ),
        (
            "pair1",
            "asset",
            "image",
            image("pair.dng", 20, String::new(), vec![]),
        ),
        (
            "pair2",
            "asset",
            "image",
            image(
                "pair.dng",
                b"synthetic raw pair two".len(),
                String::new(),
                vec![],
            ),
        ),
        // In the cloud, its edit too: Lightroom downloads it with the original.
        (
            "later",
            "asset",
            "image",
            image(
                "later.dng",
                30,
                String::new(),
                vec![(
                    "develop",
                    V::M(vec![(
                        "xmpCameraRaw",
                        V::M(vec![(
                            "sha256",
                            V::Owned(sha256_hex(LATER_EDIT.as_bytes())),
                        )]),
                    )]),
                )],
            ),
        ),
        // Three of a day and name, two of one size.
        (
            "eq1",
            "asset",
            "image",
            image("eq.dng", 8, String::new(), vec![]),
        ),
        (
            "eq2",
            "asset",
            "image",
            image("eq.dng", 9, String::new(), vec![]),
        ),
        (
            "eq3",
            "asset",
            "image",
            image("eq.dng", 8, String::new(), vec![]),
        ),
        // Three photos of a day and name only in the cloud.
        (
            "tri1",
            "asset",
            "image",
            image("tri.dng", 5, String::new(), vec![]),
        ),
        (
            "tri2",
            "asset",
            "image",
            image("tri.dng", 6, String::new(), vec![]),
        ),
        (
            "tri3",
            "asset",
            "image",
            image("tri.dng", 7, String::new(), vec![]),
        ),
        // Two photos of a day and name only in the cloud.
        (
            "dup1",
            "asset",
            "image",
            image("dup.dng", 10, String::new(), vec![]),
        ),
        (
            "dup2",
            "asset",
            "image",
            image("dup.dng", 12, String::new(), vec![]),
        ),
        (
            "set",
            "album",
            "collection_set",
            V::M(vec![("name", V::S("2. Travel"))]),
        ),
        (
            "album",
            "album",
            "collection",
            V::M(vec![("name", V::S("16. August 2025")), id("parent", "set")]),
        ),
        (
            "m1",
            "album_asset",
            "",
            V::M(vec![id("album", "album"), id("asset", "kept")]),
        ),
        (
            "m2",
            "album_asset",
            "",
            V::M(vec![id("album", "album"), id("asset", "cloud")]),
        ),
    ];
    let db = rusqlite::Connection::open(account.join(crate::lr_library::CATALOG))?;
    // As Lightroom keeps it: WAL, which every connection holds a lock in.
    db.query_row("PRAGMA journal_mode=WAL", [], |_| Ok(()))?;
    db.execute_batch(
        "CREATE TABLE schemaVersion(currentVersion, backwardCompatibleToVersion);
         INSERT INTO schemaVersion VALUES('1.2','1.2');
         CREATE TABLE labels(localDocId INTEGER, revSequence INTEGER, name STRING,
            UNIQUE(localDocId, name));
         CREATE TABLE docs(localDocId INTEGER PRIMARY KEY, fullDocId TEXT UNIQUE NOT NULL,
            winningRevSequence INTEGER, mostRecentRevSequence INTEGER, deleted BOOLEAN DEFAULT 0,
            hasConflicts BOOLEAN DEFAULT 0, annotation BLOB, spaceId TEXT, type TEXT, subtype TEXT);
         CREATE TABLE revs(sequence INTEGER PRIMARY KEY AUTOINCREMENT, localDocId INTEGER NOT NULL,
            revId TEXT NOT NULL, parentRevSequence INTEGER, current BOOLEAN, deleted BOOLEAN DEFAULT 0,
            content BLOB);",
    )?;
    let mut cloud_local = 0;
    for (n, (doc, kind, subtype, content)) in docs.iter().enumerate() {
        let mut bytes = Vec::new();
        encode(content, &mut bytes);
        let n = n as i64 + 1;
        db.execute(
            "INSERT INTO docs(localDocId,fullDocId,winningRevSequence,mostRecentRevSequence,type,subtype)
             VALUES(?,?,?,?,?,?)",
            rusqlite::params![n, doc, n, n, kind, subtype],
        )?;
        // Lightroom stores the documents as TEXT that isn't UTF-8.
        db.execute(
            "INSERT INTO revs(sequence,localDocId,revId,current,content) VALUES(?,?,'1-a',1,CAST(? AS TEXT))",
            rusqlite::params![n, n, bytes],
        )?;
        db.execute(
            "INSERT INTO labels VALUES(?,?,'oz')",
            rusqlite::params![n, n],
        )?;
        if *doc == "cloud" {
            cloud_local = n;
        }
    }
    // A deleted photo is no photo of the library.
    db.execute(
        "INSERT INTO docs(fullDocId,type,subtype,deleted) VALUES('gone','asset','image',1)",
        [],
    )?;
    drop(db);
    let previews = rusqlite::Connection::open(account.join("previews.db"))?;
    previews.execute_batch(
        "CREATE TABLE RenditionPath(localAssetId NOT NULL, longSide NOT NULL, path NOT NULL);",
    )?;
    // Lightroom keeps both numbers as REAL.
    for (side, path) in [
        (640.0, preview),
        (320.0, account.join("previews/ab/cloud320")),
        (2560.0, root.join("elsewhere.jpg")),
    ] {
        previews.execute(
            "INSERT INTO RenditionPath VALUES(?,?,?)",
            rusqlite::params![cloud_local as f64, side, path.to_string_lossy()],
        )?;
    }
    // A preview named outside the library's previews is never taken.
    previews.execute(
        "INSERT INTO RenditionPath VALUES(?,?,?)",
        rusqlite::params![1.0, 640.0, root.join("elsewhere.jpg").to_string_lossy()],
    )?;
    drop(previews);
    Ok(Library {
        _dir: dir,
        root,
        library,
        nas,
    })
}

fn photo<'a>(
    photos: &'a [crate::catalog::models::Photo],
    path: &Path,
) -> &'a crate::catalog::models::Photo {
    photos
        .iter()
        .find(|p| p.path == path)
        .unwrap_or_else(|| panic!("{} imported", path.display()))
}

fn cloud_place(l: &Library, name: &str) -> PathBuf {
    l.library.join("b67b/originals/2025/2025-08-14").join(name)
}

#[test]
fn a_library_imports_every_photo_with_its_edit_rating_and_albums() -> Result<()> {
    let l = library()?;
    let destination = l.root.join("Lightroom.rawmakase");
    let report = import_lightroom_library(
        &l.library,
        std::slice::from_ref(&l.nas),
        &destination,
        &Default::default(),
    )?;
    assert_eq!(
        (
            report.photos,
            report.added,
            report.cloud,
            report.ambiguous,
            report.left_out
        ),
        (20, 5, 15, 2, 0),
        "{report:?}"
    );
    assert_eq!(
        (
            report.edits,
            report.edits_missing,
            report.albums,
            report.unreadable
        ),
        (2, 1, 2, 0)
    );
    assert_eq!(
        report.cloud_bytes,
        1234 + 19 + 20 + 2 * 18 + 20 + 10 + 12 + 30 + 5 + 6 + 7 + 8 + 9 + 8
    );

    let catalog = Catalog::open(&destination)?;
    let photos = catalog.photos()?;
    assert_eq!(photos.len(), 20);
    // Found on the NAS and in the library: the NAS's copy, where it is kept.
    let kept = photo(&photos, &l.nas.join("2025/a/kept.dng"));
    assert_eq!((kept.rating, kept.flag), (4, 1));
    let edit = catalog.edit_record(kept.id)?;
    assert_eq!(edit.lightroom.as_deref(), Some(EDIT));
    let local = photo(&photos, &cloud_place(&l, "local.dng"));
    assert_eq!((local.rating, local.flag), (0, 0));
    assert!(catalog.edit_record(local.id)?.lightroom.is_none());
    // Of two files of its name and size, the one with its SHA-256.
    photo(&photos, &l.nas.join("2025/b/twin.dng"));
    // Of two photos of one name and size, the one whose SHA-256 the file has.
    assert_eq!(photo(&photos, &l.nas.join("2025/b/wrap.dng")).rating, 2);
    // The second of a day and name, by the name Lightroom gave its file.
    let pair2 = photo(&photos, &cloud_place(&l, "pair-2.dng"));

    // The others wait where Lightroom puts them, in the cloud until then;
    // one file for two photos without SHA-256 is neither's.
    let cloud = catalog.lightroom_cloud_photos()?;
    let waiting: Vec<&str> = {
        let mut names: Vec<&str> = photos
            .iter()
            .filter(|p| cloud.contains_key(&p.id))
            .map(|p| p.filename.as_str())
            .collect();
        names.sort_unstable();
        names
    };
    assert_eq!(
        waiting,
        [
            "cloud.dng",
            "dup-2.dng",
            "dup.dng",
            "eq-2.dng",
            "eq-3.dng",
            "eq.dng",
            "later.dng",
            "odd.dng",
            "pair.dng",
            "same-2.dng",
            "same.dng",
            "tri-2.dng",
            "tri-3.dng",
            "tri.dng",
            "wrap.dng"
        ]
    );
    let in_cloud = photo(&photos, &cloud_place(&l, "cloud.dng"));
    assert_eq!(in_cloud.captured, "2025-08-14T17:48:44");
    assert!(!cloud.contains_key(&pair2.id));
    // Its preview: Lightroom's largest up to 2048 pixels in its previews,
    // never one named elsewhere.
    assert_eq!(
        cloud[&in_cloud.id],
        crate::catalog::lightroom::CloudPhoto {
            size: 1234,
            preview: Some(l.library.join("b67b/previews/ab/cloud640").canonicalize()?),
        }
    );
    assert!(cloud.values().filter(|c| c.preview.is_some()).count() == 1);

    let collections = catalog.collections()?;
    let set = collections.iter().find(|c| c.name == "2. Travel").unwrap();
    let album = collections
        .iter()
        .find(|c| c.name == "16. August 2025")
        .unwrap();
    assert_eq!(
        (set.kind, album.kind),
        (CollectionKind::Set, CollectionKind::Collection)
    );
    assert_eq!(album.parent, Some(set.id));
    let members = catalog.collection_photos()?;
    assert_eq!(members[&album.id], [kept.id, in_cloud.id].into());
    Ok(())
}

/// Run by the tests below as Lightroom: holds the SQLite file the environment
/// names open, idle, until its input closes. Does nothing as an ordinary
/// test.
#[test]
fn holds_a_library_open_for_another_test() {
    use std::io::{BufRead as _, Write as _};
    let Some(db) = std::env::var_os("RAWMAKASE_HOLD_LIBRARY") else {
        return;
    };
    let db = rusqlite::Connection::open(db).unwrap();
    db.query_row("SELECT count(*) FROM docs", [], |_| Ok(()))
        .unwrap();
    println!("open");
    std::io::stdout().flush().unwrap();
    for _ in std::io::stdin().lock().lines() {}
}

/// Another process with the catalog at `db` open, as Lightroom running is,
/// until it is dropped.
struct Lightroom(
    std::process::Child,
    std::io::BufReader<std::process::ChildStdout>,
);
impl Lightroom {
    fn open(db: &Path) -> Lightroom {
        use std::io::{BufRead as _, BufReader};
        use std::process::{Command, Stdio};
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "catalog::lightroom::library_tests::holds_a_library_open_for_another_test",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("RAWMAKASE_HOLD_LIBRARY", db)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut out = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        while !line.contains("open") {
            line.clear();
            assert_ne!(out.read_line(&mut line).unwrap(), 0, "it ended");
        }
        Lightroom(child, out)
    }
}
impl Drop for Lightroom {
    fn drop(&mut self) {
        use std::io::Read as _;
        drop(self.0.stdin.take());
        // Read to its end, so what it writes as it ends has somewhere to go.
        let _ = self.1.read_to_end(&mut Vec::new());
        let _ = self.0.wait();
    }
}

fn catalog_of(l: &Library) -> PathBuf {
    l.library.join("b67b").join(crate::lr_library::CATALOG)
}

#[test]
fn a_library_open_in_lightroom_or_not_a_library_imports_nothing() -> Result<()> {
    let l = library()?;
    let destination = l.root.join("Lightroom.rawmakase");
    {
        let _lightroom = Lightroom::open(&catalog_of(&l));
        let error = import_lightroom_library(&l.library, &[], &destination, &Default::default())
            .unwrap_err();
        assert!(format!("{error:#}").contains("Quit Lightroom"), "{error:#}");
        assert!(!destination.exists());
    }

    assert!(import_lightroom_library(&l.nas, &[], &destination, &Default::default()).is_err());
    assert!(!destination.exists());
    // Only the library's own originals without folders to look in.
    let report = import_lightroom_library(&l.library, &[], &destination, &Default::default())?;
    assert_eq!((report.added, report.cloud), (3, 17), "{report:?}");
    // A folder given twice is looked in once: its photos aren't taken for
    // copies of themselves.
    let twice = l.root.join("Twice.rawmakase");
    let report = import_lightroom_library(
        &l.library,
        &[l.nas.clone(), l.nas.clone()],
        &twice,
        &Default::default(),
    )?;
    assert_eq!((report.added, report.ambiguous), (5, 2), "{report:?}");
    Ok(())
}

#[test]
fn what_a_closed_lightroom_left_in_its_wal_is_imported_and_the_library_kept() -> Result<()> {
    let l = library()?;
    let db = catalog_of(&l);
    // An album Lightroom added and left in its WAL when it quit.
    let wal = PathBuf::from(format!("{}-wal", db.display()));
    let saved = l.root.join("saved");
    {
        let lr = rusqlite::Connection::open(&db)?;
        lr.query_row("PRAGMA wal_autocheckpoint=0", [], |_| Ok(()))?;
        std::fs::copy(&db, &saved)?;
        let mut bytes = Vec::new();
        encode(&V::M(vec![("name", V::S("Late"))]), &mut bytes);
        lr.execute(
            "INSERT INTO docs(localDocId,fullDocId,winningRevSequence,type,subtype)
             VALUES(900,'late',900,'album','collection')",
            [],
        )?;
        lr.execute(
            "INSERT INTO revs(sequence,localDocId,revId,current,content)
             VALUES(900,900,'1-a',1,CAST(? AS TEXT))",
            [bytes],
        )?;
        std::fs::copy(&wal, l.root.join("saved-wal"))?;
    }
    // Closing checkpointed it: put back the file and the WAL it had.
    std::fs::copy(&saved, &db)?;
    std::fs::copy(l.root.join("saved-wal"), &wal)?;
    let before: Vec<Vec<u8>> = [&db, &wal]
        .iter()
        .map(std::fs::read)
        .collect::<std::io::Result<_>>()?;

    let destination = l.root.join("Lightroom.rawmakase");
    let report = import_lightroom_library(&l.library, &[], &destination, &Default::default())?;
    assert_eq!(report.albums, 3, "{report:?}");
    let after: Vec<Vec<u8>> = [&db, &wal]
        .iter()
        .map(std::fs::read)
        .collect::<std::io::Result<_>>()?;
    assert!(before == after, "the library is only read");
    Ok(())
}

#[test]
fn a_photo_in_the_cloud_is_here_once_its_original_has_its_size() -> Result<()> {
    let l = library()?;
    let destination = l.root.join("Lightroom.rawmakase");
    import_lightroom_library(&l.library, &[], &destination, &Default::default())?;
    let mut catalog = Catalog::open(&destination)?;
    let waiting = catalog.lightroom_cloud_photos()?.len();
    let by_path = |c: &Catalog, name: &str| photo(&c.photos().unwrap(), &cloud_place(&l, name)).id;
    let cloud = by_path(&catalog, "cloud.dng");

    // Still downloading: a file shorter than the original.
    std::fs::write(cloud_place(&l, "cloud.dng"), vec![1; 100])?;
    let settled = catalog.settle_lightroom_downloads()?;
    assert_eq!((settled.arrived.len(), settled.waiting), (0, waiting));
    std::fs::write(cloud_place(&l, "cloud.dng"), vec![1; 1234])?;
    let settled = catalog.settle_lightroom_downloads()?;
    assert_eq!(settled.arrived, [cloud]);
    assert!(!catalog.lightroom_cloud_photos()?.contains_key(&cloud));

    // Two photos of a day and name downloaded the other way round: each
    // takes the file of its size, the second's name Lightroom gave the first.
    let (dup1, dup2) = (by_path(&catalog, "dup.dng"), by_path(&catalog, "dup-2.dng"));
    std::fs::write(cloud_place(&l, "dup.dng"), vec![2; 12])?;
    std::fs::write(cloud_place(&l, "dup-2.dng"), vec![1; 10])?;
    let mut settled = catalog.settle_lightroom_downloads()?;
    settled.arrived.sort_unstable();
    let mut both = vec![dup1, dup2];
    both.sort_unstable();
    assert_eq!(settled.arrived, both);
    assert_eq!(
        (by_path(&catalog, "dup-2.dng"), by_path(&catalog, "dup.dng")),
        (dup1, dup2)
    );
    assert_eq!(settled.waiting, waiting - 3);

    // Two of one size can't be told apart by it: a file of that size isn't
    // given to either until their originals can be, and none is taken.
    let eq = ["eq.dng", "eq-2.dng", "eq-3.dng"].map(|n| by_path(&catalog, n));
    std::fs::write(cloud_place(&l, "eq.dng"), vec![2; 9])?;
    std::fs::write(cloud_place(&l, "eq-2.dng"), vec![3; 8])?;
    let settled = catalog.settle_lightroom_downloads()?;
    assert!(settled.arrived.is_empty(), "{settled:?}");
    assert_eq!(
        ["eq.dng", "eq-2.dng", "eq-3.dng"].map(|n| by_path(&catalog, n)),
        eq
    );

    // Three downloaded in another order: each takes the file of its size.
    let tri = ["tri.dng", "tri-2.dng", "tri-3.dng"].map(|n| by_path(&catalog, n));
    std::fs::write(cloud_place(&l, "tri.dng"), vec![3; 7])?;
    std::fs::write(cloud_place(&l, "tri-2.dng"), vec![1; 5])?;
    // One of them not there yet: nothing moves.
    let settled = catalog.settle_lightroom_downloads()?;
    assert!(settled.arrived.is_empty(), "{settled:?}");
    std::fs::write(cloud_place(&l, "tri-3.dng"), vec![2; 6])?;
    let mut settled = catalog.settle_lightroom_downloads()?;
    settled.arrived.sort_unstable();
    let mut all = tri.to_vec();
    all.sort_unstable();
    assert_eq!(settled.arrived, all);
    assert_eq!(
        ["tri.dng", "tri-2.dng", "tri-3.dng"].map(|n| by_path(&catalog, n)),
        [tri[2], tri[0], tri[1]]
    );

    // Its edit downloaded with it: read, once.
    let later = by_path(&catalog, "later.dng");
    assert!(catalog.edit_record(later)?.lightroom.is_none());
    std::fs::write(
        l.library
            .join("b67b/settings")
            .join(sha256_hex(LATER_EDIT.as_bytes())),
        LATER_EDIT,
    )?;
    assert_eq!(catalog.settle_lightroom_downloads()?.edits, [later]);
    assert_eq!(
        catalog.edit_record(later)?.lightroom.as_deref(),
        Some(LATER_EDIT)
    );
    assert!(catalog.settle_lightroom_downloads()?.edits.is_empty());
    Ok(())
}

#[test]
fn an_album_marked_for_download_is_marked_as_lightroom_marks_it() -> Result<()> {
    use crate::catalog::lightroom::mark_albums_for_download;
    let l = library()?;
    let destination = l.root.join("Lightroom.rawmakase");
    import_lightroom_library(&l.library, &[], &destination, &Default::default())?;
    let mut catalog = Catalog::open(&destination)?;
    let albums = catalog.lightroom_library_albums()?;
    let album = albums.iter().find(|a| a.id == "album").unwrap();
    // `kept` is here, `cloud` in the cloud.
    assert_eq!(
        (
            album.photos,
            album.in_cloud,
            album.in_cloud_bytes,
            album.marked
        ),
        (2, 1, 1234, false)
    );
    assert_eq!(album.parent.as_deref(), Some("set"));
    assert!(albums.iter().any(|a| a.id == "set" && a.set));

    let db = catalog_of(&l);
    let backups = l.root.join("backups");
    {
        let _lightroom = Lightroom::open(&db);
        let error = mark_albums_for_download(std::slice::from_ref(album), &backups).unwrap_err();
        assert!(format!("{error:#}").contains("Quit Lightroom"), "{error:#}");
        assert!(catalog.lightroom_library_albums().is_err(), "nor read");
    }
    let docs_before = docs(&db)?;
    let marked = mark_albums_for_download(std::slice::from_ref(album), &backups)?;
    assert_eq!((marked.albums, marked.photos, marked.unsettled), (1, 2, 0));
    // The catalog as it was, backed up.
    let backup = std::fs::read_dir(&backups)?.next().unwrap()?.path();
    assert_eq!(docs(&backup)?, docs_before);

    let after = docs(&db)?;
    for (doc, marked) in [
        ("album", true),
        ("kept", true),
        ("cloud", true),
        ("local", false),
    ] {
        let (before, now) = (&docs_before[doc], &after[doc]);
        assert_eq!(
            crate::lr_library::marked_for_download(now.annotation.as_deref().unwrap_or(&[])),
            marked,
            "{doc}"
        );
        if !marked {
            assert_eq!(before, now, "{doc} untouched");
            continue;
        }
        // A new current revision, a copy of the one it was; that one no
        // longer current, without its content; the label moved.
        assert_ne!(now.winning, before.winning);
        assert_eq!(
            now.revisions[0],
            (
                now.winning,
                "1-a".into(),
                Some(before.winning),
                true,
                before.revisions[0].4.clone()
            )
        );
        assert_eq!(
            now.revisions[1],
            (before.winning, "1-a".into(), None, false, None)
        );
        assert_eq!(now.labels, [(now.winning, "oz".to_string())]);
    }
    assert!(
        catalog
            .lightroom_library_albums()?
            .iter()
            .any(|a| a.id == "album" && a.marked)
    );
    // Marked once: again changes nothing.
    let again = mark_albums_for_download(std::slice::from_ref(album), &backups)?;
    assert_eq!((again.albums, again.photos), (1, 0));
    assert_eq!(docs(&db)?, after);
    Ok(())
}

/// A revision: sequence, revId, parent, current, content.
type Revision = (i64, String, Option<i64>, bool, Option<Vec<u8>>);
/// A document's row: its row id, id, winning revision and annotation.
type DocRowOf = (i64, String, Option<i64>, Option<Vec<u8>>);

/// A document of a library's catalog as a test compares it.
#[derive(Debug, PartialEq)]
struct Doc {
    winning: i64,
    annotation: Option<Vec<u8>>,
    /// Newest first.
    revisions: Vec<Revision>,
    labels: Vec<(i64, String)>,
}

fn docs(db: &Path) -> Result<std::collections::HashMap<String, Doc>> {
    use rusqlite::types::ValueRef;
    let bytes = |v: ValueRef<'_>| match v {
        ValueRef::Text(b) | ValueRef::Blob(b) => Some(b.to_vec()),
        _ => None,
    };
    let db = rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut out = std::collections::HashMap::new();
    let mut each =
        db.prepare("SELECT localDocId, fullDocId, winningRevSequence, annotation FROM docs")?;
    let rows: Vec<DocRowOf> = each
        .query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, bytes(r.get_ref(3)?)))
        })?
        .collect::<rusqlite::Result<_>>()?;
    for (local, id, winning, annotation) in rows {
        let revisions = db
            .prepare(
                "SELECT sequence, revId, parentRevSequence, current, content FROM revs
                 WHERE localDocId=? ORDER BY sequence DESC",
            )?
            .query_map([local], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    bytes(r.get_ref(4)?),
                ))
            })?
            .collect::<rusqlite::Result<_>>()?;
        let labels = db
            .prepare("SELECT revSequence, name FROM labels WHERE localDocId=?")?
            .query_map([local], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        out.insert(
            id,
            Doc {
                winning: winning.unwrap_or(0),
                annotation,
                revisions,
                labels,
            },
        );
    }
    Ok(out)
}

#[test]
fn photos_in_no_album_are_downloaded_through_an_album_made_as_lightroom_makes_one() -> Result<()> {
    use crate::catalog::lightroom::{NEW_ALBUM, mark_albums_for_download};
    let l = library()?;
    let destination = l.root.join("Lightroom.rawmakase");
    import_lightroom_library(&l.library, &[], &destination, &Default::default())?;
    let mut catalog = Catalog::open(&destination)?;
    let albums = catalog.lightroom_library_albums()?;
    let loose = albums.iter().find(|a| a.loose).expect("photos in no album");
    // Every photo but the album's two, of which `cloud` is in the cloud.
    let in_cloud = catalog.lightroom_cloud_photos()?.len();
    assert_eq!(loose.in_cloud, in_cloud - 1, "{loose:?}");
    assert!(loose.photos > loose.in_cloud, "local ones counted too");

    let db = catalog_of(&l);
    let before = docs(&db)?;
    let marked = mark_albums_for_download(std::slice::from_ref(loose), &l.root.join("backups"))?;
    let (album, photos) = marked.new_album.expect("an album made");
    assert_eq!(photos, loose.in_cloud);
    let after = docs(&db)?;
    let made: Vec<&String> = after
        .keys()
        .filter(|id| !before.contains_key(*id))
        .collect();
    assert_eq!(
        made.len(),
        1 + photos,
        "the album and one document per photo"
    );

    // As Lightroom makes them on this computer: one current revision, no
    // parent, no label until Lightroom has uploaded it, content as TEXT.
    let raw =
        rusqlite::Connection::open_with_flags(&db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    for id in &made {
        let doc = &after[*id];
        assert_eq!(doc.revisions.len(), 1, "{id}");
        let (sequence, rev, parent, current, content) = &doc.revisions[0];
        assert_eq!((*sequence, *parent, *current), (doc.winning, None, true));
        assert_eq!(rev.len(), 32, "a 128-bit id in hex");
        assert!(content.is_some() && doc.labels.is_empty(), "{id}");
        let (kind, storage, newest): (String, String, i64) = raw.query_row(
            "SELECT type, typeof(annotation), mostRecentRevSequence FROM docs WHERE fullDocId=?",
            [id.as_str()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        assert_eq!((storage.as_str(), newest), ("text", doc.winning));
        match kind.as_str() {
            "album" => {
                assert_eq!(id.as_str(), album);
                assert!(crate::lr_library::marked_for_download(
                    doc.annotation.as_deref().unwrap()
                ));
                let Ok(Some(crate::lr_library::Document::Album(a))) = crate::lr_library::document(
                    "album",
                    "collection",
                    id,
                    content.as_deref().unwrap(),
                ) else {
                    panic!("an album")
                };
                assert_eq!((a.name.as_str(), a.parent), (NEW_ALBUM, None));
            }
            "album_asset" => {
                assert_eq!(doc.annotation.as_deref(), Some(&[0x90][..]));
                let Ok(Some(crate::lr_library::Document::AlbumAsset(m))) =
                    crate::lr_library::document("album_asset", "", id, content.as_deref().unwrap())
                else {
                    panic!("a photo of the album")
                };
                assert_eq!(m.album, album);
                // The photo is marked to download, as an album's are.
                assert!(crate::lr_library::marked_for_download(
                    after[&m.asset].annotation.as_deref().unwrap()
                ));
            }
            other => panic!("{other}"),
        }
    }
    // Closed first: on Windows a connection of this process's own counts as
    // the library being open, as Lightroom's does.
    raw.close().map_err(|(_, e)| e)?;
    // Listed again: an album like any other, marked; nothing in the cloud
    // left in no album.
    let albums = catalog.lightroom_library_albums()?;
    let new = albums.iter().find(|a| a.id == album).unwrap();
    assert_eq!((new.marked, new.in_cloud, new.loose), (true, photos, false));
    assert!(albums.iter().filter(|a| a.loose).all(|a| a.in_cloud == 0));

    // Downloading photos in no album again adds them to that album rather
    // than making another of its name; one Lightroom hasn't settled is put in
    // it but counted apart.
    let lr = rusqlite::Connection::open(&db)?;
    lr.execute("UPDATE docs SET hasConflicts=1 WHERE fullDocId='local'", [])?;
    drop(lr);
    let mut again = loose.clone();
    again.in_cloud_assets = vec![("local".into(), 19)];
    let marked = mark_albums_for_download(&[again], &l.root.join("backups"))?;
    assert_eq!(
        (marked.new_album, marked.unsettled),
        (Some((album.clone(), 0)), 1)
    );
    let later = docs(&db)?;
    let added: Vec<&String> = later.keys().filter(|id| !after.contains_key(*id)).collect();
    assert_eq!(added.len(), 1, "one photo document, no second album");
    let raw =
        rusqlite::Connection::open_with_flags(&db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let named: i64 = raw.query_row(
        "SELECT count(*) FROM docs WHERE type='album' AND deleted=0",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(named, 3, "the two of the library and the one made");
    Ok(())
}
