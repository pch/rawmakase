//! Editor tests of `backdrop`: the menu a right click around the photo opens,
//! driven through the viewport's frames, and what the session keeps.
use super::Backdrop;
use crate::app::develop::state::Tool;
use crate::app::preferences::session::Session;
use crate::app::*;
use eframe::egui::{Pos2, Rect, Vec2};

/// Develop's photo view, as the workspace draws it: the backdrop's stage, then
/// the viewport inside its margin. Returns the accessibility tree.
fn develop_frame(
    ctx: &egui::Context,
    editor: &mut Editor,
    events: Vec<egui::Event>,
) -> egui::accesskit::TreeUpdate {
    // Held as the button is pressed, as the window reports it.
    let modifiers = events
        .iter()
        .find_map(|e| match e {
            egui::Event::PointerButton { modifiers, .. } => Some(*modifiers),
            _ => None,
        })
        .unwrap_or_default();
    let events = std::iter::once(egui::Event::ModifiersChanged(modifiers))
        .chain(events)
        .collect();
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(400.))),
            events,
            ..Default::default()
        },
        |ui| {
            let frame = editor.begin_edit_frame();
            let area = editor.backdrop_stage(ui, ui.max_rect());
            ui.scope_builder(egui::UiBuilder::new().max_rect(area), |ui| {
                editor.viewport_ui(ui)
            });
            editor.finish_edit_frame(frame, ui.ctx());
        },
    );
    output.textures_delta.clear();
    output
        .platform_output
        .accesskit_update
        .expect("accessibility is on")
}

fn button(at: Pos2, button: egui::PointerButton, pressed: bool) -> Vec<egui::Event> {
    vec![
        egui::Event::PointerMoved(at),
        egui::Event::PointerButton {
            pos: at,
            button,
            pressed,
            modifiers: egui::Modifiers::NONE,
        },
    ]
}

/// A right click at `at`, then a frame for the menu to show in.
fn right_click(ctx: &egui::Context, editor: &mut Editor, at: Pos2) -> egui::accesskit::TreeUpdate {
    develop_frame(
        ctx,
        editor,
        button(at, egui::PointerButton::Secondary, true),
    );
    develop_frame(
        ctx,
        editor,
        button(at, egui::PointerButton::Secondary, false),
    );
    develop_frame(ctx, editor, vec![])
}

fn left_click(ctx: &egui::Context, editor: &mut Editor, at: Pos2) {
    develop_frame(ctx, editor, button(at, egui::PointerButton::Primary, true));
    develop_frame(ctx, editor, button(at, egui::PointerButton::Primary, false));
}

/// An editor showing a photo in Develop, its session kept in `session`.
fn photo_editor(ctx: &egui::Context, session: Session, file: Option<std::path::PathBuf>) -> Editor {
    ctx.enable_accesskit();
    ctx.all_styles_mut(|s| s.animation_time = 0.);
    let mut editor = Editor::with_context(ctx, None, session, file);
    editor.module = Module::Develop;
    editor.document.set_image(crate::app::testing::blue_image());
    editor.preview.texture = Some(
        ctx.load_texture(
            "photo",
            egui::ColorImage::filled([32, 24], egui::Color32::GRAY),
            egui::TextureOptions::LINEAR,
        )
        .into(),
    );
    develop_frame(ctx, &mut editor, vec![]);
    editor
}

fn menu_shown(tree: &egui::accesskit::TreeUpdate) -> bool {
    crate::app::testing::labelled(tree, "Darker Gray").is_some()
}

