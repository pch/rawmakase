//! The photo menu's layout: every shortcut fits beside its label (#979).
use super::{MenuCollections, MenuTarget, Presence, photo_menu};
use crate::app::library::Module;
use crate::catalog::Photo;
use eframe::egui::{self, Pos2, Rect, Vec2};

const SCREEN: f32 = 900.;

/// The text drawn in one frame, with where it landed.
type Texts = Vec<(String, Rect)>;

/// Right-clicks a photo whose cell is at `cell` and returns the text of
/// each frame from the one the menu opens on.
fn open_menu(cell: Rect, remove_from: Option<&'static str>) -> Vec<Texts> {
    let ctx = egui::Context::default();
    let photo = Photo {
        filename: "a.NEF".into(),
        path: "/photos/a.NEF".into(),
        ..Default::default()
    };
    let frame = |events: Vec<egui::Event>| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(SCREEN))),
                events,
                ..Default::default()
            },
            |ui| {
                let response = ui.put(cell, egui::Button::new("photo"));
                photo_menu(
                    &response,
                    &photo,
                    Presence {
                        available: true,
                        copy: false,
                    },
                    MenuTarget::Photo,
                    Module::Library,
                    true,
                    MenuCollections {
                        remove_from,
                        ..Default::default()
                    },
                );
            },
        );
        output.textures_delta.clear();
        output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Text(t) => {
                    Some((t.galley.text().to_owned(), t.visual_bounding_rect()))
                }
                _ => None,
            })
            .collect::<Texts>()
    };
    frame(vec![]);
    let at = cell.center();
    let press = |pressed| egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Secondary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    let mut frames = vec![frame(vec![
        egui::Event::PointerMoved(at),
        press(true),
        press(false),
    ])];
    frames.extend((0..3).map(|_| frame(vec![])));
    frames
}

fn find<'a>(texts: &'a Texts, text: &str) -> Option<&'a Rect> {
    texts.iter().find(|(t, _)| t == text).map(|(_, r)| r)
}

#[test]
fn shortcuts_never_overlap_their_labels() {
    let frames = open_menu(
        Rect::from_min_size(Pos2::new(5., 5.), Vec2::new(60., 20.)),
        None,
    );
    let drawn = frames.last().unwrap();
    let mut rows = 0;
    for label in [
        "Remove from Catalog…",
        "Move to Trash…",
        "Create Virtual Copy",
        "Export…",
        "Export with Previous",
    ] {
        let label_rect = find(drawn, label).unwrap_or_else(|| panic!("no {label} in {drawn:?}"));
        for (text, rect) in drawn {
            if text != label && (rect.center().y - label_rect.center().y).abs() < 4. {
                rows += 1;
                assert!(
                    rect.left() >= label_rect.right() + 8.,
                    "{label:?} at {label_rect:?} runs into {text:?} at {rect:?}"
                );
            }
        }
    }
    assert_eq!(rows, 5, "every row has its shortcut: {drawn:?}");
}

/// Opened near the window's right edge, the menu is placed for its full
/// width from the frame it first shows on: it neither runs off the window
/// nor jumps on the next frame.
#[test]
fn a_menu_opened_by_the_right_edge_fits_from_its_first_frame() {
    let frames = open_menu(
        Rect::from_min_size(Pos2::new(620., 20.), Vec2::new(60., 20.)),
        None,
    );
    let trash = super::TRASH_SHORTCUT;
    let shown: Vec<Rect> = frames
        .iter()
        .filter_map(|t| find(t, trash).copied())
        .collect();
    assert!(!shown.is_empty(), "the menu never showed");
    for rect in &shown {
        assert!(
            rect.right() <= SCREEN,
            "{trash:?} runs off the window at {rect:?}"
        );
        assert_eq!(rect, &shown[0], "the menu moved: {shown:?}");
    }
}

/// Remove from Collection shows, with Delete's shortcut beside it, only when
/// the source shown takes photos out (#1199).
#[test]
fn remove_from_collection_is_offered_only_when_the_source_takes_photos_out() {
    let cell = Rect::from_min_size(Pos2::new(5., 5.), Vec2::new(60., 20.));
    let frames = open_menu(cell, Some("Remove from Collection"));
    let drawn = frames.last().unwrap();
    let label = find(drawn, "Remove from Collection").expect("offered in a collection");
    let shortcut = find(drawn, super::REMOVE_FROM_COLLECTION_SHORTCUT).expect("its shortcut");
    assert!((shortcut.center().y - label.center().y).abs() < 4.);
    assert!(shortcut.left() >= label.right() + 8.);
    let frames = open_menu(cell, None);
    let drawn = frames.last().unwrap();
    assert!(
        find(drawn, "Remove from Catalog…").is_some(),
        "the menu showed"
    );
    assert!(find(drawn, "Remove from Collection").is_none());
}

/// A photo's menu, open, with Export… and Export with Previous (#1228), for
/// `RAWMAKASE_UI_SHOTS` ([`crate::app::ui::shots`]). The menu opens on a
/// right-click, which a shot can't make, so it is drawn here.
#[test]
fn shots_of_the_photo_menu() {
    shoot_photo_menu("photo_menu.png", &[], None);
}

/// The photo menu's Edit In, open, with two external editors and with none.
#[test]
fn shots_of_the_photo_menus_edit_in() {
    let editors = [
        "Adobe Photoshop 2026".to_string(),
        "Affinity Photo 2".to_string(),
    ];
    shoot_photo_menu("photo_menu_edit_in.png", &editors, Some("Edit In"));
    shoot_photo_menu("photo_menu_edit_in_empty.png", &[], Some("Edit In"));
}

/// Draws the photo menu with `editors`, `submenu` open, into `name`.
fn shoot_photo_menu(name: &str, editors: &[String], submenu: Option<&str>) {
    use egui_kittest::kittest::Queryable;
    let Some(dir) = std::env::var_os("RAWMAKASE_UI_SHOTS").map(std::path::PathBuf::from) else {
        return;
    };
    let photo = Photo {
        filename: "a.NEF".into(),
        path: "/photos/a.NEF".into(),
        ..Default::default()
    };
    let cell = Rect::from_min_size(Pos2::new(8., 8.), Vec2::new(60., 20.));
    let mut fonts = false;
    let mut harness = egui_kittest::Harness::builder()
        .with_size(Vec2::new(800., 880.))
        .with_pixels_per_point(2.)
        .wgpu()
        .build_ui(|ui| {
            if !std::mem::replace(&mut fonts, true) {
                let text = fastframe_text::detect();
                crate::app::install_fonts(ui.ctx(), &text);
                crate::app::ui::icons::install(ui.ctx());
                crate::app::ui::theme::apply(
                    ui.ctx(),
                    crate::app::ui::theme::Palette::DEFAULT,
                    &text,
                );
            }
            let response = ui.put(cell, egui::Button::new("photo"));
            photo_menu(
                &response,
                &photo,
                Presence {
                    available: true,
                    copy: false,
                },
                MenuTarget::Photo,
                Module::Library,
                true,
                MenuCollections {
                    editors,
                    ..Default::default()
                },
            );
        });
    harness.run_steps(2);
    let at = cell.center();
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
    if let Some(submenu) = submenu {
        harness.get_by_label(submenu).hover();
        harness.run_steps(4);
    }
    let image = harness.render().expect("wgpu could not draw the shot");
    std::fs::create_dir_all(&dir).expect("the shots folder");
    image.save(dir.join(name)).expect("the shot");
}
