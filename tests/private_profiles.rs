use rawmakase::model::recipe::Recipe;
use rawmakase::{
    camera_data::{CameraImage, Metadata},
    camera_profiles, develop,
};
use rawmakase_cancel::Cancel;
#[test]
#[ignore = "User-provided private DCP files; set RAWMAKASE_PROFILES"]
fn imported_profiles_render_and_roundtrip() -> anyhow::Result<()> {
    let dir = std::path::PathBuf::from(std::env::var("RAWMAKASE_PROFILES")?);
    let mut count = 0;
    let mut errors = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if !path
            .extension()
            .is_some_and(|s| s.eq_ignore_ascii_case("dcp"))
        {
            continue;
        }
        let p = match camera_profiles::from_bytes(&std::fs::read(&path)?) {
            Ok(p) => p,
            Err(e) => {
                errors.push(format!("{}: {e:#}", path.display()));
                continue;
            }
        };
        let m = Metadata {
            model: p.camera.clone(),
            width: 16,
            height: 16,
            wb: [1.; 3],
            matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            ..Default::default()
        };
        p.ensure_camera(&m)?;
        let recipe = Recipe {
            profile: Some(std::sync::Arc::new(p)),
            ..Default::default()
        };
        let im = CameraImage {
            width: 16,
            height: 16,
            pixels: (0..256).map(|i| [i as f32 / 300., 0.3, 0.1]).collect(),
            metadata: m,
            fast: false,
            scale_factor: 1.,
            scale_clipped: 0,
            hdr: Default::default(),
            recovered: Default::default(),
        };
        let out = develop::render(&im, &recipe.checked()?, 0, Cancel::NEVER)?;
        assert!(
            out.pixels
                .iter()
                .flatten()
                .all(|v| v.is_finite() && (0. ..=1.).contains(v))
        );
        let saved = serde_json::to_vec(&recipe)?;
        let loaded: Recipe = serde_json::from_slice(&saved)?;
        assert_eq!(recipe, loaded);
        count += 1;
    }
    println!("{count} profiles passed");
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    assert!(count >= 2);
    Ok(())
}

#[test]
#[ignore = "Explicitly imported private DCP/XMP profiles; no Adobe directory discovery"]
fn imported_enhanced_profiles_match_camera_and_resolve_xmp() -> anyhow::Result<()> {
    let raw = rawmakase::photo::open(&std::path::PathBuf::from(std::env::var(
        "RAWMAKASE_PROFILE_RAW",
    )?))?;
    let (profiles, errors) = camera_profiles::installed(&raw.metadata);
    assert!(errors.is_empty(), "{errors:?}");
    for name in [
        "Adobe Color",
        "Adobe Portrait",
        "Adobe Neutral",
        "Adobe Landscape",
        "Adobe Vivid",
        "Adobe Monochrome",
    ] {
        let p = profiles.iter().find(|p| p.name == name).unwrap();
        p.ensure_camera(&raw.metadata)?;
        let look = p.enhanced.as_ref().unwrap();
        assert_eq!(look.base_name, "Adobe Standard");
        let xml = format!(
            r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><r:RDF xmlns:r="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><r:Description xmlns:c="http://ns.adobe.com/camera-raw-settings/1.0/" c:CameraProfile="Adobe Standard"><c:Look><r:Description c:Name="{name}" c:UUID="{}" c:Amount="1"/></c:Look></r:Description></r:RDF></x:xmpmeta>"#,
            look.uuid
        );
        let preset = rawmakase::xmp::parse(std::path::Path::new("reference.xmp"), &xml)?;
        let recipe = preset.apply(&Recipe::default(), &raw.metadata, &profiles, None)?;
        assert_eq!(recipe.profile.as_ref().unwrap().name, name);
        let saved = serde_json::to_vec(&recipe)?;
        let loaded: Recipe = serde_json::from_slice(&saved)?;
        assert_eq!(recipe, loaded);
        let metadata = Metadata {
            make: raw.metadata.make.clone(),
            model: raw.metadata.model.clone(),
            width: 16,
            height: 16,
            wb: [1.; 3],
            ..Default::default()
        };
        let image = CameraImage {
            width: 16,
            height: 16,
            pixels: (0..256).map(|i| [i as f32 / 300., 0.2, 0.1]).collect(),
            metadata,
            fast: false,
            scale_factor: 1.,
            scale_clipped: 0,
            hdr: Default::default(),
            recovered: Default::default(),
        };
        let full = develop::render(&image, &recipe.checked()?, 0, Cancel::NEVER)?;
        let tile = develop::render_region(&image, &recipe.checked()?, [4, 4, 8, 8])?;
        for y in 0..8 {
            for x in 0..8 {
                assert_eq!(tile.pixels[y * 8 + x], full.pixels[(y + 4) * 16 + x + 4]);
            }
        }
        assert!(
            full.pixels
                .iter()
                .flatten()
                .all(|v| v.is_finite() && (0. ..=1.).contains(v))
        );
        assert_eq!(
            recipe, loaded,
            "Rendering must not change sliders or embedded profile data"
        );
        if name == "Adobe Monochrome" {
            assert!(
                full.pixels
                    .iter()
                    .all(|p| (p[0] - p[1]).abs() < 1e-4 && (p[1] - p[2]).abs() < 1e-4)
            );
        }
        let mut wrong = raw.metadata.clone();
        wrong.model = "Different camera".into();
        assert!(p.ensure_camera(&wrong).is_err());
        let text = format!(
            r#"s = {{ CameraProfile = "Adobe Standard", Look = {{ Name = "{name}", UUID = "{}", Amount = 1 }} }}"#,
            look.uuid
        );
        let (catalog, warnings) =
            rawmakase::lr_develop::convert_develop(&text, &raw.metadata, &profiles, None)?;
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(catalog.profile.as_ref().unwrap().name, name);
    }
    assert_eq!(
        Recipe::with_profiles(&raw.metadata, &profiles)
            .profile
            .unwrap()
            .name,
        "Adobe Color"
    );
    Ok(())
}
