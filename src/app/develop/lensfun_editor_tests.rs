//! Editor tests of `lensfun`: the whole editor driven through its frames.
use crate::app::testing::*;
use crate::app::*;
use eframe::egui::{Pos2, Rect};

/// The Lensfun row fits the inspector at its usual width: before and after the
/// download, its text stays inside the panel, what it says wraps below it, and
/// nothing reaches past the panel's right edge (which would widen it).
#[test]
fn lensfun_row_stays_inside_a_narrow_panel() -> anyhow::Result<()> {
    const WIDTH: f32 = 320.;
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    ctx.all_styles_mut(|s| s.animation_time = 0.);
    let dir = tempfile::tempdir()?;
    let archive =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/lensfun/good.tar.bz2");
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    editor.lensfun =
        crate::app::develop::lensfun::Lensfun::in_folder(dir.path().to_path_buf(), archive.clone());
    editor.document.metadata = Some(crate::lens::lensfun::fixture::photo());
    let frame = |editor: &mut Editor| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(WIDTH, 6000.))),
                ..Default::default()
            },
            |ui| {
                let frame = editor.begin_edit_frame();
                egui::CentralPanel::default().show(ui, |ui| editor.controls(ui));
                editor.finish_edit_frame(frame, ui.ctx());
            },
        );
        output.textures_delta.clear();
        output
            .platform_output
            .accesskit_update
            .expect("accessibility is on")
    };
    let find = |tree: &egui::accesskit::TreeUpdate, text: &str| {
        let nodes: Vec<_> = tree
            .nodes
            .iter()
            .map(|(_, n)| n)
            .filter(|n| n.bounds().is_some())
            .collect();
        nodes
            .iter()
            .find(|n| {
                n.label().is_some_and(|l| l.starts_with(text))
                    || n.value().is_some_and(|l| l.starts_with(text))
            })
            .map(|n| (*n).clone())
            .unwrap_or_else(|| {
                let labels: Vec<_> = nodes
                    .iter()
                    .filter_map(|n| n.label().or(n.value()))
                    .collect();
                panic!("no {text:?} among {labels:?}")
            })
    };
    let inside = |node: &egui::accesskit::Node| {
        let b = node.bounds().expect("placed");
        assert!(
            b.x1 as f32 <= WIDTH + 0.5,
            "{:?} reaches {} of {WIDTH}",
            node.label().or(node.value()),
            b.x1
        );
        b
    };
    let tree = frame(&mut editor);
    let row = inside(&find(&tree, "Community lens profiles"));
    let download = inside(&find(&tree, "Download"));
    assert!(download.y0 >= row.y1, "the download is below the title");
    let terms = inside(&find(&tree, "Lensfun database by Lensfun contributors"));
    assert!(terms.y0 >= download.y1, "the terms are below the download");
    crate::lensfun_install::install(
        dir.path(),
        &crate::lensfun_install::Source::File(archive),
        &Default::default(),
    )?;
    frame(&mut editor);
    let tree = frame(&mut editor);
    let lens = inside(&find(&tree, "Canon EF 70-200mm f/2.8L IS II USM"));
    let gives = inside(&find(&tree, "Corrects "));
    assert!(gives.y0 >= lens.y1, "what it corrects is below the lens");
    let use_button = tree
        .nodes
        .iter()
        .map(|(_, n)| n)
        .find(|n| n.role() == egui::accesskit::Role::Button && n.label() == Some("Use"))
        .expect("the Use button");
    let button = inside(use_button);
    assert!(button.y0 >= gives.y1, "Use is below what it corrects");
    Ok(())
}

