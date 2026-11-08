use super::msgpack::{Value, decode};
use super::*;

/// MessagePack for `v`, in the widest forms, so the decoder's long forms are
/// what the documents below exercise.
fn encode(v: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    match v {
        Value::Nil => out.push(0xc0),
        Value::Bool(b) => out.push(if *b { 0xc3 } else { 0xc2 }),
        Value::Int(n) => {
            out.push(0xd3);
            out.extend(n.to_be_bytes());
        }
        Value::Float(f) => {
            out.push(0xcb);
            out.extend(f.to_bits().to_be_bytes());
        }
        Value::Str(s) => {
            out.push(0xdb);
            out.extend((s.len() as u32).to_be_bytes());
            out.extend(s.as_bytes());
        }
        Value::Bin(b) => {
            out.push(0xc6);
            out.extend((b.len() as u32).to_be_bytes());
            out.extend(b);
        }
        Value::Array(items) => {
            out.push(0xdd);
            out.extend((items.len() as u32).to_be_bytes());
            items.iter().for_each(|i| out.extend(encode(i)));
        }
        Value::Map(entries) => {
            out.push(0xdf);
            out.extend((entries.len() as u32).to_be_bytes());
            for (k, v) in entries {
                out.extend(encode(k));
                out.extend(encode(v));
            }
        }
        Value::Ext(kind, b) => {
            out.push(0xc9);
            out.extend((b.len() as u32).to_be_bytes());
            out.push(*kind as u8);
            out.extend(b);
        }
    }
    out
}

fn s(text: &str) -> Value {
    Value::Str(text.into())
}
fn map(entries: Vec<(&str, Value)>) -> Value {
    Value::Map(entries.into_iter().map(|(k, v)| (s(k), v)).collect())
}

const SHA: &str = "9650c1e9f33b791cff2d040ab98410857702f7173b5e0c8ffbc1c81ea0aefe87";
const EDIT: &str = "8dfc3bd70a64f874c40845219c03072080407fd9e78fe83778a644268aa05fee";

/// An image asset as Lightroom keeps one, trimmed to what an import reads
/// plus a field it doesn't.
fn image() -> Value {
    map(vec![
        ("type", s("asset")),
        ("subtype", s("image")),
        ("captureDate", s("2023-09-08T14:03:11")),
        (
            "develop",
            map(vec![
                ("croppedWidth", Value::Int(5464)),
                (
                    "xmpCameraRaw",
                    map(vec![("sha256", s(&EDIT.to_uppercase()))]),
                ),
            ]),
        ),
        (
            "importSource",
            map(vec![
                ("fileName", s("407A5334.CR3")),
                ("fileSize", Value::Int(11_256_918)),
                ("sha256", s(SHA)),
            ]),
        ),
        (
            "ratings",
            map(vec![
                (
                    "older",
                    map(vec![
                        ("date", s("2024-01-01T19:38:56Z")),
                        ("rating", Value::Int(2)),
                    ]),
                ),
                (
                    "newer",
                    map(vec![
                        ("date", s("2025-08-17T08:57:26Z")),
                        ("rating", Value::Int(4)),
                    ]),
                ),
            ]),
        ),
        (
            "reviews",
            map(vec![(
                "user",
                map(vec![
                    ("date", s("2025-07-30T18:47:20Z")),
                    ("flag", s("reject")),
                ]),
            )]),
        ),
        ("aesthetics", map(vec![("score", Value::Float(0.42))])),
    ])
}

#[test]
fn an_image_asset_gives_its_original_rating_review_and_edit() {
    let doc = document("asset", "image", "4a8b", &encode(&image())).unwrap();
    assert_eq!(
        doc,
        Some(Document::Asset(Asset {
            id: "4a8b".into(),
            file_name: "407A5334.CR3".into(),
            file_size: 11_256_918,
            sha256: SHA.into(),
            rating: 4,
            flag: -1,
            edit: Some(EDIT.into()),
            capture_date: "2023-09-08T14:03:11".into(),
        }))
    );
}

