use super::*;
use crate::metadata::{Capture, LangAlt, Location, Value};

/// A camera's EXIF with a tag of every group.
fn camera() -> CameraExif {
    CameraExif {
        main: vec![
            Field::ascii(0x010f, "Make"),
            Field::ascii(0x0110, "Model"),
            Field::ascii(CAPTION, "Camera caption"),
            Field::ascii(ARTIST, "Camera artist"),
            Field::ascii(COPYRIGHT, "Camera copyright"),
        ],
        exif: vec![
            Field::rational(0x829a, 1, 250),
            Field::ascii(0x9003, "2020:01:02 03:04:05"),
            Field::ascii(0x9291, "50"),
            Field::ascii(0x9011, "+01:00"),
            Field::ascii(0xa434, "Lens"),
        ],
        gps: vec![Field::ascii(0x0001, "N")],
    }
}
fn settings(include: Include) -> ExportSettings {
    ExportSettings {
        include,
        ..Default::default()
    }
}
fn text(a: &Assembled, tag: u16) -> Option<String> {
    a.exif.get(tag).and_then(Field::text)
}
fn values(rating: i32, keywords: &[&str]) -> Values {
    Values {
        rating,
        label: "Red".into(),
        keywords: keywords
            .iter()
            .map(|k| KeywordPath::all(vec![k.to_string()]))
            .collect(),
        ..Default::default()
    }
}

/// What releases before the Include popup wrote, from the four switches.
fn legacy(s: &ExportSettings, file: &CameraExif, v: &Values) -> Assembled {
    let mut exif = CameraExif::default();
    if s.capture {
        exif = file.clone();
        if !s.location {
            exif.gps.clear();
        }
    }
    exif.main.sort_by_key(|f| f.tag);
    exif.exif.sort_by_key(|f| f.tag);
    let xmp = (s.develop || s.descriptive).then(|| XmpFields {
        captured: s.capture.then(|| file.captured()).flatten(),
        created: None,
        rating: if s.descriptive { v.rating } else { 0 },
        label: if s.descriptive {
            v.label.clone()
        } else {
            String::new()
        },
        keywords: if s.descriptive {
            v.keywords.clone()
        } else {
            Vec::new()
        },
        lens: true,
        develop: s.develop,
        ..Default::default()
    });
    Assembled { exif, xmp }
}

#[test]
fn custom_keeps_every_combination_of_the_old_switches_exactly() {
    let file = camera();
    let v = values(3, &["Smith, John", "City"]);
    for bits in 0..16 {
        let s = ExportSettings {
            capture: bits & 1 != 0,
            location: bits & 2 != 0,
            develop: bits & 4 != 0,
            descriptive: bits & 8 != 0,
            ..Default::default()
        };
        assert_eq!(s.include, Include::Custom);
        let policy = Policy::of(&s);
        let read = policy.reads_file().then_some(&file);
        assert_eq!(read.is_some(), s.capture, "{bits:04b}");
        let new = assemble(policy, read, None, &v);
        assert_eq!(new, legacy(&s, &file, &v), "{bits:04b}");
    }
}

