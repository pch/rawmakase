//! The Profile menu's profiles: those the listing puts at the top level, in name
//! order, then a submenu for each group of looks, as Lightroom's profile browser
//! groups them. Where each profile goes is the listing's answer
//! ([`Groups::entry`]); this only lays the answers out.
use crate::camera_profiles::{CameraProfile, Groups, MenuEntry};
use eframe::egui;
use std::sync::Arc;

/// The menu's layout: the top level's profiles, then each group with its sections
/// (the one without a heading first) and their profiles.
#[derive(Debug, PartialEq)]
struct Layout<'a> {
    top: Vec<(String, &'a Arc<CameraProfile>)>,
    groups: Vec<Group<'a>>,
}
type Section<'a> = (Option<String>, Vec<(String, &'a Arc<CameraProfile>)>);
type Group<'a> = (String, Vec<Section<'a>>);

/// Groups and sections sort by name, ignoring case; profiles keep the listing's
/// order, which is by name.
fn layout<'a>(profiles: &'a [Arc<CameraProfile>], groups: &Groups) -> Layout<'a> {
    let mut top = Vec::new();
    let mut grouped: Vec<Group<'a>> = Vec::new();
    for profile in profiles {
        let MenuEntry {
            group,
            section,
            title,
        } = groups.entry(profile);
        let Some(group) = group else {
            top.push((title, profile));
            continue;
        };
        let at = match grouped.iter().position(|(name, _)| *name == group) {
            Some(at) => at,
            None => {
                grouped.push((group, Vec::new()));
                grouped.len() - 1
            }
        };
        let sections = &mut grouped[at].1;
        match sections.iter_mut().find(|(name, _)| *name == section) {
            Some((_, items)) => items.push((title, profile)),
            None => sections.push((section, vec![(title, profile)])),
        }
    }
    let key = |name: &str| (name.to_lowercase(), name.to_string());
    grouped.sort_by_key(|(name, _)| key(name));
    for (_, sections) in &mut grouped {
        sections.sort_by_key(|(name, _)| name.as_deref().map(key));
    }
    Layout {
        top,
        groups: grouped,
    }
}

/// The profiles in the Profile menu, choosing into `selected`. A group's submenu
/// is in bold when the profile chosen is in it.
pub(super) fn items(
    ui: &mut egui::Ui,
    selected: &mut Option<Arc<CameraProfile>>,
    profiles: &[Arc<CameraProfile>],
    groups: &Groups,
) {
    let Layout { top, groups } = layout(profiles, groups);
    for (title, profile) in top {
        choice(ui, selected, profile, title);
    }
    if !groups.is_empty() {
        ui.separator();
    }
    for (group, sections) in groups {
        let holds_selected = sections
            .iter()
            .flat_map(|(_, items)| items)
            .any(|(_, p)| selected.as_ref() == Some(*p));
        let mut label = egui::RichText::new(group);
        if holds_selected {
            label = label.strong();
        }
        ui.menu_button(label, |ui| {
            for (i, (section, items)) in sections.into_iter().enumerate() {
                if i > 0 {
                    ui.separator();
                }
                if let Some(section) = section {
                    ui.label(egui::RichText::new(section).small().weak());
                }
                for (title, profile) in items {
                    if choice(ui, selected, profile, title).clicked() {
                        ui.close();
                    }
                }
            }
        });
    }
}

