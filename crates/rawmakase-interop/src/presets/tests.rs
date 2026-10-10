use super::*;
use crate::model::recipe::Recipe;
use anyhow::Result;
use std::fs::{self, File};
#[test]
#[allow(clippy::approx_constant)] // Exact camera matrix coefficients, not mathematical constants.
fn embedded_profile_roundtrip_and_old_engine_pixels() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("profile.json");
    let m = crate::camera_data::Metadata {
        make: "Fujifilm".into(),
        model: "X100F".into(),
        cam_xyz: [
            [1.1434, -0.4948, -0.121],
            [-0.3746, 1.2042, 0.1903],
            [-0.0666, 0.1479, 0.5235],
        ],
        ..Default::default()
    };
    let recipe = Recipe {
        profile: crate::camera_profiles::CameraProfile::camera_matrix_default(&m)
            .map(std::sync::Arc::new),
        ..Recipe::for_metadata(&m)
    };
    assert!(recipe.profile.is_some());
    save_preset(&path, &recipe)?;
    assert_eq!(load_preset(&path)?, recipe);
    let mut legacy = Recipe {
        engine: 2,
        sharpening: 0.,
        exposure: 0.75,
        ..Default::default()
    };
    legacy.curve.insert([0.3, 0.2]);
    save_preset(&path, &legacy)?;
    let mut v: serde_json::Value = serde_json::from_reader(File::open(&path)?)?;
    v["schema"] = 2.into();
    v["pipeline"] = 2.into();
    for key in [
        "engine",
        "profile",
        "sharpening_radius",
        "sharpening_detail",
        "sharpening_masking",
    ] {
        v["recipe"].as_object_mut().unwrap().remove(key);
    }
    fs::write(&path, serde_json::to_vec(&v)?)?;
    let loaded = load_preset(&path)?;
    assert_eq!(loaded.engine, 2);
    assert_eq!(loaded.sharpening, 0.);
    assert!(loaded.profile.is_none());
    assert_eq!(loaded, legacy);
    Ok(())
}

#[allow(clippy::approx_constant)] // Exact camera matrix coefficients, not mathematical constants.
fn x100f() -> crate::camera_data::Metadata {
    crate::camera_data::Metadata {
        make: "Fujifilm".into(),
        model: "X100F".into(),
        wb: [2.0198677, 1., 1.8874172],
        cam_xyz: [
            [1.1434, -0.4948, -0.121],
            [-0.3746, 1.2042, 0.1903],
            [-0.0666, 0.1479, 0.5235],
        ],
        ..Default::default()
    }
}
/// What `camera_profiles::installed` gives with nothing imported.
fn open_profiles(
    m: &crate::camera_data::Metadata,
) -> Vec<std::sync::Arc<crate::camera_profiles::CameraProfile>> {
    use crate::camera_profiles::open;
    [open::standard(m), open::color(m)]
        .into_iter()
        .flatten()
        .chain(crate::camera_profiles::film::profiles(m))
        .map(std::sync::Arc::new)
        .collect()
}

#[test]
fn every_builtin_file_is_listed_and_parses() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/presets");
    let mut on_disk = Vec::new();
    for group in fs::read_dir(&root).unwrap().flatten() {
        for file in fs::read_dir(group.path()).unwrap().flatten() {
            let path = file.path();
            if path.extension().is_some_and(|e| e == "xmp") {
                on_disk.push(
                    path.strip_prefix(&root)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
            }
        }
    }
    let mut listed: Vec<_> = builtin::FILES.iter().map(|(p, _)| p.to_string()).collect();
    on_disk.sort();
    listed.sort();
    assert_eq!(on_disk, listed, "assets/presets and builtin::FILES differ");

    let (presets, errors) = builtin::presets();
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(presets.len(), builtin::FILES.len());
    let mut ids = std::collections::BTreeSet::new();
    let mut names = std::collections::BTreeSet::new();
    for p in &presets {
        assert!(p.builtin);
        assert!(p.blockers.is_empty(), "{}: {:?}", p.name, p.blockers);
        assert!(builtin::GROUPS.contains(&p.group.as_str()), "{}", p.group);
        assert!(ids.insert(p.id.clone()), "duplicate id {}", p.id);
        assert!(
            names.insert((p.group.clone(), p.name.clone())),
            "{}",
            p.name
        );
        assert_eq!(
            p.settings.get("Copyright").map(String::as_str),
            Some("RAWmakase contributors, MIT licence")
        );
        // Only the looks that need one name a profile, and only one we can stand in for.
        if let Some(profile) = p.settings.get("CameraProfile") {
            assert_eq!(p.group, "Creative", "{}", p.name);
            assert!(["Adobe Standard", "Adobe Color"].contains(&profile.as_str()));
        }
    }
    let ranks: Vec<_> = presets
        .iter()
        .map(|p| builtin::group_rank(&p.group))
        .collect();
    assert!(ranks.is_sorted(), "built-in presets out of group order");
}