#[test]
fn missing_or_odd_fields_leave_their_part_out() {
    let odd = map(vec![
        (
            "importSource",
            map(vec![
                ("fileName", Value::Int(3)),
                ("sha256", s("../../etc")),
            ]),
        ),
        (
            "develop",
            map(vec![(
                "xmpCameraRaw",
                map(vec![("sha256", s("../settings"))]),
            )]),
        ),
        ("ratings", s("five")),
    ]);
    let Some(Document::Asset(asset)) = document("asset", "image", "x", &encode(&odd)).unwrap()
    else {
        panic!("still an asset");
    };
    assert_eq!(
        asset,
        Asset {
            id: "x".into(),
            ..Default::default()
        }
    );
}

#[test]
fn albums_and_their_photos_are_read_and_other_documents_passed_over() {
    let album = map(vec![
        ("name", s("16. August 2025")),
        ("parent", map(vec![("id", s("set1"))])),
    ]);
    assert_eq!(
        document("album", "collection", "a1", &encode(&album)).unwrap(),
        Some(Document::Album(Album {
            id: "a1".into(),
            name: "16. August 2025".into(),
            parent: Some("set1".into()),
            set: false,
        }))
    );
    let set = map(vec![("name", s("2. Travel")), ("parent", Value::Nil)]);
    let Some(Document::Album(set)) =
        document("album", "collection_set", "set1", &encode(&set)).unwrap()
    else {
        panic!("an album folder");
    };
    assert!(set.set && set.parent.is_none());
    let member = map(vec![
        ("album", map(vec![("id", s("a1"))])),
        ("asset", map(vec![("id", s("4a8b"))])),
    ]);
    assert_eq!(
        document("album_asset", "", "m", &encode(&member)).unwrap(),
        Some(Document::AlbumAsset(AlbumAsset {
            album: "a1".into(),
            asset: "4a8b".into(),
        }))
    );
    // Not read at all, so not even MessagePack is asked of them.
    assert_eq!(document("asset", "preset", "p", b"\xc1").unwrap(), None);
    assert_eq!(document("space", "shared", "q", b"").unwrap(), None);
    assert!(document("asset", "image", "z", b"\xc1").is_err());
    // A document far larger than any Lightroom writes isn't decoded.
    assert!(document("asset", "image", "big", &vec![0xc0; 17 << 20]).is_err());
}

#[test]
fn short_forms_decode_as_long_ones() {
    // {"a": [1, -1, 200, -200, 70000, "x", nil, true], "b": 1.5f32}
    let bytes = [
        0x82, 0xa1, b'a', 0x98, 0x01, 0xff, 0xcc, 200, 0xd1, 0xff, 0x38, 0xce, 0, 1, 0x11, 0x70,
        0xa1, b'x', 0xc0, 0xc3, 0xa1, b'b', 0xca, 0x3f, 0xc0, 0, 0,
    ];
    let v = decode(&bytes).unwrap();
    let Some(Value::Array(a)) = v.get("a") else {
        panic!("{v:?}")
    };
    assert_eq!(
        a,
        &[
            Value::Int(1),
            Value::Int(-1),
            Value::Int(200),
            Value::Int(-200),
            Value::Int(70_000),
            s("x"),
            Value::Nil,
            Value::Bool(true),
        ]
    );
    assert_eq!(v.get("b"), Some(&Value::Float(1.5)));
    assert_eq!(decode(&encode(&image())).unwrap(), image());
}

#[test]
fn damaged_documents_are_errors_not_panics() {
    for bytes in [
        &b""[..],
        b"\xdb\xff\xff\xff\xff",
        b"\xdd\xff\xff\xff\xff",
        b"\xdf\x00\x00\x00\x03\xa1a",
        b"\xc6\x00\x00\x00\x09ab",
        b"\x01\x02",
    ] {
        assert!(decode(bytes).is_err(), "{bytes:?}");
    }
    let deep: Vec<u8> = std::iter::repeat_n(0x91, 100).chain([0xc0]).collect();
    assert!(decode(&deep).is_err());
}

