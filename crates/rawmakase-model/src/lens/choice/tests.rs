use super::*;
use crate::lens::lcp::{Library, test_profile};

pub const ADOBE: &str = "Testcam (35mm F2) - RAW.lcp";
pub const MINE: &str = "Mine 35mm F2.lcp";
pub const OTHER: &str = "Lensco (50mm F1.4) - RAW.lcp";

/// Imported profiles: two of the test lens (Adobe's and one the user made) and one
/// of another lens and make.
pub fn library() -> Library {
    let texts = [
        (
            ADOBE,
            test_profile("Testcam", "35mm F2", "Adobe (Testcam 35mm F2)", -0.05, -0.5),
        ),
        (
            MINE,
            test_profile("Testcam", "35mm F2", "Mine (Testcam 35mm F2)", 0., -0.2),
        ),
        (
            OTHER,
            test_profile(
                "Lensco",
                "50mm F1.4",
                "Adobe (Lensco 50mm F1.4)",
                0.02,
                -0.8,
            ),
        ),
    ];
    Library::from_texts(texts.iter().map(|(f, t)| (*f, t.as_str())))
}
/// A photo from a synthetic camera and lens, with the profiles imported for it.
pub fn photo() -> Metadata {
    let mut m = Metadata {
        make: "Testcam".into(),
        model: "T1".into(),
        lens_model: "35mm F2".into(),
        focal: 35.,
        aperture: 2.,
        width: 600,
        height: 400,
        ..Default::default()
    };
    m.lens_profiles = library().for_photo(&m);
    m
}
fn id(filename: &str) -> Option<LensProfileRef> {
    Some(LensProfileRef::Adobe(LensProfileId {
        filename: filename.into(),
        ..Default::default()
    }))
}
fn choice(setup: LensProfileSetup, id: Option<LensProfileRef>) -> LensProfileChoice {
    LensProfileChoice { setup, id }
}
fn used(c: &LensProfileChoice, m: &Metadata) -> Option<String> {
    c.resolve(&m.lens_profiles, m)
        .used
        .map(|c| c.profile.filename.clone())
}

#[test]
fn default_and_auto_match_the_lens() {
    let m = photo();
    assert_eq!(m.lens_profiles.all().len(), 3);
    for setup in [LensProfileSetup::Default, LensProfileSetup::Auto] {
        let c = choice(setup, None);
        assert_eq!(used(&c, &m).as_deref(), Some(ADOBE), "{setup:?}");
        assert!(c.resolve(&m.lens_profiles, &m).missing.is_none());
    }
    // No imported profile of the lens: nothing automatic, whatever else is imported.
    let mut other_lens = photo();
    other_lens.lens_model = "85mm F1.8".into();
    other_lens.lens_profiles = library().for_photo(&other_lens);
    assert_eq!(used(&LensProfileChoice::default(), &other_lens), None);
}

#[test]
fn canon_ef_mount_spacing_matches_without_confusing_lens_versions() {
    // #610: a 5D Mark II records a space after EF; Adobe's LCP omits it.
    // Synthetic coefficients: no Adobe profile data is needed for this test.
    let original = test_profile("Canon", "EF24-105mm f/4L IS USM", "Original", -0.05, -0.3);
    let second = test_profile("Canon", "EF24-105mm f/4L IS II USM", "Second", -0.02, -0.2);
    let library = Library::from_texts([
        ("original.lcp", original.as_str()),
        ("second.lcp", second.as_str()),
    ]);
    for (lens, expected) in [
        ("EF 24-105mm f/4L IS USM", Some("original.lcp")),
        ("EF24-105mm f/4L IS USM", Some("original.lcp")),
        ("EF 24-105mm f/4L IS II USM", Some("second.lcp")),
        ("EF 24-105mm f/4L IS I I USM", None),
        ("EF 24-105mm f/4L USM", None),
        ("RF 24-105mm f/4L IS USM", None),
    ] {
        let mut m = Metadata {
            make: "Canon".into(),
            model: "EOS 5D Mark II".into(),
            lens_model: lens.into(),
            focal: 24.,
            aperture: 11.,
            width: 5634,
            height: 3752,
            ..Default::default()
        };
        m.lens_profiles = library.for_photo(&m);
        for setup in [LensProfileSetup::Default, LensProfileSetup::Auto] {
            let c = choice(setup, None);
            assert_eq!(used(&c, &m).as_deref(), expected, "{lens}, {setup:?}");
            let r = crate::model::recipe::Recipe {
                lens_profile: true,
                lens_profile_choice: c,
                ..Default::default()
            };
            assert_eq!(r.missing_lens_profile(&m).is_none(), expected.is_some());
        }
    }
}

