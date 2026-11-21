//! The whole app read as a screen reader reads it: every view, side panel and
//! dialog people reach is drawn, and its AccessKit tree must name every control,
//! give every slider a value and a range, and give every dialog a title.
//!
//! It reads the controls that have a role. A control painted by hand on an
//! area that only senses clicks (a slider's rail, a rating star, a keyword
//! tile) has no role, so a screen reader does not see it as a control at all;
//! this audit doesn't look for those.
use crate::app::testing::*;
use crate::app::*;
use eframe::egui::accesskit::{Node, NodeId, Role, TreeUpdate};
use eframe::egui::{Pos2, Rect};
use std::collections::HashMap;

/// The roles people operate, each of which needs a name.
const CONTROLS: &[Role] = &[
    Role::Button,
    Role::DefaultButton,
    Role::CheckBox,
    Role::Switch,
    Role::RadioButton,
    Role::Slider,
    Role::SpinButton,
    Role::ComboBox,
    Role::EditableComboBox,
    Role::TextInput,
    Role::MultilineTextInput,
    Role::SearchInput,
    Role::PasswordInput,
    Role::NumberInput,
    Role::MenuItem,
    Role::MenuItemCheckBox,
    Role::MenuItemRadio,
    Role::Link,
    Role::Tab,
    Role::ColorWell,
    Role::ListBoxOption,
    Role::TreeItem,
];

/// The tree of one frame of the whole window, `size` points large.
fn window_frame(editor: &mut Editor, size: egui::Vec2) -> TreeUpdate {
    let ctx = editor.context.clone();
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
            ..Default::default()
        },
        |ui| editor.draw(ui),
    );
    output.textures_delta.clear();
    output
        .platform_output
        .accesskit_update
        .expect("accessibility is on")
}

/// Draws `editor`'s window a few times, as a dialog takes a frame to size
/// itself, and returns the last frame's tree.
fn settled(editor: &mut Editor) -> TreeUpdate {
    editor.context.enable_accesskit();
    editor.context.all_styles_mut(|s| s.animation_time = 0.);
    let size = egui::vec2(1600., 1100.);
    for _ in 0..3 {
        window_frame(editor, size);
    }
    window_frame(editor, size)
}

/// The text a node shows or is called by: its label, or a label's value.
fn text(node: &Node) -> Option<&str> {
    node.label()
        .or(node.value())
        .map(str::trim)
        .filter(|t| !t.is_empty())
}