#[test]
fn a_right_click_beside_the_photo_chooses_a_backdrop_kept_in_the_session() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("session.json");
    let ctx = egui::Context::default();
    let mut editor = photo_editor(&ctx, Session::default(), Some(file.clone()));
    // In the margin, outside the viewport, and beside the photo inside it.
    for at in [Pos2::new(4., 200.), Pos2::new(200., 60.)] {
        let tree = right_click(&ctx, &mut editor, at);
        assert!(menu_shown(&tree), "a right click at {at:?} opens the menu");
        develop_frame(
            &ctx,
            &mut editor,
            vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }],
        );
    }
    let tree = right_click(&ctx, &mut editor, Pos2::new(4., 200.));
    let white = crate::app::testing::labelled(&tree, "White").expect("White in the menu");
    left_click(&ctx, &mut editor, crate::app::testing::centre(white));
    assert_eq!(editor.view.backdrop, Backdrop::White);
    assert!(
        !editor.view.zoom.on,
        "choosing a color doesn't zoom the photo"
    );
    let saved: Session = serde_json::from_slice(&std::fs::read(&file)?)?;
    assert_eq!(saved.backdrop.as_deref(), Some("white"));
    let reopened = Editor::with_context(&ctx, None, saved, None);
    assert_eq!(reopened.view.backdrop, Backdrop::White);
    Ok(())
}

#[test]
fn the_photo_opens_the_menu_only_while_no_tool_takes_its_clicks() {
    let ctx = egui::Context::default();
    let mut editor = photo_editor(&ctx, Session::default(), None);
    let photo = editor.view.shown_rect.expect("the photo is shown");
    editor.view.set_tool(Tool::WhiteBalance);
    develop_frame(&ctx, &mut editor, vec![]);
    assert!(!menu_shown(&right_click(&ctx, &mut editor, photo.center())));
    editor.view.set_tool(Tool::None);
    develop_frame(&ctx, &mut editor, vec![]);
    assert!(menu_shown(&right_click(&ctx, &mut editor, photo.center())));
    assert!(!editor.view.zoom.on, "a right click doesn't zoom");
}

/// With a tool open, the menu opened beside the photo closes on a click on
/// the photo, and stays closed once the pointer leaves it.
#[test]
fn a_click_on_the_photo_closes_the_menu_while_a_tool_is_open() {
    let ctx = egui::Context::default();
    let mut editor = photo_editor(&ctx, Session::default(), None);
    let photo = editor.view.shown_rect.expect("the photo is shown");
    editor.view.set_tool(Tool::WhiteBalance);
    develop_frame(&ctx, &mut editor, vec![]);
    assert!(menu_shown(&right_click(
        &ctx,
        &mut editor,
        Pos2::new(4., 200.)
    )));
    left_click(&ctx, &mut editor, photo.center());
    develop_frame(
        &ctx,
        &mut editor,
        vec![egui::Event::PointerMoved(Pos2::new(4., 200.))],
    );
    assert!(!menu_shown(&develop_frame(&ctx, &mut editor, vec![])));
}

/// Every backdrop leaves the photo the same margin, so changing it never moves
/// or resizes the photo.
#[test]
fn every_backdrop_keeps_the_photo_where_it_is() {
    let ctx = egui::Context::default();
    let mut editor = photo_editor(&ctx, Session::default(), None);
    let fitted = editor.view.shown_rect.expect("the photo is shown");
    for backdrop in Backdrop::ALL {
        editor.set_backdrop(backdrop);
        develop_frame(&ctx, &mut editor, vec![]);
        assert_eq!(editor.view.shown_rect, Some(fitted), "{backdrop:?}");
    }
}

