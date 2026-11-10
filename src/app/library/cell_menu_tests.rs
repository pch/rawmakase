//! The photo menu's layout: every shortcut fits beside its label (#979).
use super::photo_menu;
use crate::app::library::Module;
use crate::catalog::Photo;
use eframe::egui::{self, Pos2, Rect, Vec2};

const SCREEN: f32 = 900.;

/// The text drawn in one frame, with where it landed.
type Texts = Vec<(String, Rect)>;

/// Right-clicks a photo whose cell is at `cell` and returns the text of
/// each frame from the one the menu opens on.
fn open_menu(cell: Rect) -> Vec<Texts> {
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
                photo_menu(&response, &photo, true, true, Module::Library, true);
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
    let frames = open_menu(Rect::from_min_size(Pos2::new(5., 5.), Vec2::new(60., 20.)));
    let drawn = frames.last().unwrap();
    let mut rows = 0;
    for label in [
        "Remove from Catalog…",
        "Move to Trash…",
        "Create Virtual Copy",
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
    assert_eq!(rows, 3, "every row has its shortcut: {drawn:?}");
}

/// Opened near the window's right edge, the menu is placed for its full
/// width from the frame it first shows on: it neither runs off the window
/// nor jumps on the next frame.
#[test]
fn a_menu_opened_by_the_right_edge_fits_from_its_first_frame() {
    let frames = open_menu(Rect::from_min_size(
        Pos2::new(620., 20.),
        Vec2::new(60., 20.),
    ));
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