struct Tree<'a> {
    nodes: HashMap<NodeId, &'a Node>,
    parents: HashMap<NodeId, NodeId>,
    root: NodeId,
}
impl<'a> Tree<'a> {
    fn new(update: &'a TreeUpdate) -> Self {
        let nodes: HashMap<NodeId, &Node> = update.nodes.iter().map(|(id, n)| (*id, n)).collect();
        let parents = update
            .nodes
            .iter()
            .flat_map(|(id, n)| n.children().iter().map(move |c| (*c, *id)))
            .collect();
        let root = update.tree.as_ref().expect("a whole tree").root;
        Self {
            nodes,
            parents,
            root,
        }
    }
    /// The node's accessible name: its own label, else the text of the
    /// nodes it is labelled by.
    fn name(&self, node: &Node) -> Option<String> {
        if let Some(label) = node.label().map(str::trim).filter(|l| !l.is_empty()) {
            return Some(label.to_owned());
        }
        let by: Vec<&str> = node
            .labelled_by()
            .iter()
            .filter_map(|id| self.nodes.get(id).and_then(|n| text(n)))
            .collect();
        (!by.is_empty()).then(|| by.join(" "))
    }
    /// The child of the root holding `id`: the layer (panel, popup or dialog)
    /// it is drawn in.
    fn layer(&self, mut id: NodeId) -> NodeId {
        while let Some(parent) = self.parents.get(&id) {
            if *parent == self.root {
                break;
            }
            id = *parent;
        }
        id
    }
    /// Where a node sits, to find it by: the nearest ancestor with text, and
    /// the text of its layer shown on its row (the row's first and the
    /// nearest to its left) or else above it.
    fn context(&self, id: NodeId) -> String {
        let mut ancestor = None;
        let mut at = id;
        while let Some(parent) = self.parents.get(&at) {
            if *parent == self.root {
                break;
            }
            if let Some(t) = self.nodes.get(parent).and_then(|n| text(n)) {
                ancestor = Some(t.to_owned());
                break;
            }
            at = *parent;
        }
        let mut place = Vec::new();
        if let Some(a) = ancestor {
            place.push(format!("in {a:?}"));
        }
        place.push(self.nearby_text(id));
        place.join(", ")
    }
    fn nearby_text(&self, id: NodeId) -> String {
        let Some(b) = self.nodes[&id].bounds() else {
            return "unplaced".into();
        };
        let middle = (b.y0 + b.y1) / 2.;
        let layer = self.layer(id);
        let texts: Vec<(&str, egui::accesskit::Rect)> = self
            .nodes
            .iter()
            .filter(|(other, _)| **other != id && self.layer(**other) == layer)
            .filter_map(|(_, n)| Some((text(n)?, n.bounds()?)))
            .filter(|(t, _)| t.len() < 60)
            .collect();
        let mut row: Vec<_> = texts
            .iter()
            .filter(|(_, o)| o.y0 <= middle && middle <= o.y1 && o.x1 <= b.x0 + 1.)
            .collect();
        row.sort_by(|p, q| p.1.x0.total_cmp(&q.1.x0));
        match (row.first(), row.last()) {
            (Some(first), Some(last)) if first.0 != last.0 => {
                format!("on the row of {:?}, after {:?}", first.0, last.0)
            }
            (Some(first), _) => format!("after {:?}", first.0),
            _ => texts
                .iter()
                .filter(|(_, o)| o.y1 <= b.y0 + 1. && o.x0 < b.x1 && b.x0 < o.x1)
                .min_by(|p, q| (b.y0 - p.1.y1).total_cmp(&(b.y0 - q.1.y1)))
                .map_or("with no text near".into(), |(t, _)| format!("below {t:?}")),
        }
    }
}

fn bounds(node: &Node) -> String {
    node.bounds().map_or("unplaced".into(), |b| {
        format!("({:.0}, {:.0})–({:.0}, {:.0})", b.x0, b.y0, b.x1, b.y1)
    })
}

/// A control the audit knows is unnamed, with the reason it stays so: in one
/// of `views`, of `role`, showing `showing`.
struct Allowed {
    views: &'static [&'static str],
    role: Role,
    showing: &'static str,
    #[expect(dead_code, reason = "read by people, not the audit")]
    why: &'static str,
}
/// An [`egui::Slider`] showing its value draws the value's field itself and
/// returns only the slider's response, so the field can't be named from
/// outside; naming it means drawing the slider without its value and the
/// field beside it, a layout change left for later.
const SLIDER_VALUE: &str = "egui::Slider's own value field, which egui gives no handle to";
const ALLOWED: &[Allowed] = &[
    Allowed {
        // And under the dialogs that open over Export.
        views: &["Export", "Watermark Editor", "Filename Template Editor"],
        role: Role::SpinButton,
        showing: "92",
        why: SLIDER_VALUE,
    },
    Allowed {
        views: &["Auto-Stack"],
        role: Role::SpinButton,
        showing: "0:01:00",
        why: SLIDER_VALUE,
    },
];