#[test]
fn canon_ef_spacing_keeps_raw_and_non_raw_profiles_the_same_lens() {
    let raw = crate::lens::lcp::test_profile_with_lens_id(
        "Canon",
        "EF24-105mm f/4L IS USM",
        "Original",
        Some("237"),
        -0.05,
        -0.3,
    );
    let jpeg = raw
        .replace("EF24-105", "EF 24-105")
        .replace(r#"CameraRawProfile="True""#, r#"CameraRawProfile="False""#);
    let library = Library::from_texts([("raw.lcp", raw.as_str()), ("jpeg.lcp", jpeg.as_str())]);
    let m = Metadata {
        make: "Canon".into(),
        lens_id: 237,
        focal: 24.,
        aperture: 11.,
        width: 5634,
        height: 3752,
        ..Default::default()
    };
    let profiles = library.for_photo(&m);
    assert_eq!(profiles.all().len(), 1);
    assert_eq!(profiles.auto(&m).unwrap().profile.filename, "raw.lcp");
}

#[test]
fn an_unmatched_imported_profile_is_not_reported_as_an_import_failure() {
    let mut m = photo();
    m.lens_model = "Unmatched 85mm F2".into();
    m.lens_profiles = library().for_photo(&m);
    let r = crate::model::recipe::Recipe {
        lens_profile: true,
        ..Default::default()
    };
    assert_eq!(
        r.missing_lens_profile(&m).as_deref(),
        Some("No matching imported Adobe lens profile for Unmatched 85mm F2; no lens correction")
    );
}
#[test]
fn an_edit_keeps_the_profile_it_names() {
    let m = photo();
    // Default and Auto keep a named profile of this lens over the best match.
    assert_eq!(
        used(&choice(LensProfileSetup::Auto, id(MINE)), &m).as_deref(),
        Some(MINE)
    );
    assert_eq!(
        used(&choice(LensProfileSetup::Default, id(MINE)), &m).as_deref(),
        Some(MINE)
    );
    // A profile of another lens only under Custom, as for an adapted lens.
    assert_eq!(
        used(&choice(LensProfileSetup::Custom, id(OTHER)), &m).as_deref(),
        Some(OTHER)
    );
    assert_eq!(
        used(&choice(LensProfileSetup::Auto, id(OTHER)), &m).as_deref(),
        Some(ADOBE)
    );
    // Without a file name, the profile name identifies it.
    let by_name = LensProfileId {
        name: "Mine (Testcam 35mm F2)".into(),
        ..Default::default()
    };
    assert_eq!(
        used(
            &choice(
                LensProfileSetup::Custom,
                Some(LensProfileRef::Adobe(by_name))
            ),
            &m
        )
        .as_deref(),
        Some(MINE)
    );
    // File names compare without case, as on macOS.
    assert_eq!(
        used(
            &choice(LensProfileSetup::Custom, id(&MINE.to_uppercase())),
            &m
        )
        .as_deref(),
        Some(MINE)
    );
}

#[test]
fn a_named_profile_that_is_not_imported_is_reported() {
    let m = photo();
    let gone = choice(LensProfileSetup::Custom, id("Gone.lcp"));
    let r = gone.resolve(&m.lens_profiles, &m);
    assert!(r.used.is_none());
    assert_eq!(r.missing.map(|i| i.filename.as_str()), Some("Gone.lcp"));
    // Auto still matches, and says what it stands in for.
    let auto = choice(LensProfileSetup::Auto, id("Gone.lcp"));
    let r = auto.resolve(&m.lens_profiles, &m);
    assert_eq!(r.used.map(|c| c.profile.filename.as_str()), Some(ADOBE));
    assert!(r.missing.is_some());
}

#[test]
fn choosing_a_profile_sets_custom_and_setup_rematches() {
    let m = photo();
    let other = &m
        .lens_profiles
        .all()
        .iter()
        .find(|c| c.profile.filename == OTHER)
        .unwrap()
        .profile;
    let mut c = LensProfileChoice::default();
    c.choose(other);
    assert_eq!(c.setup, LensProfileSetup::Custom);
    assert_eq!(
        c.adobe().map(|i| i.name.as_str()),
        Some("Adobe (Lensco 50mm F1.4)")
    );
    assert_eq!(used(&c, &m).as_deref(), Some(OTHER));
    // Auto and Default forget it and match again.
    c.set_setup(LensProfileSetup::Auto, Some(other));
    assert_eq!(c, choice(LensProfileSetup::Auto, None));
    assert_eq!(used(&c, &m).as_deref(), Some(ADOBE));
    // Custom keeps the profile in use.
    let adobe = &m.lens_profiles.auto(&m).unwrap().profile;
    c.set_setup(LensProfileSetup::Custom, Some(adobe));
    assert_eq!(c.adobe().map(|i| i.filename.as_str()), Some(ADOBE));
    // Custom with nothing in use keeps what the edit names.
    let mut named = choice(LensProfileSetup::Auto, id("Gone.lcp"));
    named.set_setup(LensProfileSetup::Custom, None);
    assert_eq!(named, choice(LensProfileSetup::Custom, id("Gone.lcp")));
}

#[test]
fn menus_list_makes_models_and_profiles() {
    let m = photo();
    let menus = ProfileMenus::new(&m.lens_profiles, &m);
    assert_eq!(menus.makes(), ["Lensco", "Testcam"]);
    assert_eq!(menus.models("Testcam"), ["Testcam 35mm F2"]);
    let names: Vec<&str> = menus
        .profiles("Testcam", "Testcam 35mm F2")
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    assert_eq!(names, ["Adobe (Testcam 35mm F2)", "Mine (Testcam 35mm F2)"]);
    assert_eq!(
        menus.first_of_make("Lensco").map(|p| p.filename.as_str()),
        Some(OTHER)
    );
}

#[test]
fn profiles_that_do_not_fit_the_camera_are_not_offered() {
    // A profile made on a smaller sensor of another make does not cover this one,
    // and a non-raw profile gives way to the raw one for the same lens.
    let small = test_profile("Tinycam", "12mm F2", "Adobe (Tinycam 12mm F2)", 0., -0.3)
        .replace(r#"SensorFormatFactor="1""#, r#"SensorFormatFactor="2""#);
    let jpeg = test_profile("Testcam", "35mm F2", "Adobe (Testcam 35mm F2)", 0., -0.3)
        .replace(r#"CameraRawProfile="True""#, r#"CameraRawProfile="False""#);
    let raw = test_profile("Testcam", "35mm F2", "Adobe (Testcam 35mm F2)", 0., -0.3);
    let mut m = photo();
    m.focal_35mm = 35.;
    let library = Library::from_texts([
        ("small.lcp", small.as_str()),
        ("jpeg.lcp", jpeg.as_str()),
        ("raw.lcp", raw.as_str()),
    ]);
    let offered = library.for_photo(&m);
    let names: Vec<&str> = offered
        .all()
        .iter()
        .map(|c| c.profile.filename.as_str())
        .collect();
    assert_eq!(names, ["raw.lcp"]);
}

#[test]
fn switching_to_custom_keeps_the_digest_of_the_same_profile() {
    let m = photo();
    let adobe = &m.lens_profiles.auto(&m).unwrap().profile;
    let recorded = LensProfileId {
        name: adobe.name.clone(),
        filename: adobe.filename.clone(),
        digest: "0123ABCD".into(),
        embedded: false,
    };
    let mut c = choice(
        LensProfileSetup::Auto,
        Some(LensProfileRef::Adobe(recorded.clone())),
    );
    c.set_setup(LensProfileSetup::Custom, Some(adobe));
    assert_eq!(c.adobe(), Some(&recorded));
    // Picking the profile already named keeps it too; another one does not.
    c.choose(adobe);
    assert_eq!(c.adobe(), Some(&recorded));
    let mine = &m
        .lens_profiles
        .all()
        .iter()
        .find(|c| c.profile.filename == MINE)
        .unwrap()
        .profile;
    c.choose(mine);
    assert!(c.adobe().unwrap().digest.is_empty());
}

#[test]
fn default_and_auto_render_alike() {
    assert_eq!(
        choice(LensProfileSetup::Default, None).rendering(),
        choice(LensProfileSetup::Auto, None).rendering()
    );
    assert_eq!(
        choice(LensProfileSetup::Default, id(MINE)).rendering(),
        choice(LensProfileSetup::Auto, id(MINE)).rendering()
    );
    let digest = Some(LensProfileRef::Adobe(LensProfileId {
        filename: MINE.into(),
        digest: "0123ABCD".into(),
        ..Default::default()
    }));
    assert_eq!(
        choice(LensProfileSetup::Custom, digest).rendering(),
        choice(LensProfileSetup::Custom, id(MINE)).rendering()
    );
    assert_ne!(
        choice(LensProfileSetup::Auto, id(MINE)).rendering(),
        choice(LensProfileSetup::Custom, id(MINE)).rendering()
    );
}

#[test]
fn profiles_without_a_correction_model_are_not_offered() {
    let empty = test_profile("Testcam", "28mm F2", "Adobe (Testcam 28mm F2)", 0., 0.).replace(
        r#"<stCamera:PerspectiveModel><rdf:Description stCamera:RadialDistortParam1="0">
  <stCamera:VignetteModel stCamera:VignetteModelParam1="0"/>
 </rdf:Description></stCamera:PerspectiveModel>"#,
        "",
    );
    assert!(!empty.contains("PerspectiveModel"));
    let library = Library::from_texts([("empty.lcp", empty.as_str())]);
    assert!(library.for_photo(&photo()).all().is_empty());
}

#[test]
fn choosing_an_imported_profile_replaces_the_cameras_own() {
    let m = photo();
    let adobe = &m.lens_profiles.auto(&m).unwrap().profile;
    // An embedded identity with the same name as an imported profile.
    let mut c = choice(
        LensProfileSetup::Default,
        Some(LensProfileRef::Adobe(LensProfileId {
            name: adobe.name.clone(),
            embedded: true,
            ..Default::default()
        })),
    );
    assert!(c.resolve(&m.lens_profiles, &m).used.is_none());
    c.choose(adobe);
    assert!(!c.adobe().unwrap().embedded);
    assert_eq!(used(&c, &m).as_deref(), Some(ADOBE));
}

#[test]
fn a_profile_whose_entries_for_this_lens_have_no_model_is_not_its_match() {
    // One file with an empty entry for the photo's lens and a usable one for another.
    let usable = test_profile("Testcam", "50mm F2", "Adobe (Testcam 35/50)", 0.01, -0.4);
    let empty = test_profile("Testcam", "35mm F2", "Adobe (Testcam 35/50)", 0., 0.).replace(
        r#"<stCamera:PerspectiveModel><rdf:Description stCamera:RadialDistortParam1="0">
  <stCamera:VignetteModel stCamera:VignetteModelParam1="0"/>
 </rdf:Description></stCamera:PerspectiveModel>"#,
        "",
    );
    let entry = |t: &str| {
        let start = t.find("<rdf:li>").unwrap();
        let end = t.rfind("</rdf:li>").unwrap() + "</rdf:li>".len();
        t[start..end].to_string()
    };
    let both = usable.replace(&entry(&usable), &(entry(&empty) + &entry(&usable)));
    let mut m = photo();
    m.lens_profiles = Library::from_texts([("both.lcp", both.as_str())]).for_photo(&m);
    assert_eq!(m.lens_profiles.all().len(), 1);
    assert!(m.lens_profiles.auto(&m).is_none());
    let custom = choice(LensProfileSetup::Custom, id("both.lcp"));
    let candidate = custom.resolve(&m.lens_profiles, &m).used.unwrap();
    assert!(candidate.correction(&m).is_some());
}

#[test]
fn picking_one_of_two_files_with_the_same_name_records_its_file() {
    let text = test_profile("Testcam", "35mm F2", "Adobe (Testcam 35mm F2)", -0.05, -0.5);
    let mut m = photo();
    m.lens_profiles =
        Library::from_texts([("a.lcp", text.as_str()), ("b.lcp", text.as_str())]).for_photo(&m);
    let b = &m.lens_profiles.all()[1].profile;
    let mut c = choice(
        LensProfileSetup::Auto,
        Some(LensProfileRef::Adobe(LensProfileId {
            name: b.name.clone(),
            digest: "0123ABCD".into(),
            ..Default::default()
        })),
    );
    c.choose(b);
    assert_eq!(used(&c, &m).as_deref(), Some("b.lcp"));
    assert_eq!(c.adobe().unwrap().digest, "0123ABCD");
}

#[test]
fn entries_without_a_model_do_not_outrank_usable_ones() {
    // The photo's lens profiled on this make without a model, and on another make
    // with one: the usable entry corrects.
    let empty = test_profile("Testcam", "35mm F2", "Adobe (Testcam 35mm F2)", 0., 0.).replace(
        r#"<stCamera:PerspectiveModel><rdf:Description stCamera:RadialDistortParam1="0">
  <stCamera:VignetteModel stCamera:VignetteModelParam1="0"/>
 </rdf:Description></stCamera:PerspectiveModel>"#,
        "",
    );
    let other_make = test_profile("Lensco", "35mm F2", "Adobe (Testcam 35mm F2)", 0., -0.5);
    let entry = |t: &str| {
        let start = t.find("<rdf:li>").unwrap();
        let end = t.rfind("</rdf:li>").unwrap() + "</rdf:li>".len();
        t[start..end].to_string()
    };
    let both = other_make.replace(&entry(&other_make), &(entry(&empty) + &entry(&other_make)));
    let mut m = photo();
    m.lens_profiles = Library::from_texts([("both.lcp", both.as_str())]).for_photo(&m);
    let auto = m.lens_profiles.auto(&m).expect("the usable entry matches");
    assert!(auto.correction(&m).unwrap().vignetting_gain(1.) > 1.01);
}

#[test]
fn a_recorded_file_is_not_stood_in_for_by_another_of_the_same_name() {
    let m = photo();
    let other_file = LensProfileId {
        name: "Adobe (Testcam 35mm F2)".into(),
        filename: "Testcam (35mm F2) - copy.lcp".into(),
        ..Default::default()
    };
    let c = choice(
        LensProfileSetup::Custom,
        Some(LensProfileRef::Adobe(other_file)),
    );
    let r = c.resolve(&m.lens_profiles, &m);
    assert!(r.used.is_none() && r.missing.is_some());
}

#[test]
fn profiles_whose_correction_does_not_validate_are_not_offered() {
    // A distortion model so strong its radial scale leaves the valid range.
    let wild = test_profile("Testcam", "28mm F2", "Adobe (Testcam 28mm F2)", 1000., 0.);
    let mut m = photo();
    m.lens_profiles = Library::from_texts([("wild.lcp", wild.as_str())]).for_photo(&m);
    let menus = ProfileMenus::new(&m.lens_profiles, &m);
    assert!(menus.profiles("Testcam", "Testcam 28mm F2").is_empty());
    assert!(menus.first_of_make("Testcam").is_none());
    assert!(menus.models("Testcam").is_empty());
    // Named under Auto, it is reported, not silently replaced.
    for setup in [LensProfileSetup::Auto, LensProfileSetup::Custom] {
        let named = choice(setup, id("wild.lcp"));
        assert!(
            named.resolve(&m.lens_profiles, &m).missing.is_some(),
            "{setup:?}"
        );
    }
}

#[test]
fn a_profile_leaving_distortion_to_the_camera_is_its_match() {
    // An entry with no model of its own that defers distortion to the RAW's data.
    let deferring = test_profile("Testcam", "35mm F2", "Adobe (Testcam 35mm F2)", 0., 0.)
        .replace(
            r#"<stCamera:PerspectiveModel><rdf:Description stCamera:RadialDistortParam1="0">
  <stCamera:VignetteModel stCamera:VignetteModelParam1="0"/>
 </rdf:Description></stCamera:PerspectiveModel>"#,
            "",
        )
        .replace(
            r#"stCamera:SensorFormatFactor="1""#,
            r#"stCamera:SensorFormatFactor="1" stCamera:PreferMetadataDistort="True""#,
        );
    let mut m = photo();
    m.lens = Some(crate::optics::LensCorrection {
        source: "Testcam built-in".into(),
        distortion: Some(crate::optics::Radial {
            knots: vec![0., 1.],
            values: vec![1., 1.02],
        }),
        vignetting: Some(crate::optics::Radial {
            knots: vec![0., 1.],
            values: vec![1., 1.5],
        }),
        ..Default::default()
    });
    m.lens_profiles = Library::from_texts([("defer.lcp", deferring.as_str())]).for_photo(&m);
    let c = m
        .lens_profiles
        .auto(&m)
        .expect("the deferring profile matches");
    let correction = c.correction(&m).unwrap();
    // Only the camera's distortion, not its vignetting.
    assert_eq!(correction.distortion, m.lens.as_ref().unwrap().distortion);
    assert!(correction.vignetting.is_none());
}

/// Today's saved Adobe choices read as they did; a Lensfun choice reads as one; one
/// that is damaged is refused, never read as an Adobe profile without a name.
#[test]
fn saved_profiles_read_as_the_source_they_name() {
    let adobe: LensProfileChoice = serde_json::from_str(
        r#"{"setup":"Custom","id":{"name":"Adobe (Sony FE 55mm F1.8 ZA)","filename":"a.lcp","digest":"AB","future":1}}"#,
    )
    .unwrap();
    assert_eq!(adobe.adobe().unwrap().filename, "a.lcp");
    assert!(adobe.lensfun().is_none());
    // Exactly the JSON it was written as.
    let embedded: LensProfileChoice =
        serde_json::from_str(r#"{"setup":"Auto","id":{"name":"Camera Settings","embedded":true}}"#)
            .unwrap();
    assert_eq!(
        serde_json::to_string(&embedded).unwrap(),
        r#"{"setup":"Auto","id":{"name":"Camera Settings","embedded":true}}"#
    );
    let mut lensfun = LensProfileChoice::default();
    lensfun.choose_lensfun(crate::lens::lensfun::fixture::choice());
    let text = serde_json::to_string(&lensfun).unwrap();
    assert!(text.contains(r#""id":{"lensfun":{"#), "{text}");
    let back: LensProfileChoice = serde_json::from_str(&text).unwrap();
    assert_eq!(back, lensfun);
    assert!(back.adobe().is_none());
    for damaged in [
        r#"{"setup":"Custom","id":{"lensfun":{"lens":3}}}"#,
        r#"{"setup":"Custom","id":{"lensfun":null}}"#,
        r#"{"setup":"Custom","id":{"lensfun":{"lens":{"maker":"A","model":"B","crop":1,"aspect":1.5}},"name":"Adobe (B)"}}"#,
    ] {
        assert!(
            serde_json::from_str::<LensProfileChoice>(damaged).is_err(),
            "{damaged}"
        );
        assert!(
            serde_json::from_str::<crate::model::recipe::Recipe>(&format!(
                r#"{{"lens_profile_choice":{}}}"#,
                &damaged[damaged.find(r#"{"setup"#).unwrap()..]
            ))
            .is_err()
        );
    }
    // Picking Default or Auto forgets the Lensfun lens; picking an Adobe profile
    // replaces it.
    let mut c = lensfun.clone();
    c.set_setup(LensProfileSetup::Auto, None);
    assert_eq!(c.id, None);
    let m = photo();
    let adobe = &m.lens_profiles.auto(&m).unwrap().profile;
    let mut c = lensfun.clone();
    c.choose(adobe);
    assert!(c.lensfun().is_none() && c.adobe().is_some());
}

/// A Lensfun choice renders from the calibration it copied, without the database,
/// and is never stood in for by an imported Adobe profile, even one of this lens.
#[test]
fn lensfun_renders_from_its_copy_and_never_as_adobe() {
    use crate::lens::lensfun::fixture;
    use crate::model::recipe::Recipe;
    let mut m = fixture::photo();
    // An imported Adobe profile of the same lens.
    let text = test_profile(
        "Canon",
        "EF70-200mm f/2.8L IS II USM",
        "Adobe (EF70-200)",
        -0.05,
        -0.5,
    );
    m.lens_profiles = Library::from_texts([("adobe.lcp", text.as_str())]).for_photo(&m);
    assert!(m.lens_profiles.auto(&m).is_some());
    let mut r = Recipe {
        lens_profile: true,
        ..Default::default()
    };
    r.lens_profile_choice.choose_lensfun(fixture::choice());
    assert!(r.lens_profile_in_use(&m).used.is_none());
    let c = r.lens_correction(&m).unwrap().into_owned();
    assert!(c.source.starts_with("Lensfun: "), "{}", c.source);
    assert!(c.distortion.is_some() && c.chromatic.is_some() && c.vignetting.is_some());
    assert!(c.chromatic_at_source);
    assert_eq!(r.missing_lens_profile(&m), None);
    // Rendering takes no database, so neither none installed nor another version of
    // it changes this: the choice carries its own calibration.
    // Switched off, nothing of it renders; Default never picks it.
    r.lens_profile = false;
    assert!(
        r.lens_correction(&m)
            .is_none_or(|c| !c.source.starts_with("Lensfun"))
    );
    let auto = Recipe {
        lens_profile: true,
        ..Default::default()
    };
    assert!(
        auto.lens_correction(&m)
            .unwrap()
            .source
            .starts_with("Adobe profile")
    );
}

/// An unresolved choice (read from XMP without the database) renders the built-in
/// correction or none, never an Adobe profile, and says so.
#[test]
fn an_unresolved_lensfun_choice_falls_back_to_built_in_only() {
    use crate::lens::lensfun::{LensfunChoice, fixture};
    use crate::model::recipe::Recipe;
    let mut m = fixture::photo();
    let text = test_profile(
        "Canon",
        "EF70-200mm f/2.8L IS II USM",
        "Adobe (EF70-200)",
        -0.05,
        -0.5,
    );
    m.lens_profiles = Library::from_texts([("adobe.lcp", text.as_str())]).for_photo(&m);
    let named = fixture::choice();
    let unresolved = LensfunChoice::unresolved(named.lens.clone(), named.label, 0);
    let mut r = Recipe {
        lens_profile: true,
        ..Default::default()
    };
    r.lens_profile_choice.choose_lensfun(unresolved);
    assert!(r.uses_lensfun());
    assert!(r.lens_correction(&m).is_none());
    let note = r.missing_lens_profile(&m).unwrap();
    assert!(
        note.starts_with("Lensfun profile \"Canon EF 70-200mm f/2.8L IS II USM\" isn't resolved"),
        "{note}"
    );
    assert!(note.ends_with("; no lens correction"), "{note}");
    m.lens = Some(crate::optics::LensCorrection {
        source: "Canon built-in".into(),
        default_on: true,
        distortion: crate::optics::Radial::new(vec![0., 1.], vec![1., 1.01]),
        ..Default::default()
    });
    assert_eq!(r.lens_correction(&m).unwrap().source, "Canon built-in");
    assert!(
        r.missing_lens_profile(&m)
            .unwrap()
            .ends_with("; using Canon built-in")
    );
    // Resolving copies the installed calibration.
    let installed = fixture::installed(-0.02, 1_793_180_205);
    let resolved = r
        .lens_profile_choice
        .lensfun()
        .unwrap()
        .resolved(&installed.get().unwrap().db, &m)
        .unwrap();
    assert_eq!(resolved.calibration, fixture::choice().calibration);
}

/// A recipe naming a Lensfun lens, used or not, is saved as version 12, which
/// releases before Lensfun refuse; every other stays 11.
#[test]
fn lensfun_recipes_save_in_a_format_earlier_releases_refuse() {
    use crate::model::{recipe::Recipe, saved_format};
    let plain = Recipe::default();
    assert_eq!(saved_format::schema_for(&plain), saved_format::SCHEMA);
    let mut off = Recipe::default();
    off.lens_profile_choice
        .choose_lensfun(crate::lens::lensfun::fixture::choice());
    assert!(!off.lens_profile);
    assert_eq!(saved_format::schema_for(&off), saved_format::LENSFUN_SCHEMA);
    let envelope = serde_json::json!({"schema": 12, "pipeline": 12, "recipe": off});
    assert!(!saved_format::reads(&envelope, saved_format::SCHEMA));
    let mut read = envelope;
    saved_format::migrate_recipe(&mut read).unwrap();
    let back: Recipe = serde_json::from_value(read["recipe"].clone()).unwrap();
    assert_eq!(back, off);
}

/// Copy Settings and Sync take a Lensfun lens to another camera's photo with that
/// camera's context, keeping the calibration and its provenance: a camera Lensfun
/// knows renders even without a 35 mm equivalent focal length in the photo.
#[test]
fn a_transferred_lensfun_choice_takes_the_target_s_camera() {
    use crate::lens::lensfun::fixture;
    use crate::model::recipe::Recipe;
    use crate::model::settings_groups::{GroupSelection, Source, Target, transfer_with_lensfun};
    let source_photo = fixture::photo();
    let target_photo = fixture::other_camera();
    let installed = fixture::installed(-0.02, 1_793_180_205);
    let mut source = Recipe {
        lens_profile: true,
        ..Default::default()
    };
    source.lens_profile_choice.choose_lensfun(fixture::choice());
    let out = transfer_with_lensfun(
        Source {
            recipe: &source,
            metadata: &source_photo,
        },
        &Recipe::default(),
        &GroupSelection::all(),
        Target {
            metadata: &target_photo,
            profiles: &[],
        },
        &installed,
    );
    let moved = out.recipe.lens_profile_choice.lensfun().unwrap();
    assert_eq!(moved.camera.as_ref().unwrap().model, "Canon EOS 6D");
    assert_eq!(moved.calibration, fixture::choice().calibration);
    assert_eq!(moved.provenance, fixture::choice().provenance);
    assert!(
        out.recipe
            .lens_correction(&target_photo)
            .unwrap()
            .source
            .starts_with("Lensfun")
    );
    // Without a database, the copy keeps what the photo itself says: nothing here.
    let alone = fixture::other_camera();
    let out = transfer_with_lensfun(
        Source {
            recipe: &source,
            metadata: &source_photo,
        },
        &Recipe::default(),
        &GroupSelection::all(),
        Target {
            metadata: &alone,
            profiles: &[],
        },
        &fixture::none(),
    );
    assert!(
        out.recipe
            .lens_profile_choice
            .lensfun()
            .unwrap()
            .camera
            .is_none()
    );
    assert!(
        out.recipe
            .missing_lens_profile(&alone)
            .unwrap()
            .contains("crop factor")
    );
}
