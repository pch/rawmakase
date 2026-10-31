use super::*;

/// An original synthetic DCP: a forward matrix and the given identity, no
/// proprietary data.
fn dcp(camera: &str, name: &str, notice: &str) -> Vec<u8> {
    let text = |s: &str| format!("{s}\0").into_bytes();
    // An empty notice is left out: a TIFF value this short would be inline.
    let entries: Vec<_> = [
        (50708u16, 2u16, text(camera)),
        (50936, 2, text(name)),
        (50942, 2, text(notice)),
        (
            50964,
            10,
            (0..9)
                .flat_map(|i| [i32::from(i % 4 == 0), 1])
                .flat_map(i32::to_le_bytes)
                .collect(),
        ),
    ]
    .into_iter()
    .filter(|(_, _, data)| data.len() > 1)
    .collect();
    let mut at = 8 + 2 + entries.len() * 12 + 4;
    let mut bytes = b"IIRC".to_vec();
    bytes.extend(8u32.to_le_bytes());
    bytes.extend((entries.len() as u16).to_le_bytes());
    for (tag, kind, data) in &entries {
        bytes.extend(tag.to_le_bytes());
        bytes.extend(kind.to_le_bytes());
        bytes.extend((if *kind == 10 { 9 } else { data.len() as u32 }).to_le_bytes());
        bytes.extend((at as u32).to_le_bytes());
        at += data.len();
    }
    bytes.extend(0u32.to_le_bytes());
    for (_, _, data) in entries {
        bytes.extend(data);
    }
    bytes
}

fn camera(name: &str) -> Metadata {
    // The decoders report a make and a model; profiles name both together.
    let (make, model) = name.split_once(' ').unwrap_or(("", name));
    Metadata {
        make: make.into(),
        model: model.into(),
        ..Default::default()
    }
}

fn sources() -> Vec<Source> {
    parse_sources(SOURCES).unwrap()
}

/// A catalogue of synthetic profiles from the shipped sources.
fn synthetic(found: Vec<Found>) -> (Generated, Catalogue) {
    let generated = generate(&sources(), found).unwrap();
    let catalogue = Catalogue::parse(SOURCES, &generated.catalogue_toml()).unwrap();
    (generated, catalogue)
}

fn found(source: &str, file: &str, bytes: Vec<u8>) -> Found {
    Found {
        source: source.into(),
        file: file.into(),
        bytes,
    }
}

#[test]
fn the_shipped_catalogue_is_pinned_credited_and_dedicated_by_each_profiles_own_notice() {
    let c = catalogue();
    assert!(!c.entries().is_empty());
    let mut per_source = std::collections::BTreeMap::new();
    for e in c.entries() {
        let p = &e.profile;
        *per_source.entry(e.source.id.as_str()).or_insert(0) += 1;
        assert!(
            e.url().starts_with("https://raw.githubusercontent.com/"),
            "{}",
            p.id
        );
        for url in [e.url(), e.page(), e.repository_license_url()] {
            assert!(url.contains(&format!("/{}/", e.source.revision)), "{url}");
            assert!(!url.contains(' '), "{url}");
        }
        assert_eq!(Terms::of_notice(&p.notice), Some(p.terms), "{}", p.id);
    }
    assert_eq!(
        per_source.keys().copied().collect::<Vec<_>>(),
        ["art", "rawtherapee"]
    );
    // RawTherapee's X-T20 carries only "Copyright Maciej Dworak": no terms. ART's
    // own X-T20 profile states it is public domain.
    let x_t20 = c.matching(&camera("Fujifilm X-T20"));
    assert_eq!(x_t20.len(), 1);
    assert_eq!(x_t20[0].source.id, "art");
    assert_eq!(x_t20[0].profile.terms, Terms::PublicDomain);
    assert!(
        c.entries().iter().all(|e| e.profile.sha256
            != "17ed74368790c197d32563252985451375831930d9282f34c9602a2a51fc3656")
    );
}