/// What is wrong in the tree of `view`: unnamed controls, sliders without a
/// value or range, and, for a dialog, a missing title `window`.
fn audit(view: &str, update: &TreeUpdate, window: Option<&str>) -> Vec<(String, String)> {
    let tree = Tree::new(update);
    let mut found = Vec::new();
    for (id, node) in &update.nodes {
        let role = node.role();
        if !CONTROLS.contains(&role) {
            continue;
        }
        let context = tree.context(*id);
        let allowed = || {
            ALLOWED.iter().any(|a| {
                a.views.contains(&view) && a.role == role && node.value() == Some(a.showing)
            })
        };
        if tree.name(node).is_none() && !allowed() {
            found.push((
                view.to_owned(),
                format!(
                    "{role:?}{} at {} has no name, {context}",
                    node.value()
                        .filter(|v| !v.trim().is_empty())
                        .map_or(String::new(), |v| format!(" showing {v:?}")),
                    bounds(node)
                ),
            ));
        }
        if role == Role::Slider
            && (node.numeric_value().is_none()
                || node.min_numeric_value().is_none()
                || node.max_numeric_value().is_none())
        {
            found.push((
                view.to_owned(),
                format!(
                    "Slider {:?} at {} lacks a value or range (value {:?}, {:?}–{:?})",
                    tree.name(node).unwrap_or_default(),
                    bounds(node),
                    node.numeric_value(),
                    node.min_numeric_value(),
                    node.max_numeric_value()
                ),
            ));
        }
    }
    if let Some(title) = window {
        let titled = update.nodes.iter().any(|(id, n)| {
            *id != tree.root
                && matches!(n.role(), Role::Window | Role::Dialog)
                && n.label().is_some_and(|l| l.contains(title))
        });
        if !titled {
            found.push((
                view.to_owned(),
                format!("no window or dialog titled {title:?}"),
            ));
        }
    }
    found.sort();
    found
}

/// The view the dialogs open over, audited by [`library_views_name_every_control`].
const BEHIND: &str = "behind";

/// The Library grid's findings, as [`BEHIND`], so a dialog's audit lists only
/// what the dialog adds.
fn behind() -> Vec<(String, String)> {
    let (_dir, mut e, _) = catalog_editor(2);
    audit(BEHIND, &settled(&mut e), None)
}

/// Asserts that nothing was found. A control drawn in several views (the
/// Library behind a dialog) is listed once, under the first view showing it;
/// one first found in [`BEHIND`] is left to the test of that view.
fn assert_clean(found: Vec<(String, String)>) {
    let mut seen = std::collections::HashSet::new();
    let lines: Vec<String> = found
        .into_iter()
        .filter(|(_, what)| seen.insert(what.clone()))
        .filter(|(view, _)| view != BEHIND)
        .map(|(view, what)| format!("{view}: {what}"))
        .collect();
    assert!(
        lines.is_empty(),
        "{} accessibility findings:\n{}",
        lines.len(),
        lines.join("\n")
    );
}

/// An editor with a catalog of `count` small JPEGs, real images so the views'
/// previews arrive, its Library open, all photos selected, and no onboarding.
fn catalog_editor(count: usize) -> (tempfile::TempDir, Editor, Vec<crate::catalog::PhotoId>) {
    assert!(
        crate::export_settings::ExportSettings::load().is_none(),
        "The accessibility audit needs an empty data folder: run it with \
         RAWMAKASE_DATA_DIR and XDG_DATA_HOME set to an empty temporary folder."
    );
    let dir = tempfile::tempdir().expect("a temporary folder");
    let photos = dir.path().join("photos");
    std::fs::create_dir(&photos).expect("a photo folder");
    for i in 0..count {
        let shade = (40 * i) as u8;
        image::RgbImage::from_pixel(12, 8, image::Rgb([shade, 128, 255 - shade]))
            .save(photos.join(format!("{i}.jpg")))
            .expect("a JPEG");
    }
    let path = dir.path().join("audit.rawmakase");
    crate::catalog::Catalog::create(&path)
        .and_then(|mut c| c.add_folder(&photos).map(|_| ()))
        .expect("a catalog");
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    let library = crate::app::library::Library::load(&path, ctx).expect("the catalog loads");
    let mut listed: Vec<_> = library
        .session
        .photos()
        .iter()
        .map(|p| (p.filename.clone(), p.id))
        .collect();
    listed.sort();
    let ids: Vec<_> = listed.into_iter().map(|(_, id)| id).collect();
    e.library = Some(Box::new(library));
    e.module = Module::Library;
    e.onboarding.visible = false;
    e.context.enable_accesskit();
    let library = e.library.as_mut().unwrap();
    library.select(Some(ids[0]));
    library.select_range_to(ids[count - 1]);
    (dir, e, ids)
}

/// Opens photo `id` in Develop, decoded, without a worker.
fn open_in_develop(e: &mut Editor, id: crate::catalog::PhotoId) {
    let image = blue_image();
    e.module = Module::Develop;
    e.document.metadata = Some(image.metadata.clone());
    e.document.catalog_photo = Some(id);
    e.document.set_image(image);
    e.loupe_tried = Some(id);
}