/// Where no imported profile fits, Lens Corrections offers Lensfun's download, which
/// only installs; installed, it offers the lens this photo matches, and Use chooses
/// it, with Enable Profile Corrections, as one History step.
#[test]
fn lensfun_is_offered_downloaded_and_used_in_one_step() -> anyhow::Result<()> {
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    ctx.all_styles_mut(|s| s.animation_time = 0.);
    let dir = tempfile::tempdir()?;
    let archive =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/lensfun/good.tar.bz2");
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    editor.lensfun =
        crate::app::develop::lensfun::Lensfun::in_folder(dir.path().to_path_buf(), archive.clone());
    editor.document.metadata = Some(crate::lens::lensfun::fixture::photo());
    // Tall and wide enough for the whole panel, Lens Corrections being far down.
    let frame = |editor: &mut Editor, events: Vec<egui::Event>| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(480., 6000.))),
                events,
                ..Default::default()
            },
            |ui| {
                let frame = editor.begin_edit_frame();
                egui::CentralPanel::default().show(ui, |ui| editor.controls(ui));
                editor.finish_edit_frame(frame, ui.ctx());
            },
        );
        output.textures_delta.clear();
        output
            .platform_output
            .accesskit_update
            .expect("accessibility is on")
    };
    let find = |tree: &egui::accesskit::TreeUpdate, text: &str| {
        tree.nodes
            .iter()
            .map(|(_, n)| n)
            .find(|n| {
                n.label().is_some_and(|l| l.contains(text))
                    || n.value().is_some_and(|v| v.contains(text))
            })
            .cloned()
    };
    let tree = frame(&mut editor, vec![]);
    assert!(find(&tree, "Community lens profiles").is_some());
    assert!(find(&tree, "Download · 0.5 MB").is_some());
    assert!(find(&tree, "CC BY-SA 3.0").is_some());
    let before = editor.document.edit.recipe().clone();
    // The download (an archive here) installs and changes no edit.
    crate::lensfun_install::install(
        dir.path(),
        &crate::lensfun_install::Source::File(archive),
        &Default::default(),
    )?;
    let tree = frame(&mut editor, vec![]);
    assert_eq!(*editor.document.edit.recipe(), before);
    assert!(find(&tree, "Canon EF 70-200mm f/2.8L IS II USM").is_some());
    assert!(find(&tree, "Corrects ").is_some());
    let steps = editor.document.edit.history().steps().0.len();
    let use_button = tree
        .nodes
        .iter()
        .map(|(_, n)| n)
        .find(|n| n.role() == egui::accesskit::Role::Button && n.label() == Some("Use"))
        .cloned()
        .expect("the Use button");
    let [down, up] = press_and_release(centre(&use_button));
    frame(&mut editor, down);
    frame(&mut editor, up);
    frame(&mut editor, vec![]);
    let r = editor.document.edit.recipe();
    assert!(r.lens_profile);
    let chosen = r.lens_profile_choice.lensfun().expect("a Lensfun lens");
    assert!(chosen.is_resolved());
    assert_eq!(chosen.lens.model, "Canon EF 70-200mm f/2.8L IS II USM");
    assert_eq!(editor.document.edit.history().steps().0.len(), steps + 1);
    assert!(
        r.lens_correction(editor.document.metadata.as_ref().unwrap())
            .unwrap()
            .source
            .starts_with("Lensfun")
    );
    // The menus show it as chosen.
    let tree = frame(&mut editor, vec![]);
    assert!(find(&tree, "Lensfun").is_some());
    // An edit naming the lens only, as read from XMP, resolves on request, as one
    // step.
    let named = editor
        .document
        .edit
        .recipe()
        .lens_profile_choice
        .lensfun()
        .unwrap()
        .clone();
    editor
        .document
        .edit
        .setup_mut()
        .lens_profile_choice
        .choose_lensfun(crate::lens::lensfun::LensfunChoice::unresolved(
            named.lens.clone(),
            named.label.clone(),
            0,
        ));
    let tree = frame(&mut editor, vec![]);
    assert!(find(&tree, "isn't resolved").is_some());
    let steps = editor.document.edit.history().steps().0.len();
    let resolve = tree
        .nodes
        .iter()
        .map(|(_, n)| n)
        .find(|n| n.role() == egui::accesskit::Role::Button && n.label() == Some("Resolve"))
        .cloned()
        .expect("the Resolve button");
    let [down, up] = press_and_release(centre(&resolve));
    frame(&mut editor, down);
    frame(&mut editor, up);
    frame(&mut editor, vec![]);
    let chosen = editor
        .document
        .edit
        .recipe()
        .lens_profile_choice
        .lensfun()
        .unwrap()
        .clone();
    assert!(chosen.is_resolved());
    assert_eq!(chosen.calibration, named.calibration);
    assert_eq!(editor.document.edit.history().steps().0.len(), steps + 1);
    Ok(())
}