#[test]
fn each_profile_matches_its_own_camera_and_never_another_body() {
    let entries = catalogue().entries();
    let key = |s: &str| {
        s.chars()
            .filter(|c| !c.is_whitespace() && *c != '_')
            .flat_map(char::to_lowercase)
            .collect::<String>()
    };
    for e in entries {
        let own = camera(&e.profile.camera);
        assert!(e.fits(&own), "{} for its own camera", e.profile.id);
        // As the decoders spell makes ("Fujifilm", "Nikon"), not as DCPs do.
        let decoded = Metadata {
            make: {
                let mut make = own.make.to_ascii_lowercase();
                if let Some(first) = make.get_mut(..1) {
                    first.make_ascii_uppercase();
                }
                make
            },
            ..own.clone()
        };
        assert!(e.fits(&decoded), "{} for decoded names", e.profile.id);
        for other in entries {
            if key(&other.profile.camera) != key(&e.profile.camera) {
                assert!(
                    !other.fits(&own),
                    "{} must not be offered for {}",
                    other.profile.id,
                    e.profile.camera
                );
            }
        }
        let rendered = Metadata {
            source: crate::camera_data::Source::Rendered,
            ..own
        };
        assert!(!e.fits(&rendered), "{} for a rendered file", e.profile.id);
    }
    for unknown in [
        "Fujifilm X-T5",
        "Fujifilm X-T200",
        "Nikon Z 30",
        "Sony ILCE-7M5",
        "",
    ] {
        assert!(
            catalogue().matching(&camera(unknown)).is_empty(),
            "{unknown}"
        );
    }
}

#[test]
fn a_body_with_several_sources_installs_each_under_its_own_name() {
    let mut cameras: std::collections::BTreeMap<String, Vec<&Entry>> = Default::default();
    for e in catalogue().entries() {
        cameras
            .entry(e.profile.camera.to_ascii_lowercase())
            .or_default()
            .push(e);
    }
    let shared: Vec<_> = cameras.values().filter(|v| v.len() > 1).collect();
    assert!(
        !shared.is_empty(),
        "the snapshots have bodies in both sources"
    );
    for entries in shared {
        let m = camera(&entries[0].profile.camera);
        assert_eq!(catalogue().matching(&m).len(), entries.len());
        let mut names: Vec<_> = entries
            .iter()
            .map(|e| e.profile.filename.to_ascii_lowercase())
            .collect();
        names.dedup();
        assert_eq!(names.len(), entries.len());
    }
}