#[test]
fn library_views_name_every_control() {
    let mut found = Vec::new();
    let (_dir, mut e, _) = catalog_editor(3);
    found.extend(audit("Library grid", &settled(&mut e), None));
    e.library.as_mut().unwrap().open_loupe();
    // The Loupe draws its photo once a worker has read it.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while e.library.as_ref().unwrap().loupe_fit().is_none() {
        assert!(
            std::time::Instant::now() < deadline,
            "the Loupe's photo never came"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
        window_frame(&mut e, egui::vec2(1600., 1100.));
    }
    found.extend(audit("Loupe", &settled(&mut e), None));
    e.library.as_mut().unwrap().open_compare();
    found.extend(audit("Compare", &settled(&mut e), None));
    e.library.as_mut().unwrap().open_survey();
    found.extend(audit("Survey", &settled(&mut e), None));
    assert_clean(found);
}

#[test]
fn develop_panels_name_every_control() {
    let (_dir, mut e, ids) = catalog_editor(2);
    open_in_develop(&mut e, ids[0]);
    // Every panel starts expanded in a new session; the window is tall
    // enough for the whole inspector.
    let mut found = audit("Develop, Presets tab", &settled(&mut e), None);
    e.set_develop_tab(crate::app::shell::panels::DevelopTab::History);
    found.extend(audit("Develop, History tab", &settled(&mut e), None));
    e.document.edit.setup_mut().effects.monochrome = true;
    found.extend(audit("Develop, black and white", &settled(&mut e), None));
    // Over the Develop panels audited above.
    e.open_copy_dialog(crate::app::develop::settings_transfer::Transfer::Copy);
    found.extend(audit(
        "Copy Settings",
        &settled(&mut e),
        Some("Copy Settings"),
    ));
    assert_clean(found);
}

#[test]
fn develop_tools_name_every_control() {
    use crate::app::develop::state::Tool;
    use crate::model::masks::{MaskComponent, MaskGroup, MaskShape};
    let mut found = Vec::new();
    let fresh = || {
        let (dir, mut e, ids) = catalog_editor(1);
        open_in_develop(&mut e, ids[0]);
        (dir, e)
    };
    {
        let (_dir, mut e) = fresh();
        e.document.edit.setup_mut().crop = [0.1, 0.2, 0.5, 0.6];
        e.view.toggle(Tool::Crop);
        found.extend(audit("Crop", &settled(&mut e), None));
    }
    {
        let (_dir, mut e) = fresh();
        e.document
            .edit
            .setup_mut()
            .retouch
            .push(crate::model::retouch::RetouchOp {
                mode: crate::model::retouch::RetouchMode::Heal,
                shape: crate::model::retouch::RetouchShape::Spot {
                    center: [0.3, 0.4],
                    radius: 0.05,
                },
                feather: 0.5,
                opacity: 1.,
                offset: [0.2, 0.],
                patch: None,
            });
        e.view.toggle(Tool::Remove);
        e.view.retouch.selected = Some(0);
        found.extend(audit("Remove", &settled(&mut e), None));
    }
    {
        let (_dir, mut e) = fresh();
        e.document
            .edit
            .setup_mut()
            .red_eye
            .push(crate::model::red_eye::RedEyeOp {
                kind: Default::default(),
                center: [0.5, 0.5],
                radius: [0.05, 0.05],
                correlation: 0.,
                pupil_size: 0.5,
                darken: 0.5,
            });
        e.view.toggle(Tool::RedEye);
        e.view.red_eye.selected = Some(0);
        found.extend(audit("Red Eye", &settled(&mut e), None));
    }
    for (view, shape) in [
        (
            "Masking, brush",
            MaskShape::Brush {
                strokes: Vec::new(),
            },
        ),
        (
            "Masking, linear gradient",
            MaskShape::Linear {
                from: [0.2, 0.5],
                to: [0.8, 0.5],
            },
        ),
        (
            "Masking, radial gradient",
            MaskShape::Radial {
                center: [0.5, 0.5],
                radii: [0.2, 0.1],
                angle: 0.,
                feather: 0.5,
            },
        ),
    ] {
        let (_dir, mut e) = fresh();
        e.view.toggle(Tool::Mask);
        e.document.edit.setup_mut().masks = vec![MaskGroup {
            components: vec![MaskComponent::new(shape)],
            ..Default::default()
        }];
        e.view.masking.selected = Some(0);
        e.view.masking.component = Some(0);
        found.extend(audit(view, &settled(&mut e), None));
    }
    assert_clean(found);
}