#[test]
fn an_edit_is_found_only_by_a_sha256() {
    let account = Path::new("/library/b67b");
    assert_eq!(
        edit_path(account, EDIT),
        Some(PathBuf::from("/library/b67b/settings").join(EDIT))
    );
    assert_eq!(edit_path(account, "../../secret"), None);
}

#[test]
fn accounts_are_the_folders_with_a_catalog() {
    let library = tempfile::tempdir().unwrap();
    for account in ["b67b", "0a1c"] {
        std::fs::create_dir_all(library.path().join(account)).unwrap();
        std::fs::write(library.path().join(account).join(CATALOG), "").unwrap();
    }
    std::fs::create_dir_all(library.path().join("FolderSourcePreviews")).unwrap();
    assert_eq!(
        accounts(library.path()),
        [library.path().join("0a1c"), library.path().join("b67b")]
    );
}

#[test]
fn an_original_is_kept_by_its_capture_day() {
    let account = Path::new("/lib/acc");
    let asset = |date: &str, name: &str| Asset {
        capture_date: date.into(),
        file_name: name.into(),
        ..Asset::default()
    };
    assert_eq!(
        original_path(account, &asset("2023-09-08T14:03:11", "407A1969.CR3")),
        Some(account.join("originals/2023/2023-09-08/407A1969.CR3"))
    );
    // No day, or a name that would leave `originals/`: nowhere.
    for (date, name) in [
        ("", "a.CR3"),
        ("0000-00-00T00:00:00", "a.CR3"),
        ("2023-9-8", "a.CR3"),
        ("2023-09-08", ""),
        ("2023-09-08", ".."),
        ("2023-09-08", "../a.CR3"),
        ("2023-09-08", "a\\b.CR3"),
    ] {
        assert_eq!(
            original_path(account, &asset(date, name)),
            None,
            "{date} {name}"
        );
    }
}

/// An annotation as Lightroom keeps one for a photo: what it has of it here,
/// and what the server said, in Lightroom's short forms.
fn annotation(local: Option<Vec<(&str, Value)>>) -> Vec<u8> {
    let mut entries = vec![(
        "_serverProvidedMetadata",
        map(vec![("created", s("2023-04-21T08:31:01Z"))]),
    )];
    if let Some(local) = local {
        entries.push(("_localOnly", map(local)));
    }
    short(&map(entries))
}

/// MessagePack for `v` in the shortest forms, as Lightroom writes it.
fn short(v: &Value) -> Vec<u8> {
    match v {
        Value::Str(t) if t.len() < 32 => [&[0xa0 | t.len() as u8][..], t.as_bytes()].concat(),
        Value::Map(entries) if entries.len() < 16 => {
            let mut out = vec![0x80 | entries.len() as u8];
            for (k, v) in entries {
                out.extend(short(k));
                out.extend(short(v));
            }
            out
        }
        other => encode(other),
    }
}

#[test]
fn marking_a_document_sets_one_flag_and_keeps_every_other_byte() {
    let files = map(vec![("aux", map(vec![("x", s("auxiliary/x"))]))]);
    let local = vec![("files", files.clone())];
    let before = annotation(Some(local.clone()));
    assert!(!marked_for_download(&before));
    let after = mark_for_download(Some(&before)).unwrap().unwrap();
    assert!(marked_for_download(&after));
    let mut marked = local.clone();
    marked.push(("isMarkedForOfflineUse", Value::Bool(true)));
    assert_eq!(after, annotation(Some(marked)), "only the flag is added");
    assert_eq!(
        mark_for_download(Some(&after)).unwrap(),
        None,
        "marked once"
    );

    // A flag that is false is replaced, and moved to the end.
    let unmarked = annotation(Some(vec![
        ("isMarkedForOfflineUse", Value::Bool(false)),
        ("files", files.clone()),
    ]));
    let after = mark_for_download(Some(&unmarked)).unwrap().unwrap();
    assert_eq!(
        after,
        annotation(Some(vec![
            ("files", files),
            ("isMarkedForOfflineUse", Value::Bool(true)),
        ]))
    );
}