#[test]
fn unsafe_unpinned_or_unsupported_catalogue_entries_are_refused() {
    let (generated, _) = synthetic(vec![found(
        "art",
        "Test X1.dcp",
        dcp("Test X1", "Test X1", "public domain"),
    )]);
    let valid = generated.catalogue_toml();
    assert!(Catalogue::parse(SOURCES, &valid).is_ok());
    let line = |key: &str| {
        valid
            .lines()
            .find(|l| l.starts_with(&format!("{key} = ")))
            .unwrap()
            .to_string()
    };
    for (key, bad) in [
        ("file", r#"file = "../Test X1.dcp""#),
        ("file", r#"file = "Test X1.xmp""#),
        ("filename", r#"filename = "sub/Test X1.dcp""#),
        ("filename", r#"filename = "C:Test X1.dcp""#),
        ("filename", r#"filename = ".Test X1.dcp""#),
        ("id", r#"id = "rawtherapee/test-x1""#),
        ("id", r#"id = "art/Test X1""#),
        ("source", r#"source = "elsewhere""#),
        ("terms", r#"terms = "CC0-1.0""#),
        ("notice", r#"notice = "Copyright Someone""#),
        ("size", "size = 0"),
        ("size", "size = 16000001"),
        ("sha256", r#"sha256 = "abc""#),
        ("camera", r#"camera = " ""#),
    ] {
        let text = valid.replace(&line(key), bad);
        assert!(Catalogue::parse(SOURCES, &text).is_err(), "{bad}");
    }
    assert!(Catalogue::parse(SOURCES, &format!("{valid}licence = \"MIT\"\n")).is_err());
    let twice = format!("{valid}{}", &valid[valid.find("\n[[profile]]").unwrap()..]);
    assert!(Catalogue::parse(SOURCES, &twice).is_err());
    let same_name = twice.replacen("art/test-x1", "art/other", 1).replacen(
        &line("sha256"),
        &format!("sha256 = \"{}\"", "0".repeat(64)),
        1,
    );
    assert!(
        Catalogue::parse(SOURCES, &same_name).is_err(),
        "case-insensitive file names"
    );
    for (from, to) in [
        ("b2ee504c47e5e6d5e054395aa12bf1fe43bd1c3c", "dev"),
        (
            "https://github.com/RawTherapee/RawTherapee",
            "http://github.com/RawTherapee/RawTherapee",
        ),
        (
            "https://github.com/RawTherapee/RawTherapee",
            "https://example.com/RawTherapee/RawTherapee",
        ),
        ("rtdata/dcpprofiles", "rtdata/../dcpprofiles"),
        ("id = \"art\"", "id = \"rawtherapee\""),
    ] {
        let sources = SOURCES.replace(from, to);
        assert_ne!(sources, SOURCES);
        assert!(Catalogue::parse(&sources, &valid).is_err(), "{to}");
    }
}

#[test]
fn generation_is_deterministic_and_says_why_each_file_is_left_out() {
    let shared = dcp("Test X2", "Test X2", "RawTherapee CC0");
    let files = || {
        vec![
            found(
                "art",
                "Test X1.dcp",
                dcp("Test X1", "Test X1", "public domain"),
            ),
            found("rawtherapee", "Test X2.dcp", shared.clone()),
            found("art", "Test X2.dcp", shared.clone()),
            found(
                "rawtherapee",
                "Test X1.dcp",
                dcp("Test X1", "Test X1 Alt", "RawTherapee CC0"),
            ),
            found(
                "rawtherapee",
                "Test X3.dcp",
                dcp("Test X3", "Test X3", "Copyright Someone"),
            ),
            found("rawtherapee", "Test X4.dcp", dcp("Test X4", "Test X4", "")),
            found("rawtherapee", "Broken.dcp", b"IIRC".to_vec()),
        ]
    };
    let (generated, catalogue) = synthetic(files());
    let mut reversed = files();
    reversed.reverse();
    assert_eq!(generate(&sources(), reversed).unwrap(), generated);
    let ids: Vec<_> = generated.profiles.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(
        ids,
        ["rawtherapee/test-x1", "rawtherapee/test-x2", "art/test-x1"]
    );
    let filenames: Vec<_> = generated
        .profiles
        .iter()
        .map(|p| p.filename.as_str())
        .collect();
    assert_eq!(
        filenames,
        [
            "Test X1 (RawTherapee).dcp",
            "Test X2.dcp",
            "Test X1 (ART).dcp"
        ]
    );
    let reasons: Vec<_> = generated
        .excluded
        .iter()
        .map(|e| (e.source.as_str(), e.file.as_str(), e.reason.as_str()))
        .collect();
    assert_eq!(reasons.len(), 4);
    assert!(reasons[0].1 == "Broken.dcp" && reasons[0].2.starts_with("not supported"));
    assert!(reasons[1].2.contains("grants no terms beyond copyright"));
    assert!(reasons[2].2.starts_with("no copyright notice"));
    assert_eq!(
        reasons[3],
        ("art", "Test X2.dcp", "the same file as rawtherapee/test-x2")
    );
    assert_eq!(
        parse_excluded(&generated.excluded_toml()).unwrap(),
        generated.excluded
    );
    assert_eq!(catalogue.matching(&camera("Test X1")).len(), 2);
    assert_eq!(catalogue.entries()[1].profile.terms, Terms::Cc0);
}

fn fixture() -> (&'static Entry, Vec<u8>) {
    let bytes = dcp("Fujifilm X-T20", "Test profile", "public domain");
    let (_, catalogue) = synthetic(vec![found("art", "Test.dcp", bytes.clone())]);
    let catalogue = Box::leak(Box::new(catalogue));
    (&catalogue.entries()[0], bytes)
}

fn fuji() -> Metadata {
    camera("FUJIFILM X-T20")
}

#[test]
fn verified_install_is_persistent_idempotent_and_never_overwrites_a_conflict() {
    let (e, bytes) = fixture();
    let dir = tempfile::tempdir().unwrap();
    let destination = dir.path().join("library");
    assert_eq!(e.status_in(&[], &destination), Status::Available);
    let target = e.install_into(&bytes, &fuji(), &destination).unwrap();
    assert_eq!(target, destination.join("Test.dcp"));
    assert_eq!(
        e.status_in(&[], &destination),
        Status::Installed(target.clone())
    );
    assert_eq!(
        e.install_into(&bytes, &fuji(), &destination).unwrap(),
        target
    );
    assert_eq!(
        library::listed_from(std::slice::from_ref(&destination), &fuji())
            .profiles
            .iter()
            .filter(|p| p.name == "Test profile")
            .count(),
        1
    );
    std::fs::write(&target, b"my existing profile").unwrap();
    assert_eq!(e.status_in(&[], &destination), Status::Conflict);
    assert!(e.install_into(&bytes, &fuji(), &destination).is_err());
    assert_eq!(std::fs::read(target).unwrap(), b"my existing profile");
    // The exact file elsewhere in the library still counts as installed.
    let other = dir.path().join("other");
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(other.join("Test.dcp"), &bytes).unwrap();
    assert_eq!(
        e.status_in(std::slice::from_ref(&other), &destination),
        Status::Installed(other.join("Test.dcp"))
    );
}

#[test]
fn a_profile_imported_by_hand_under_its_source_name_counts_as_installed() {
    let bytes = dcp("Test X1", "Test X1", "public domain");
    let (_, catalogue) = synthetic(vec![
        found("rawtherapee", "Test X1.dcp", bytes.clone()),
        found(
            "art",
            "Test X1.dcp",
            dcp("Test X1", "Other", "public domain"),
        ),
    ]);
    let e = &catalogue.entries()[0];
    assert_eq!(e.profile.filename, "Test X1 (RawTherapee).dcp");
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("Test X1.dcp"), &bytes).unwrap();
    assert_eq!(
        e.status_in(&[], dir.path()),
        Status::Installed(dir.path().join("Test X1.dcp"))
    );
    // The other source's same-named file is a different profile.
    assert_eq!(
        catalogue.entries()[1].status_in(&[], dir.path()),
        Status::Available
    );
}

#[test]
fn an_installed_file_is_used_only_while_it_holds_the_reviewed_bytes() {
    let (e, bytes) = fixture();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Test.dcp");
    std::fs::write(&path, &bytes).unwrap();
    let read = e.read_installed(&path, &fuji()).unwrap();
    assert_eq!(read, from_bytes(&bytes).unwrap());
    // A valid profile with the same name and camera in its place is another file.
    let other = dcp("Fujifilm X-T20", "Test profile", "public domain")
        .into_iter()
        .chain(*b"pad")
        .collect::<Vec<_>>();
    assert!(from_bytes(&other).is_ok());
    std::fs::write(&path, &other).unwrap();
    assert!(e.read_installed(&path, &fuji()).is_err());
    std::fs::remove_file(&path).unwrap();
    assert!(e.read_installed(&path, &fuji()).is_err());
}

#[test]
fn bad_bytes_and_camera_mismatches_never_create_a_library() {
    let (e, bytes) = fixture();
    let dir = tempfile::tempdir().unwrap();
    let destination = dir.path().join("library");
    let mut corrupt = bytes.clone();
    *corrupt.last_mut().unwrap() ^= 1;
    for invalid in [&b"short"[..], &vec![0; bytes.len()], &corrupt] {
        assert!(e.install_into(invalid, &fuji(), &destination).is_err());
    }
    for other in ["Fujifilm X-T2", "Fujifilm X-T200"] {
        assert!(
            e.install_into(&bytes, &camera(other), &destination)
                .is_err()
        );
    }
    let rendered = Metadata {
        source: crate::camera_data::Source::Rendered,
        ..fuji()
    };
    assert!(e.install_into(&bytes, &rendered, &destination).is_err());
    assert!(!destination.exists());
}

/// Regenerates the catalogue from local checkouts of the pinned revisions and
/// installs each profile into a temporary library: the parser, identities,
/// notices, digests, and that every file is either listed or excluded. No
/// network: `RAWMAKASE_PROFILE_SOURCES=rawtherapee=<dir>,art=<dir>`, each the
/// source's DCP directory at its pinned revision (docs/lightroom-profiles.md).
#[test]
#[ignore = "Needs checkouts of the pinned sources; set RAWMAKASE_PROFILE_SOURCES"]
fn pinned_checkouts_regenerate_the_shipped_catalogue_and_install() {
    let dirs = std::env::var("RAWMAKASE_PROFILE_SOURCES").unwrap();
    let mut files = Vec::new();
    for pair in dirs.split(',') {
        let (source, dir) = pair.split_once('=').unwrap();
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("dcp"))
            {
                files.push(found(
                    source,
                    &path.file_name().unwrap().to_string_lossy(),
                    std::fs::read(&path).unwrap(),
                ));
            }
        }
    }
    let generated = generate(&sources(), files).unwrap();
    assert_eq!(generated.catalogue_toml(), CATALOGUE);
    assert_eq!(
        generated.excluded_toml(),
        include_str!("../../../../data/camera-profiles/excluded.toml")
    );
    let destination = tempfile::tempdir().unwrap();
    for e in catalogue().entries() {
        let m = camera(&e.profile.camera);
        let dir = dirs
            .split(',')
            .find_map(|p| p.strip_prefix(&format!("{}=", e.source.id)))
            .unwrap();
        let bytes = std::fs::read(Path::new(dir).join(&e.profile.file)).unwrap();
        let installed = e
            .install_into(&bytes, &m, destination.path())
            .unwrap_or_else(|why| panic!("{}: {why:#}", e.profile.id));
        assert_eq!(
            e.status_in(&[], destination.path()),
            Status::Installed(installed)
        );
    }
}
