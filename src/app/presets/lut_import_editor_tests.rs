//! Editor tests of `lut_import`: the whole editor driven through its frames.
use crate::app::testing::*;
use crate::app::*;
use crate::camera_data::Metadata;
use eframe::egui::{Pos2, Rect};

/// A raw photo's Profile menu offers Import .cube LUTs…, which opens the window at
/// sRGB. Choosing Rec. 2020, the last of Create Profile's five spaces, and Choose
/// Files… asks for the file chooser for Rec. 2020 LUTs; Cancel asks for nothing; the
/// window opens at sRGB again.
#[test]
fn import_cube_luts_asks_the_space_before_the_files() {
    use egui::accesskit::Role;
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    ctx.all_styles_mut(|s| s.animation_time = 0.);
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    let m = Metadata {
        make: "Test".into(),
        model: "Camera".into(),
        cam_xyz: [[0.8, -0.2, -0.1], [-0.3, 1.1, 0.2], [-0.05, 0.15, 0.6]],
        width: 64,
        height: 48,
        ..Default::default()
    };
    // Keep the menu independent of the user's installed look packs: enough
    // real looks would put the import action below the popup's scroll clip.
    let profiles = crate::camera_profiles::listed_from(&[], &m).profiles;
    editor.document.reset(None);
    let (generation, _) = editor.document.start_load();
    for event in [
        Event::Header(Box::new(worker::LoadedHeader {
            id: generation,
            file: None,
            path: "photo.dng".into(),
            metadata: m,
            recipe: Recipe::default(),
            export: Default::default(),
            status: "Original".into(),
        })),
        Event::Profiles {
            id: generation,
            profiles,
            errors: Vec::new(),
            groups: Default::default(),
            imported: false,
        },
    ] {
        editor.tx.send(event).unwrap();
    }
    editor.events(&ctx);
    controls_frame(&ctx, &mut editor, vec![]);
    let update = controls_frame(&ctx, &mut editor, vec![]);
    let profile = update
        .nodes
        .iter()
        .map(|(_, n)| n)
        .find(|n| n.role() == Role::ComboBox && n.value() == Some("RAWmakase Color"))
        .cloned()
        .expect("the Profile menu");
    let [down, up] = press_and_release(centre(&profile));
    controls_frame(&ctx, &mut editor, down);
    controls_frame(&ctx, &mut editor, up);
    let menu = controls_frame(&ctx, &mut editor, vec![]);
    let node = |update: &egui::accesskit::TreeUpdate, label: &str| {
        update
            .nodes
            .iter()
            .map(|(_, n)| n)
            .find(|n| n.label() == Some(label) || n.value() == Some(label))
            .cloned()
            .unwrap_or_else(|| panic!("no {label}"))
    };
    let item = node(&menu, "Import .cube LUTs…");
    let [down, up] = press_and_release(centre(&item));
    controls_frame(&ctx, &mut editor, down);
    controls_frame(&ctx, &mut editor, up);
    assert_eq!(
        editor.lut_import,
        Some(crate::camera_profiles::LutSpace::Srgb)
    );

    // The window on its own, as the workspace draws it over everything.
    let frame = |editor: &mut Editor, events| {
        let mut asked = None;
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(800., 600.))),
                events,
                ..Default::default()
            },
            |ui| asked = editor.lut_import_ui(ui.ctx()),
        );
        output.textures_delta.clear();
        let update = output.platform_output.accesskit_update.expect("on");
        (asked, update)
    };
    let click = |editor: &mut Editor, label: &str| {
        let (_, shown) = frame(editor, vec![]);
        let [down, up] = press_and_release(centre(&node(&shown, label)));
        let (first, _) = frame(editor, down);
        let (second, _) = frame(editor, up);
        first.or(second)
    };
    frame(&mut editor, vec![]);
    assert_eq!(click(&mut editor, "sRGB"), None);
    assert_eq!(click(&mut editor, "Rec. 2020"), None);
    assert_eq!(
        editor.lut_import,
        Some(crate::camera_profiles::LutSpace::Rec2020)
    );
    assert_eq!(
        click(&mut editor, "Choose Files…"),
        Some(crate::app::shell::dialogs::FileDialog::Luts(
            crate::camera_profiles::LutSpace::Rec2020
        ))
    );
    assert_eq!(editor.lut_import, None);
    // Again: sRGB, whatever was chosen before; Cancel asks for nothing.
    editor.open_lut_import();
    let (_, shown) = frame(&mut editor, vec![]);
    node(&shown, "sRGB");
    assert_eq!(click(&mut editor, "Cancel"), None);
    assert_eq!(editor.lut_import, None);
    let (asked, _) = frame(&mut editor, vec![]);
    assert_eq!(asked, None);
}