#[test]
fn builtin_presets_apply_without_imported_profiles() {
    let m = x100f();
    let profiles = open_profiles(&m);
    let base = Recipe::with_profiles(&m, &profiles);
    let (presets, _) = builtin::presets();
    for p in &presets {
        let r = p
            .apply(&base, &m, &profiles, None)
            .unwrap_or_else(|e| panic!("{}: {e:#}", p.name));
        if p.group == "Film" {
            // A film preset chooses its look, which is always there.
            let look = r.profile.as_ref().unwrap();
            assert_eq!(format!("RMKS Film: {}", p.name), look.name);
            assert!(p.profile_substitute(&m, &profiles).is_none());
            continue;
        }
        match p.settings.get("CameraProfile") {
            Some(_) => {
                assert_eq!(
                    r.profile.as_ref().unwrap().name,
                    crate::camera_profiles::open::STANDARD
                );
                assert_eq!(
                    p.profile_substitute(&m, &profiles),
                    Some(("Adobe Standard".into(), "RAWmakase Standard".into()))
                );
            }
            // Profile-free presets keep the photo's profile.
            None => {
                assert_eq!(r.profile, base.profile, "{}", p.name);
                assert!(p.profile_substitute(&m, &profiles).is_none());
            }
        }
    }
}

#[test]
fn builtin_presets_prefer_imported_adobe_profiles() {
    let m = x100f();
    let mut profiles = open_profiles(&m);
    let mut adobe = crate::camera_profiles::open::standard(&m).unwrap();
    adobe.name = "Adobe Standard".into();
    profiles.push(std::sync::Arc::new(adobe));
    let (presets, _) = builtin::presets();
    let haze = presets.iter().find(|p| p.name == "Concrete Haze").unwrap();
    let r = haze
        .apply(&Recipe::with_profiles(&m, &profiles), &m, &profiles, None)
        .unwrap();
    assert_eq!(r.profile.unwrap().name, "Adobe Standard");
    assert!(haze.profile_substitute(&m, &profiles).is_none());

    // The DNG's own profile comes before RAWmakase Standard.
    let chart = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/charts/fujifilm-x100f-d65.dng");
    let dng = crate::camera_data::Metadata {
        embedded_dcp: crate::dng::read(&chart)
            .and_then(|d| d.profile)
            .map(Into::into),
        ..x100f()
    };
    let embedded = crate::camera_profiles::builtin(&dng).expect("the chart's profile fits");
    let profiles = open_profiles(&dng);
    let r = haze
        .apply(
            &Recipe::with_profiles(&dng, &profiles),
            &dng,
            &profiles,
            None,
        )
        .unwrap();
    assert_eq!(r.profile.unwrap().name, embedded.name);
}

#[test]
fn imported_presets_still_need_their_profile() {
    let m = x100f();
    let profiles = open_profiles(&m);
    let (presets, _) = builtin::presets();
    let mut imported = presets
        .into_iter()
        .find(|p| p.name == "Faded Gold")
        .unwrap();
    imported.builtin = false;
    let base = Recipe::with_profiles(&m, &profiles);
    let e = imported.apply(&base, &m, &profiles, None).unwrap_err();
    assert!(format!("{e:#}").contains("Missing camera profile ‘Adobe Standard’"));
    assert!(imported.profile_substitute(&m, &profiles).is_none());
}

/// Built-in presets offer Lightroom's Amount, except Linear, which resets the curve
/// as Adobe's "None" presets do.
#[test]
fn builtin_presets_offer_an_amount() {
    let m = x100f();
    let profiles = open_profiles(&m);
    let base = Recipe::with_profiles(&m, &profiles);
    for p in builtin::presets().0 {
        let full = p.apply(&base, &m, &profiles, None).unwrap();
        let amount = super::amount::PresetAmount::new(&p, base.clone(), full);
        assert_eq!(amount.is_ok(), p.name != "Linear", "{}", p.name);
        if let Ok(amount) = amount {
            for t in [0., 0.5, 2.] {
                amount.at(t, &m).validate().unwrap();
            }
        }
    }
}