#[test]
fn custom_writes_catalog_fields_with_descriptive_and_exif_only_with_capture() {
    let file = camera();
    let mut v = values(0, &[]);
    v.descriptive.copyright = Some(Value::Set(LangAlt::new("© Catalog")));
    v.descriptive.creator = Some(Value::Set(vec!["A".into(), "B".into()]));
    v.descriptive.title = Some(Value::Set(LangAlt::new("Title")));
    let both = ExportSettings::default();
    let a = assemble(Policy::of(&both), Some(&file), None, &v);
    assert_eq!(text(&a, COPYRIGHT).as_deref(), Some("© Catalog"));
    assert_eq!(text(&a, ARTIST).as_deref(), Some("A; B"));
    // No catalog caption: the camera's own, as before.
    assert_eq!(text(&a, CAPTION).as_deref(), Some("Camera caption"));
    let xmp = a.xmp.unwrap();
    assert_eq!(xmp.rights, LangAlt::new("© Catalog").0);
    assert_eq!(xmp.creators, ["A", "B"]);
    assert_eq!(xmp.title, LangAlt::new("Title").0);
    assert!(xmp.caption.is_empty());
    // Descriptive off: the camera's own still go with the camera info.
    let off = ExportSettings {
        descriptive: false,
        ..Default::default()
    };
    let a = assemble(Policy::of(&off), Some(&file), None, &v);
    assert_eq!(text(&a, COPYRIGHT).as_deref(), Some("Camera copyright"));
    assert!(a.xmp.unwrap().rights.is_empty());
    // Capture off: XMP only.
    let xmp_only = ExportSettings {
        capture: false,
        ..Default::default()
    };
    let a = assemble(Policy::of(&xmp_only), None, None, &v);
    assert!(a.exif.main.is_empty());
    assert_eq!(a.xmp.unwrap().rights, LangAlt::new("© Catalog").0);
}

#[test]
fn a_cleared_field_strips_its_tag_in_every_mode() {
    let file = camera();
    let mut v = values(0, &[]);
    v.descriptive.copyright = Some(Value::Cleared);
    v.descriptive.caption = Some(Value::Cleared);
    v.descriptive.creator = Some(Value::Cleared);
    for include in Include::ALL {
        for descriptive in [false, true] {
            let s = ExportSettings {
                include,
                descriptive,
                ..Default::default()
            };
            let a = assemble(Policy::of(&s), Some(&file), None, &v);
            for tag in [CAPTION, ARTIST, COPYRIGHT] {
                assert_eq!(text(&a, tag), None, "{include:?} {tag:#x}");
            }
            if let Some(xmp) = a.xmp {
                assert!(xmp.rights.is_empty() && xmp.caption.is_empty());
                assert!(xmp.creators.is_empty());
            }
        }
    }
}

#[test]
fn each_lightroom_mode_includes_exactly_its_groups() {
    let file = camera();
    let mut v = values(4, &["City"]);
    v.descriptive.title = Some(Value::Set(LangAlt::new("Title")));
    let has = |a: &Assembled, tag| a.exif.get(tag).is_some();
    // Mode, then copyright, contact, descriptive, capture time, camera, GPS
    // and develop settings.
    for (include, groups) in [
        (Include::CopyrightOnly, [1, 0, 0, 0, 0, 0, 0]),
        (Include::CopyrightAndContact, [1, 1, 0, 0, 0, 0, 0]),
        (Include::AllExceptCameraAndCameraRaw, [1, 1, 1, 1, 0, 1, 0]),
        (Include::AllExceptCameraRaw, [1, 1, 1, 1, 1, 1, 0]),
        (Include::All, [1, 1, 1, 1, 1, 1, 1]),
    ] {
        let [c, contact, d, time, cam, gps, dev] = groups.map(|g| g == 1);
        let a = assemble(Policy::of(&settings(include)), Some(&file), None, &v);
        assert_eq!(has(&a, COPYRIGHT), c, "{include:?}");
        assert_eq!(has(&a, ARTIST), contact, "{include:?}");
        assert_eq!(has(&a, CAPTION), d, "{include:?}");
        assert_eq!(has(&a, 0x9003) && has(&a, 0x9291), time, "{include:?}");
        assert_eq!(
            has(&a, 0x010f) && has(&a, 0x829a) && has(&a, 0xa434),
            cam,
            "{include:?}"
        );
        assert_eq!(!a.exif.gps.is_empty(), gps, "{include:?}");
        let xmp = a.xmp.unwrap();
        assert_eq!(xmp.develop, dev, "{include:?}");
        assert_eq!(xmp.lens, cam, "{include:?}");
        assert_eq!(xmp.rating == 4 && !xmp.title.is_empty(), d, "{include:?}");
        assert_eq!(!xmp.keywords.is_empty(), d, "{include:?}");
        assert_eq!(xmp.captured.is_some(), time, "{include:?}");
        // The file's own copyright and creator fill fields the catalog lacks.
        assert_eq!(!xmp.rights.is_empty(), c, "{include:?}");
        assert_eq!(xmp.creators == ["Camera artist"], contact, "{include:?}");
        // Remove Location Info drops GPS everywhere.
        let s = ExportSettings {
            remove_location: true,
            ..settings(include)
        };
        assert!(
            assemble(Policy::of(&s), Some(&file), None, &v)
                .exif
                .gps
                .is_empty()
        );
    }
}