fn choice(
    ui: &mut egui::Ui,
    selected: &mut Option<Arc<CameraProfile>>,
    profile: &Arc<CameraProfile>,
    title: String,
) -> egui::Response {
    ui.selectable_value(selected, Some(profile.clone()), title)
        .on_hover_text(&profile.copyright)
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{Pos2, Rect, Vec2};

    /// A library with a camera profile, a look without a group, Adobe's looks
    /// ("Profiles") and two packs of looks.
    fn library() -> crate::camera_profiles::Listed {
        let temp = tempfile::tempdir().unwrap();
        let library = temp.path().join("library");
        let mut picks = Vec::new();
        for (folder, name, shift) in [
            ("Pack B/Cube Files", "Warm", 0.1),
            ("Pack B/Cube Files", "Cool", 0.2),
            ("pack a", "Fade", 0.3),
            ("Pack B/Cube Files", "Portra", 0.4),
            ("Film", "Portra", 0.5),
        ] {
            let dir = temp.path().join(folder);
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join(format!("{name}.cube"));
            let mut text = String::from("LUT_3D_SIZE 2\n");
            for b in [0., 1.] {
                for g in [0., 1.] {
                    for r in [0f32, 1.] {
                        text += &format!("{} {g} {b}\n", (r + shift).min(1.));
                    }
                }
            }
            std::fs::write(&path, text).unwrap();
            picks.push(path);
        }
        crate::camera_profiles::import_into_dir(&picks, &library, Default::default()).unwrap();
        let photo = crate::camera_data::Metadata {
            make: "Test".into(),
            model: "Camera".into(),
            cam_xyz: [[0.8, -0.2, -0.1], [-0.3, 1.1, 0.2], [-0.05, 0.15, 0.6]],
            ..Default::default()
        };
        crate::camera_profiles::listed_from(&[library], &photo)
    }

    fn names<'a>(items: &[(String, &'a Arc<CameraProfile>)]) -> Vec<&'a str> {
        items.iter().map(|(_, p)| p.name.as_str()).collect()
    }

    #[test]
    fn looks_go_in_a_submenu_per_group_and_the_rest_stay_on_top() {
        let listed = library();
        let layout = layout(&listed.profiles, &listed.groups);
        assert_eq!(
            names(&layout.top),
            [
                crate::camera_profiles::open::COLOR,
                crate::camera_profiles::open::STANDARD
            ]
        );
        let groups: Vec<_> = layout
            .groups
            .iter()
            .map(|(name, sections)| {
                assert_eq!(sections.len(), 1);
                assert_eq!(sections[0].0, None);
                (name.as_str(), names(&sections[0].1))
            })
            .collect();
        assert_eq!(
            groups,
            [
                ("Film", vec!["Portra (Film)"]),
                ("pack a", vec!["Fade"]),
                ("Pack B", vec!["Cool", "Portra", "Warm"]),
            ]
        );
    }

    /// The labels painted on screen, with where they are.
    fn texts(shapes: &[egui::epaint::ClippedShape]) -> Vec<(String, Rect)> {
        fn walk(shape: &egui::Shape, out: &mut Vec<(String, Rect)>) {
            match shape {
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, out)),
                egui::Shape::Text(t) => {
                    out.push((t.galley.text().to_string(), t.visual_bounding_rect()))
                }
                _ => {}
            }
        }
        let mut out = Vec::new();
        shapes.iter().for_each(|s| walk(&s.shape, &mut out));
        out
    }

    /// Opening the menu, pointing at a group and clicking a look in its submenu
    /// chooses that look.
    #[test]
    fn a_look_is_chosen_from_its_groups_submenu() {
        let listed = library();
        let ctx = egui::Context::default();
        ctx.all_styles_mut(|s| s.animation_time = 0.);
        let mut selected: Option<Arc<CameraProfile>> = None;
        let mut shapes = Vec::new();
        let frame = |events: Vec<egui::Event>, selected: &mut Option<Arc<CameraProfile>>| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(600., 500.))),
                    events,
                    ..Default::default()
                },
                |ui| {
                    egui::ComboBox::from_id_salt("camera-profile")
                        .width(200.)
                        .selected_text("Profile")
                        .show_ui(ui, |ui| {
                            items(ui, selected, &listed.profiles, &listed.groups)
                        });
                },
            );
            output.textures_delta.clear();
            texts(&output.shapes)
        };
        let find = |shown: &[(String, Rect)], text: &str| {
            shown
                .iter()
                .find(|(t, _)| t == text)
                .map(|(_, r)| r.center())
                .unwrap_or_else(|| panic!("{text} not shown: {shown:?}"))
        };
        let click = |pos| {
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                },
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: Default::default(),
                },
            ]
        };
        shapes.extend(frame(vec![], &mut selected));
        let combo = find(&shapes, "Profile");
        frame(click(combo), &mut selected);
        let shown = frame(vec![], &mut selected);
        // A look is not on the top level, only its group.
        assert!(!shown.iter().any(|(t, _)| t == "Cool"), "{shown:?}");
        let group = find(&shown, "Pack B");
        frame(vec![egui::Event::PointerMoved(group)], &mut selected);
        let mut shown = frame(vec![], &mut selected);
        for _ in 0..3 {
            if shown.iter().any(|(t, _)| t == "Cool") {
                break;
            }
            shown = frame(vec![egui::Event::PointerMoved(group)], &mut selected);
        }
        let look = find(&shown, "Cool");
        // Into the submenu sideways, then a click on the look.
        frame(
            vec![egui::Event::PointerMoved(Pos2::new(look.x, group.y))],
            &mut selected,
        );
        frame(click(look), &mut selected);
        frame(vec![], &mut selected);
        assert_eq!(selected.as_ref().map(|p| p.name.as_str()), Some("Cool"));
        // The menu closed with the choice.
        let shown = frame(vec![], &mut selected);
        assert!(!shown.iter().any(|(t, _)| t == "Pack B"), "{shown:?}");
    }
}
