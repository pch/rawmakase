//! Upgrading a catalog to store each profile once changes no render: every edit and
//! History state develops bit for bit as it did before the upgrade, on the CPU.
use crate::camera_data::{CameraImage, Decode, Demosaic, Metadata, Source};
use crate::camera_profiles::{CameraProfile, fitted};
use crate::catalog::upgrade_fixtures::{Earlier, inline};
use crate::model::recipe::Recipe;
use anyhow::Result;
use rawmakase_cancel::Cancel;
use std::sync::Arc;

/// The synthetic chart, decoded at full size.
fn chart(path: &std::path::Path) -> Result<CameraImage> {
    let crate::photo::Opened::Raw(raw) = crate::photo::open_source(path, Cancel::NEVER)? else {
        anyhow::bail!("the chart is a RAW")
    };
    raw.develop(Decode::full(Demosaic::default()), Cancel::NEVER)
}
/// `r` developed on the CPU, as the bits of its pixels.
fn bits(image: &CameraImage, r: &Recipe) -> Result<Vec<u32>> {
    let rendered = crate::develop::render(image, &r.checked()?, 0, Cancel::NEVER)?;
    Ok(rendered
        .pixels
        .iter()
        .flatten()
        .map(|v| v.to_bits())
        .collect())
}
fn read_inline(r: &Recipe) -> Recipe {
    serde_json::from_value(inline(r)).unwrap()
}

#[test]
fn the_upgrade_changes_no_render() -> Result<()> {
    let m = |seed: f32| Metadata {
        make: "RAWmakase".into(),
        model: "Synthetic Chart".into(),
        cam_xyz: [[0.9, -0.2, seed], [-0.4, 1.2, 0.15], [-0.05, 0.1, 0.6]],
        ..Default::default()
    };
    let matrix = CameraProfile::camera_matrix_default(&m(0.05)).unwrap();
    let mut look = fitted::FittedLook::identity();
    look.table[7][2] = 1.1;
    let mut embedded = serde_json::to_value(CameraProfile::embedded(&Metadata {
        source: Source::Rendered,
        ..m(0.)
    }))?;
    embedded["tone"] = serde_json::json!([[0., 0.], [1., 1.]]);
    let cube = "LUT_3D_SIZE 2\n0 0 0\n1 0 0\n0 1 0\n1 1 0\n0 0 1\n1 0 1\n0 1 1\n0.9 0.95 1\n";
    let profiles: Vec<Arc<CameraProfile>> = vec![
        Arc::new(matrix.clone().with_test_srgb_look()),
        Arc::new(fitted::with_look(&matrix, &look)?),
        Arc::new(matrix.with_cube_for_test(cube)?),
        Arc::new(serde_json::from_value(embedded)?),
    ];
    let mut edits: Vec<Recipe> = profiles
        .iter()
        .enumerate()
        .map(|(n, p)| Recipe {
            profile: Some(p.clone()),
            exposure: 0.2 * n as f32,
            contrast: 0.3,
            ..Default::default()
        })
        .collect();
    let mut lens = edits[0].clone();
    lens.lens_profile = true;
    lens.lens_profile_choice
        .choose_lensfun(rawmakase_model::lens::lensfun::fixture::choice());
    edits.push(lens);
    let mut f = Earlier::new(6, edits.len())?;
    for (n, r) in edits.iter().enumerate() {
        f.set_edit(n, r)?;
    }
    let states = [edits[2].clone(), edits[1].clone(), edits[0].clone()];
    f.set_history(0, &states)?;
    let image = chart(&f.photos[0].1)?;
    let before: Vec<Vec<u32>> = edits
        .iter()
        .map(|r| bits(&image, &read_inline(r)))
        .collect::<Result<_>>()?;
    let before_steps: Vec<Vec<u32>> = states
        .iter()
        .map(|r| bits(&image, &read_inline(r)))
        .collect::<Result<_>>()?;

    // The edits render differently, so equal renders mean something.
    assert!(before.windows(2).all(|w| w[0] != w[1]));
    let catalog = f.open()?;
    crate::storage::recipe_assets::clear_cache();
    for (n, (id, file)) in f.photos.iter().enumerate() {
        let recipe = catalog.load_edit(*id, file)?.unwrap().recipe;
        assert!(
            bits(&image, &recipe)? == before[n],
            "edit {n} renders differently"
        );
    }
    let history = catalog.load_history(f.photos[0].0)?.unwrap();
    let after_steps: Vec<Vec<u32>> = std::iter::once(&history.origin)
        .chain(history.steps.iter().map(|s| &s.recipe))
        .map(|r| bits(&image, r))
        .collect::<Result<_>>()?;
    assert!(
        after_steps == before_steps,
        "a History state renders differently"
    );
    Ok(())
}