#[test]
fn a_capture_time_from_the_catalog_replaces_the_cameras() {
    let file = camera();
    for (subsec, offset) in [("12", None), ("120456", Some("+02:00"))] {
        let mut v = values(0, &[]);
        v.descriptive.capture = Some(Capture {
            captured: "2024-05-01T12:30:15".into(),
            subsec: Some(subsec.into()),
            offset: offset.map(String::from),
        });
        let a = assemble(Policy::of(&settings(Include::All)), Some(&file), None, &v);
        assert_eq!(text(&a, 0x9003).as_deref(), Some("2024:05:01 12:30:15"));
        assert_eq!(text(&a, 0x9291).as_deref(), Some(subsec));
        // Never the camera's offset with another time.
        assert_eq!(text(&a, 0x9011).as_deref(), offset);
        let created = a.xmp.unwrap().created.unwrap();
        assert_eq!(
            created,
            format!("2024-05-01T12:30:15.{subsec}{}", offset.unwrap_or(""))
        );
    }
    // Without one, the camera's pass through.
    let a = assemble(
        Policy::of(&settings(Include::All)),
        Some(&file),
        None,
        &values(0, &[]),
    );
    assert_eq!(text(&a, 0x9291).as_deref(), Some("50"));
    assert_eq!(text(&a, 0x9011).as_deref(), Some("+01:00"));
}

#[test]
fn a_catalog_location_replaces_or_clears_the_files() {
    let file = camera();
    let mut v = values(0, &[]);
    v.descriptive.location = Some(Location::At {
        lat: -33.8568,
        lon: 151.2153,
        alt: Some(-2.5),
    });
    let a = assemble(Policy::of(&settings(Include::All)), Some(&file), None, &v);
    let get = |tag| a.exif.gps.iter().find(|f| f.tag == tag).cloned();
    assert_eq!(get(0x0001).and_then(|f| f.text()).as_deref(), Some("S"));
    assert_eq!(get(0x0003).and_then(|f| f.text()).as_deref(), Some("E"));
    assert_eq!(get(0x0005).map(|f| f.bytes), Some(vec![1]));
    assert_eq!(get(0x0006).and_then(|f| f.number()), Some(2.5));
    let lat = get(0x0002).unwrap();
    let part = |i: usize| {
        let w = |at: usize| u32::from_le_bytes(lat.bytes[at..at + 4].try_into().unwrap()) as f64;
        w(i * 8) / w(i * 8 + 4)
    };
    let degrees = part(0) + part(1) / 60. + part(2) / 3600.;
    assert!((degrees - 33.8568).abs() < 1e-6);
    v.descriptive.location = Some(Location::Cleared);
    let a = assemble(Policy::of(&settings(Include::All)), Some(&file), None, &v);
    assert!(a.exif.gps.is_empty());
}