#[test]
fn a_document_without_local_data_gains_it() {
    let after = mark_for_download(Some(&annotation(None))).unwrap().unwrap();
    assert_eq!(
        after,
        annotation(Some(vec![("isMarkedForOfflineUse", Value::Bool(true))]))
    );
    // None at all: a map of its own.
    let fresh = mark_for_download(None).unwrap().unwrap();
    assert_eq!(
        decode(&fresh).unwrap(),
        map(vec![(
            "_localOnly",
            map(vec![("isMarkedForOfflineUse", Value::Bool(true))])
        )])
    );
    // A long form stays readable: a map of 15 entries outgrows its header.
    let wide: Vec<(String, Value)> = (0..15).map(|i| (format!("k{i}"), Value::Int(i))).collect();
    let wide = short(&Value::Map(
        wide.into_iter().map(|(k, v)| (s(&k), v)).collect(),
    ));
    let after = mark_for_download(Some(&wide)).unwrap().unwrap();
    assert!(marked_for_download(&after));
    assert_eq!(decode(&after).unwrap().entries().unwrap().len(), 16);
}

#[test]
fn a_damaged_annotation_is_refused_not_rewritten() {
    for bad in [&[0x93, 1, 2, 3][..], &[0x81, 0xa1], &[0x80, 0x00], &[0xc1]] {
        assert!(mark_for_download(Some(bad)).is_err(), "{bad:?}");
        assert!(!marked_for_download(bad));
    }
    // `_localOnly` that isn't a map.
    let odd = short(&map(vec![("_localOnly", s("x"))]));
    assert!(mark_for_download(Some(&odd)).is_err());
}

#[test]
fn a_new_album_is_written_byte_for_byte_as_lightroom_writes_one() {
    // The bytes of an album and one of its photos Lightroom made on this
    // computer, before they synced, with made-up ids.
    let now = "2026-11-08T10:22:05.786Z";
    let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02X}")).collect::<String>();
    let s = |t: &str| hex(t.as_bytes());
    assert_eq!(
        hex(&new_album("RAWmakase Test", now)),
        format!(
            "85AB{}B8{}A4{}A5{}A4{}AE{}A7{}AA{}AB{}B8{}",
            s("userUpdated"),
            s(now),
            s("type"),
            s("album"),
            s("name"),
            s("RAWmakase Test"),
            s("subtype"),
            s("collection"),
            s("userCreated"),
            s(now)
        )
    );
    let album = "a7060b76f0474b318de64fc08d99b57e";
    let asset = "c332ad5fb1cb462bb258119bc315d968";
    assert_eq!(
        hex(&new_album_asset(album, asset, now)),
        format!(
            "85A5{}81A2{}D920{}A4{}AB{}AB{}B8{}A5{}81A2{}D920{}AB{}B8{}",
            s("album"),
            s("id"),
            s(album),
            s("type"),
            s("album_asset"),
            s("userUpdated"),
            s(now),
            s("asset"),
            s("id"),
            s(asset),
            s("userCreated"),
            s(now)
        )
    );
    let annotation = new_album_annotation(now);
    assert!(marked_for_download(&annotation));
    assert_eq!(
        hex(&annotation),
        format!(
            "81AA{}82AF{}B8{}B5{}C3",
            s("_localOnly"),
            s("updateTimeStamp"),
            s(now),
            s("isMarkedForOfflineUse")
        )
    );
    assert_eq!(
        decode(NEW_ALBUM_ASSET_ANNOTATION).unwrap(),
        Value::Array(vec![])
    );
    // A name too long for a short string takes a longer form, read back as it was.
    let long = "A".repeat(40);
    let doc = document("album", "collection", "x", &new_album(&long, now)).unwrap();
    assert!(matches!(doc, Some(Document::Album(a)) if a.name == long));
}
