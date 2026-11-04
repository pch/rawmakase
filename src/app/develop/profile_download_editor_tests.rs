//! Editor tests of `profile_download`: the whole editor driven through its frames.
use crate::app::testing::*;
use crate::app::*;

/// Opens the Profile menu; returns the frame showing it.
fn open_profile_menu(ctx: &egui::Context, editor: &mut Editor) -> egui::accesskit::TreeUpdate {
    let tree = controls_frame(ctx, editor, vec![]);
    let profile = tree
        .nodes
        .iter()
        .map(|(_, n)| n)
        .find(|n| {
            n.role() == egui::accesskit::Role::ComboBox
                && n.value() == Some("Default (camera matrix)")
        })
        .expect("Profile menu");
    let [down, up] = press_and_release(centre(profile));
    controls_frame(ctx, editor, down);
    controls_frame(ctx, editor, up);
    controls_frame(ctx, editor, vec![])
}

#[test]
fn the_profile_menu_offers_community_profiles_only_for_covered_raw_cameras() {
    for (model, rendered, expected) in [
        ("X-T20", false, true),
        ("GFX100", false, true),
        ("X-T5", false, false),
        ("X-T20", true, false),
    ] {
        let ctx = egui::Context::default();
        let mut editor = community_editor(&ctx, model, rendered);
        let menu = open_profile_menu(&ctx, &mut editor);
        if !rendered {
            assert!(labelled(&menu, "Import profiles…").is_some());
        }
        assert_eq!(
            labelled(&menu, "Show community profiles for this camera").is_some(),
            expected,
            "{model}, rendered {rendered}"
        );
        assert_eq!(
            labelled(
                &menu,
                crate::app::develop::panels::inspector::profile_offer_available()
            )
            .is_some(),
            expected,
            "the offer for {model}, rendered {rendered}"
        );
    }
}

#[test]
fn the_community_offer_sits_under_the_profile_menu_dismisses_and_returns_from_the_menu() {
    let ctx = egui::Context::default();
    let mut editor = community_editor(&ctx, "X-T20", false);
    controls_frame(&ctx, &mut editor, vec![]);
    let tree = controls_frame(&ctx, &mut editor, vec![]);
    let profile = tree
        .nodes
        .iter()
        .map(|(_, n)| n)
        .find(|n| {
            n.role() == egui::accesskit::Role::ComboBox
                && n.value() == Some("Default (camera matrix)")
        })
        .expect("Profile menu");
    let offer = labelled(
        &tree,
        crate::app::develop::panels::inspector::profile_offer_available(),
    )
    .expect("the offer");
    let (menu, offer_at) = (profile.bounds().unwrap(), offer.bounds().unwrap());
    // Profile Amount's value is the first spin button under the menu.
    let amount_at = tree
        .nodes
        .iter()
        .filter_map(|(_, n)| {
            (n.role() == egui::accesskit::Role::SpinButton)
                .then(|| n.bounds())
                .flatten()
        })
        .filter(|b| b.y0 >= menu.y1)
        .min_by(|a, b| a.y0.total_cmp(&b.y0))
        .expect("Profile Amount");
    assert!(menu.y1 <= offer_at.y0 && offer_at.y1 <= amount_at.y0);
    // Who made it, on what terms, and where it is from.
    assert!(labelled(&tree, "ART contributors (Alberto Griggio and others)").is_some());
    assert!(labelled(&tree, "Public domain").is_some());
    // Dismissed, it stays away on the next frames…
    let dismiss = labelled(&tree, "Dismiss").expect("Dismiss").clone();
    click(&ctx, &mut editor, &dismiss);
    let tree = controls_frame(&ctx, &mut editor, vec![]);
    assert!(
        labelled(
            &tree,
            crate::app::develop::panels::inspector::profile_offer_available()
        )
        .is_none()
    );
    // …until the Profile menu asks for it.
    let menu = open_profile_menu(&ctx, &mut editor);
    let show = labelled(&menu, "Show community profiles for this camera")
        .expect("still in the menu")
        .clone();
    click(&ctx, &mut editor, &show);
    let tree = controls_frame(&ctx, &mut editor, vec![]);
    assert!(
        labelled(
            &tree,
            crate::app::develop::panels::inspector::profile_offer_available()
        )
        .is_some()
    );
    assert!(editor.document.edit.history().steps().0.is_empty());
}

#[test]
fn a_running_community_download_shows_its_progress_and_cancels_from_the_offer() {
    let ctx = egui::Context::default();
    let mut editor = community_editor(&ctx, "X-T20", false);
    let tree = controls_frame(&ctx, &mut editor, vec![]);
    assert!(labelled(&tree, "Download & Apply").is_some());
    editor.download_waiting_for_cancel();
    let tree = controls_frame(&ctx, &mut editor, vec![]);
    let cancel = labelled(&tree, "Cancel").expect("Cancel").clone();
    assert!(
        !tree
            .nodes
            .iter()
            .any(|(_, n)| n.label().is_some_and(|l| l.starts_with("Download & Apply")))
    );
    click(&ctx, &mut editor, &cancel);
    assert!(!editor.profile_download.task.is_running());
    let tree = controls_frame(&ctx, &mut editor, vec![]);
    assert!(labelled(&tree, "Cancel").is_none());
    assert!(
        labelled(
            &tree,
            crate::app::develop::panels::inspector::profile_offer_available()
        )
        .is_some()
    );
}

/// The community profile offer under Basic › Profile in each state, at the right
/// panel's width, written to `target/community-offer-*.png` to look at.
#[test]
#[ignore = "writes images to look at"]
fn render_the_community_offer() -> anyhow::Result<()> {
    use crate::camera_profiles::catalogue::{Status, catalogue};
    type Setup = fn(&mut Editor);
    let states: [(&str, &str, Setup); 5] = [
        ("available", "X-T3", |_| {}),
        ("downloading", "X-T3", |e| e.download_waiting_for_cancel()),
        ("installed", "X-T3", |e| {
            let m = e.document.metadata.clone().unwrap();
            let entries = catalogue().matching(&m);
            e.assume_catalogue_profiles(
                entries
                    .into_iter()
                    .map(|en| (en, Status::Installed("/nowhere.dcp".into())))
                    .collect(),
            );
        }),
        ("conflict", "X-T3", |e| {
            let m = e.document.metadata.clone().unwrap();
            let entries = catalogue().matching(&m);
            e.assume_catalogue_profiles(
                entries
                    .into_iter()
                    .map(|en| (en, Status::Conflict))
                    .collect(),
            );
        }),
        ("narrow", "X-T3", |_| {}),
    ];
    for (name, model, setup) in states {
        let ctx = egui::Context::default();
        crate::app::install_fonts(&ctx, &fastframe_text::TextRendering::platform_default());
        crate::app::ui::icons::install(&ctx);
        let mut e = community_editor(&ctx, model, false);
        setup(&mut e);
        let width = if name == "narrow" { 260. } else { 320. };
        let mut textures = Default::default();
        let image = crate::app::ui::ui_snapshot::render(
            &ctx,
            &mut textures,
            Vec2::new(width, 330.),
            2.,
            4,
            |ui| {
                let frame = e.begin_edit_frame();
                egui::CentralPanel::default().show(ui, |ui| e.controls(ui));
                e.finish_edit_frame(frame, ui.ctx());
            },
        );
        let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("target/community-offer-{name}.png"));
        image.save(&out)?;
        eprintln!("{}", out.display());
    }
    Ok(())
}