#[test]
fn the_session_keeps_no_backdrop_by_default_and_unknown_names_read_as_it() {
    let json = serde_json::to_value(Session::default()).unwrap();
    assert!(json.get("backdrop").is_none());
    let saved: Session = serde_json::from_str(r#"{"backdrop": "mauve"}"#).unwrap();
    let editor = Editor::with_context(&egui::Context::default(), None, saved, None);
    assert_eq!(editor.view.backdrop, Backdrop::Default);
    let saved: Session = serde_json::from_str(r#"{"backdrop": 7}"#).unwrap();
    assert_eq!(saved.backdrop, None);
}

/// On macOS a Control-click is a right click: it opens the menu over the
/// photo, and doesn't zoom it as a click would.
#[cfg(target_os = "macos")]
#[test]
fn a_control_click_on_the_photo_opens_the_menu_without_zooming() {
    let ctx = egui::Context::default();
    let mut editor = photo_editor(&ctx, Session::default(), None);
    let at = editor.view.shown_rect.expect("the photo is shown").center();
    let ctrl_click = |pressed| {
        let modifiers = egui::Modifiers {
            ctrl: true,
            ..Default::default()
        };
        vec![
            egui::Event::PointerMoved(at),
            egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers,
            },
        ]
    };
    develop_frame(&ctx, &mut editor, ctrl_click(true));
    develop_frame(&ctx, &mut editor, ctrl_click(false));
    assert!(menu_shown(&develop_frame(&ctx, &mut editor, vec![])));
    assert!(!editor.view.zoom.on);
}

/// Develop with a photo on each backdrop, the toolbar under it, and the menu
/// open over the margin, for `RAWMAKASE_UI_SHOTS` ([`crate::app::ui::shots`]).
/// The menu opens on a right click, which `shots::shot` can't make, so this
/// drives its own harness.
#[test]
fn shots_of_the_backdrop() {
    let Some(dir) = std::env::var_os("RAWMAKASE_UI_SHOTS").map(std::path::PathBuf::from) else {
        return;
    };
    std::fs::create_dir_all(&dir).expect("the shots folder");
    let size = Vec2::new(1200., 760.);
    for (name, backdrop, menu) in [
        ("backdrop-default", Backdrop::Default, false),
        ("backdrop-white", Backdrop::White, false),
        ("backdrop-light-gray", Backdrop::LightGray, false),
        ("backdrop-medium-gray", Backdrop::MediumGray, false),
        ("backdrop-black", Backdrop::Black, false),
        ("backdrop-menu", Backdrop::MediumGray, true),
    ] {
        let mut editor: Option<Editor> = None;
        let mut harness = egui_kittest::Harness::builder()
            .with_size(size)
            .with_pixels_per_point(2.)
            .wgpu()
            .build_ui(move |ui| {
                let editor = editor.get_or_insert_with(|| {
                    let text = fastframe_text::detect();
                    crate::app::install_fonts(ui.ctx(), &text);
                    crate::app::ui::icons::install(ui.ctx());
                    let mut e = Editor::with_context(ui.ctx(), None, Session::default(), None);
                    e.module = Module::Develop;
                    e.document.opened_at("/photos/Dunes.ARW".into());
                    e.document.set_image(crate::app::testing::blue_image());
                    // A photo of soft bands, light at the top, as a high-key sky.
                    let (w, h) = (300, 200);
                    let pixels = (0..w * h)
                        .map(|i| {
                            let (x, y) = ((i % w) as f32 / w as f32, (i / w) as f32 / h as f32);
                            let v = 245. - 120. * y;
                            egui::Color32::from_rgb(
                                v as u8,
                                (v - 10. * x) as u8,
                                (v - 40. * y) as u8,
                            )
                        })
                        .collect();
                    e.preview.texture = Some(
                        ui.ctx()
                            .load_texture(
                                "photo",
                                egui::ColorImage::new([w, h], pixels),
                                egui::TextureOptions::LINEAR,
                            )
                            .into(),
                    );
                    e.view.backdrop = backdrop;
                    e
                });
                editor.draw(ui);
            });
        harness.run_steps(6);
        if menu {
            // In the margin left of the photo.
            let at = Pos2::new(300., 300.);
            harness.event(egui::Event::PointerMoved(at));
            for pressed in [true, false] {
                harness.event(egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Secondary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                });
            }
            harness.run_steps(4);
        }
        let image = harness.render().expect("wgpu could not draw the shot");
        image
            .save(dir.join(format!("{name}.png")))
            .expect("the shot");
    }
}