#[test]
fn keywords_export_with_their_parents_and_paths() {
    let paths: Vec<KeywordPath> = [
        vec!["Places".to_string(), "Poland".into(), "Kraków".into()],
        vec!["Places".to_string()],
        vec!["Smith, John".to_string()],
        vec!["AC/DC".to_string()],
        vec!["Flat|Name".to_string()],
        vec!["Flat|Name".to_string(), "Child".into()],
    ]
    .into_iter()
    .map(KeywordPath::all)
    .collect();
    let (subject, hierarchical) = crate::xmp::write::keyword_lists(&paths);
    assert_eq!(
        subject,
        [
            "Places",
            "Poland",
            "Kraków",
            "Smith, John",
            "AC/DC",
            "Flat|Name",
            "Child"
        ]
    );
    assert_eq!(
        hierarchical,
        ["Places|Poland|Kraków", "Places", "Smith, John", "AC/DC"]
    );
}

#[test]
fn unreadable_exif_falls_back_to_librarys_capture_settings_with_overrides() {
    let libraw = CameraExif {
        main: vec![Field::ascii(0x010f, "Make")],
        exif: vec![Field::rational(0x829a, 1, 250)],
        gps: Vec::new(),
    };
    let mut v = values(0, &[]);
    v.descriptive.copyright = Some(Value::Set(LangAlt::new("© Catalog")));
    let a = assemble(
        Policy::of(&ExportSettings::default()),
        None,
        Some(libraw),
        &v,
    );
    assert_eq!(text(&a, 0x010f).as_deref(), Some("Make"));
    assert!(a.exif.get(0x829a).is_some());
    assert_eq!(text(&a, COPYRIGHT).as_deref(), Some("© Catalog"));
}

#[test]
fn keywords_lightroom_keeps_out_of_exports_stay_out() {
    let keyword = |path: &[&str], exported: &[bool]| KeywordPath {
        path: path.iter().map(|n| n.to_string()).collect(),
        exported: exported.to_vec(),
    };
    let (subject, hierarchical) = crate::xmp::write::keyword_lists(&[
        // A private container: the child exports alone.
        keyword(&["Private", "Clients", "Acme"], &[false, true, true]),
        // Export Containing Keywords off.
        keyword(&["Places", "Home"], &[false, true]),
        // Include on Export off.
        keyword(&["Hidden"], &[false]),
    ]);
    assert_eq!(subject, ["Clients", "Acme", "Home"]);
    assert_eq!(hierarchical, ["Acme", "Home"]);
}

#[test]
fn text_too_long_for_exif_goes_in_the_xmp_only() {
    let mut v = values(0, &[]);
    let long = "x".repeat(crate::exif::MAX_VALUE + 1);
    v.descriptive.caption = Some(Value::Set(LangAlt::new(&long)));
    let a = assemble(
        Policy::of(&settings(Include::All)),
        Some(&camera()),
        None,
        &v,
    );
    assert_eq!(text(&a, CAPTION), None);
    assert_eq!(a.xmp.unwrap().caption, LangAlt::new(&long).0);
}

/// A catalog location read from the file's own EXIF exports as the file's
/// GPS directory whole, its time, direction and other tags kept; one
/// changed since writes the location alone.
#[test]
fn a_location_read_from_the_files_gps_keeps_its_other_tags() {
    let mut file = camera();
    file.gps = crate::exif::gps_fixture::coordinates(
        "N",
        [(40, 1), (42, 1), (5363, 100)],
        "W",
        [(73, 1), (59, 1), (5480, 100)],
    );
    // GPSImgDirection.
    file.gps
        .push(crate::exif::Field::rational(0x0011, 2705, 10));
    let mut v = values(0, &[]);
    v.descriptive.location = file.location();
    let a = assemble(Policy::of(&settings(Include::All)), Some(&file), None, &v);
    assert_eq!(a.exif.gps, file.gps);
    v.descriptive.location = Some(Location::At {
        lat: 40.7,
        lon: -74.,
        alt: None,
    });
    let a = assemble(Policy::of(&settings(Include::All)), Some(&file), None, &v);
    assert!(!a.exif.gps.iter().any(|f| f.tag == 0x0011));
}