#[test]
fn preferences_pages_name_every_control() {
    use crate::app::preferences::Tab;
    let mut found = behind();
    for (name, tab) in [
        ("General", Tab::General),
        ("Catalog", Tab::Catalog),
        ("Profiles", Tab::Profiles),
        ("Performance", Tab::Performance),
        ("Display", Tab::Display),
        ("Shortcuts", Tab::Shortcuts),
        ("Extensions", Tab::Extensions),
    ] {
        let (_dir, mut e, _) = catalog_editor(1);
        e.open_preferences(tab);
        found.extend(audit(
            &format!("Preferences, {name}"),
            &settled(&mut e),
            Some("Preferences"),
        ));
    }
    let (_dir, mut e, _) = catalog_editor(1);
    e.open_extension_page("rawmakase.mcp");
    found.extend(audit(
        "Preferences, an extension's page",
        &settled(&mut e),
        Some("Preferences"),
    ));
    assert_clean(found);
}

#[test]
fn dialogs_name_every_control_and_their_window() {
    use crate::platform::menu::Action;
    let mut found = behind();
    let mut check =
        |view: &str, title: &str, open: &dyn Fn(&mut Editor, &[crate::catalog::PhotoId])| {
            let (_dir, mut e, ids) = catalog_editor(2);
            open(&mut e, &ids);
            found.extend(audit(view, &settled(&mut e), Some(title)));
        };
    check("Export", "Export", &|e, _| e.open_export_dialog());
    check("Watermark Editor", "Watermark Editor", &|e, _| {
        e.open_export_dialog();
        let ctx = e.context.clone();
        e.open_watermark_editor(&ctx);
    });
    check(
        "Filename Template Editor",
        "Filename Template Editor",
        &|e, _| {
            e.open_export_dialog();
            e.open_name_editor("{{image_name}}");
        },
    );
    check("Import Photos", "Import Photos", &|e, _| {
        e.request_import();
        let ctx = e.context.clone();
        settle_catalog(e, &ctx);
        e.events(&ctx);
    });
    check("Smart Collection editor", "Smart Collection", &|e, _| {
        e.library.as_mut().unwrap().new_smart_collection();
    });
    check("New Collection", "Collection", &|e, _| {
        e.library
            .as_mut()
            .unwrap()
            .ask_create_collection(false, None);
    });
    check("Rename Collection", "Rename", &|e, _| {
        let library = e.library.as_mut().unwrap();
        let id = library
            .create_collection(false, "Audit", None, &[], false)
            .expect("a collection");
        library.ask_rename_collection(id);
    });
    check("Merge to HDR", "HDR", &|e, ids| {
        e.open_merge_hdr(ids.to_vec())
    });
    check("Manage Presets", "Manage Presets", &|e, _| {
        e.modal = Some(Modal::ManagePresets(Default::default()));
    });
    check("Keyboard Shortcuts", "Keyboard Shortcuts", &|e, _| {
        e.view.shortcuts = true;
    });
    check("Report an Issue", "Report an Issue", &|e, _| {
        e.open_report_issue()
    });
    check("Auto-Stack", "Stack", &|e, _| {
        let ctx = e.context.clone();
        e.menu_action(Action::AutoStack, &ctx);
    });
    #[cfg(not(windows))]
    check("Rename Photos", "Rename", &|e, ids| {
        e.request_rename(ids.to_vec());
        let ctx = e.context.clone();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while e.file_ops.state_name() != "Renaming" {
            assert!(
                std::time::Instant::now() < deadline,
                "Rename Photos never opened"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
            e.events(&ctx);
            window_frame(e, egui::vec2(1600., 1100.));
        }
    });
    assert_clean(found);
}
