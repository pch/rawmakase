//! What the keyboard shortcut pins share: an editor in each place a shortcut can
//! be pressed, set up so that every shortcut there has something to act on, whole
//! frames sent as a keyboard sends them, and a fingerprint of everything a
//! shortcut can change.
//!
//! The pins hold today's behaviour still while the shortcuts move into one
//! command registry (#963): `keymap_pins_editor_tests` one shortcut at a time,
//! `keymap_golden_editor_tests` every key in every place against a checked-in
//! table.
use crate::app::develop::state::Tool;
use crate::app::*;
use crate::catalog::PhotoId;
use eframe::egui::{self, Key, Modifiers, Pos2, Rect};
use std::sync::Arc;

/// The photos in each place's catalog.
pub(super) const PHOTOS: usize = 8;

/// The message every place starts with, for Shift+Esc to dismiss.
pub(super) const MESSAGE: &str = "A message for Shift+Esc to dismiss";

/// Where a shortcut is pressed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Place {
    Grid,
    Loupe,
    Compare,
    Survey,
    Develop,
    Crop,
    Remove,
    RedEye,
    Mask,
    Guided,
    /// The Loupe with a menu open over it.
    LoupePopup,
    /// Develop with a menu open over it.
    DevelopPopup,
    /// The grid while a text field has the keyboard.
    GridTyping,
    DevelopTyping,
    /// The grid with a button holding the keyboard focus.
    GridButton,
    DevelopButton,
    /// The grid behind Preferences.
    GridModal,
    DevelopModal,
}

impl Place {
    pub(super) const ALL: [Place; 18] = [
        Place::Grid,
        Place::Loupe,
        Place::Compare,
        Place::Survey,
        Place::Develop,
        Place::Crop,
        Place::Remove,
        Place::RedEye,
        Place::Mask,
        Place::Guided,
        Place::LoupePopup,
        Place::DevelopPopup,
        Place::GridTyping,
        Place::DevelopTyping,
        Place::GridButton,
        Place::DevelopButton,
        Place::GridModal,
        Place::DevelopModal,
    ];
    fn develop(self) -> bool {
        matches!(
            self,
            Place::Develop
                | Place::Crop
                | Place::Remove
                | Place::RedEye
                | Place::Mask
                | Place::Guided
                | Place::DevelopPopup
                | Place::DevelopTyping
                | Place::DevelopButton
                | Place::DevelopModal
        )
    }
    fn surround(self) -> Surround {
        match self {
            Place::LoupePopup | Place::DevelopPopup => Surround::Popup,
            Place::GridTyping | Place::DevelopTyping => Surround::Typing,
            Place::GridButton | Place::DevelopButton => Surround::Button,
            _ => Surround::Nothing,
        }
    }
}

/// What is drawn around the editor in each frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Surround {
    Nothing,
    /// A menu, drawn after the editor as a widget's menu is.
    Popup,
    /// A text field holding the keyboard, drawn before the editor.
    Typing,
    /// A button holding the keyboard focus, drawn before the editor.
    Button,
}

fn popup_id() -> egui::Id {
    egui::Id::new("keymap-pins-popup")
}

/// The modifiers for `[cmd, ctrl, alt, shift]` held, as egui-winit reports them
/// on the platform the tests run on. Cmd is the command key: Cmd on macOS, where
/// Control is a key of its own, and Ctrl elsewhere, where Control is the command
/// key too.
pub(super) fn modifiers([cmd, ctrl, alt, shift]: [bool; 4]) -> Modifiers {
    let mac = cfg!(target_os = "macos");
    Modifiers {
        alt,
        ctrl: ctrl || (cmd && !mac),
        shift,
        mac_cmd: cmd && mac,
        command: if mac { cmd } else { cmd || ctrl },
    }
}

/// The command key, with `extra` held too.
pub(super) fn command(extra: Modifiers) -> Modifiers {
    modifiers([true, false, false, false]) | extra
}

/// An editor in a place.
pub(super) struct Fixture {
    /// The catalog file this editor has open, a copy of the template's.
    catalog: std::path::PathBuf,
    /// The template's folder, when the fixture is the only one built from it.
    _dir: Option<tempfile::TempDir>,
    pub(super) e: Editor,
    pub(super) ids: Vec<PhotoId>,
    pub(super) surround: Surround,
    /// What the text field holds, while one has the keyboard.
    pub(super) field: String,
}

/// A key event as egui-winit sends it.
pub(super) fn key_event(key: Key, physical: Key, pressed: bool, m: Modifiers) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: Some(physical),
        pressed,
        repeat: false,
        modifiers: m,
    }
}

/// A press and release of `key` with `m` held, then let go: one tap.
pub(super) fn tap_events(key: Key, physical: Key, m: Modifiers) -> Vec<egui::Event> {
    vec![
        egui::Event::ModifiersChanged(m),
        key_event(key, physical, true, m),
        key_event(key, physical, false, m),
        egui::Event::ModifiersChanged(Modifiers::NONE),
    ]
}

/// A small photo of colors spread along blue, so Auto and the Remove tool's
/// source search find something.
fn blue_photo() -> Arc<crate::camera_data::CameraImage> {
    let (width, height) = (48u32, 32u32);
    let metadata = crate::camera_data::Metadata {
        width,
        height,
        wb: [1.; 3],
        daylight_wb: [1.; 3],
        matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        cam_xyz: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        ..Default::default()
    };
    Arc::new(crate::camera_data::CameraImage {
        recovered: Default::default(),
        width,
        height,
        pixels: (0..width * height)
            .map(|i| {
                let x = (i % width) as f32 / width as f32;
                let y = (i / width) as f32 / height as f32;
                let v = 0.05 + 0.3 * x + 0.2 * y * y;
                [v, 0.8 * v, 2. * v]
            })
            .collect(),
        metadata,
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
        hdr: Default::default(),
    })
}

/// Opens the photo `id` as Develop's document, decoded, without a worker.
fn open_decoded(e: &mut Editor, id: PhotoId) {
    let image = blue_photo();
    e.document.metadata = Some(image.metadata.clone());
    e.document.catalog_photo = Some(id);
    e.document.set_image(image);
    e.loupe_tried = Some(id);
}

/// What every editor of one place is built from: a catalog of [`PHOTOS`] small
/// JPEGs, copied for each editor, and one egui context, its memory cleared for
/// each. A new context would lay out its fonts again, the slowest part of a
/// first frame. Real images, so the previews the views ask workers for arrive
/// rather than fail at a moment no frame can predict.
pub(super) struct Template {
    dir: tempfile::TempDir,
    catalog: std::path::PathBuf,
    ctx: egui::Context,
    ids: Vec<PhotoId>,
    built: usize,
}

impl Template {
    pub(super) fn new() -> Self {
        // Export with Previous runs the last export's settings, kept in the data
        // folder: here they must be none, so it opens the dialog rather than
        // writing files wherever they say.
        assert!(
            crate::export_settings::ExportSettings::load().is_none(),
            "The keyboard shortcut pins need an empty data folder: run them with \
             RAWMAKASE_DATA_DIR and XDG_DATA_HOME set to an empty temporary folder."
        );
        let dir = tempfile::tempdir().expect("a temporary folder");
        let photos = dir.path().join("photos");
        std::fs::create_dir(&photos).expect("a photo folder");
        for i in 0..PHOTOS {
            let shade = (30 * i) as u8;
            image::RgbImage::from_pixel(12, 8, image::Rgb([shade, 128, 255 - shade]))
                .save(photos.join(format!("{i}.jpg")))
                .expect("a JPEG");
        }
        let catalog = dir.path().join("template.rawmakase");
        crate::catalog::Catalog::create(&catalog)
            .and_then(|mut c| c.add_folder(&photos).map(|_| ()))
            .expect("a catalog");
        let ctx = egui::Context::default();
        let mut e = Editor::with_context(
            &ctx,
            None,
            crate::app::preferences::session::Session::default(),
            None,
        );
        let library =
            crate::app::library::Library::load(&catalog, ctx.clone()).expect("the catalog loads");
        let mut photos: Vec<_> = library
            .session
            .photos()
            .iter()
            .map(|p| (p.filename.clone(), p.id))
            .collect();
        photos.sort();
        let ids: Vec<PhotoId> = photos.into_iter().map(|(_, id)| id).collect();
        // In the catalog every editor copies: the first photo in the Quick
        // Collection and rated 2, by B and 2 as the grid takes them, and the last
        // rejected.
        e.library = Some(Box::new(library));
        e.module = Module::Library;
        e.onboarding.visible = false;
        e.library.as_mut().unwrap().select(Some(ids[0]));
        let events = [Key::B, Key::Num2]
            .into_iter()
            .flat_map(|key| tap_events(key, key, Modifiers::NONE))
            .collect();
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(1200., 800.))),
                events,
                ..Default::default()
            },
            |ui| e.draw(ui),
        );
        output.textures_delta.clear();
        let photo = e.library.as_ref().unwrap().photo(ids[0]).unwrap();
        assert_eq!(photo.rating, 2);
        assert!(e.library.as_ref().unwrap().in_quick_collection(ids[0]));
        // The last photo rejected, for Delete Rejected Photos.
        e.library
            .as_mut()
            .unwrap()
            .edit_metadata(
                ids[PHOTOS - 1],
                crate::app::library::photo_metadata::Edit::Flag(-1),
                false,
            )
            .expect("rejected");
        drop(e);
        Template {
            dir,
            catalog,
            ctx,
            ids,
            built: 0,
        }
    }

    /// An editor in `place`; see [`Fixture::new`].
    pub(super) fn fixture(&mut self, place: Place) -> Fixture {
        self.built += 1;
        let catalog = self.dir.path().join(format!("{}.rawmakase", self.built));
        std::fs::copy(&self.catalog, &catalog).expect("the catalog copies");
        let ctx = self.ctx.clone();
        ctx.memory_mut(|m| {
            let options = m.options.clone();
            *m = Default::default();
            m.options = options;
        });
        let mut e = Editor::with_context(
            &ctx,
            None,
            crate::app::preferences::session::Session::default(),
            None,
        );
        let library = crate::app::library::Library::load(&catalog, ctx).expect("the catalog loads");
        e.library = Some(Box::new(library));
        e.module = Module::Library;
        e.onboarding.visible = false;
        let f = Fixture {
            catalog,
            _dir: None,
            e,
            ids: self.ids.clone(),
            surround: Surround::Nothing,
            field: String::new(),
        };
        f.set_up(place)
    }

    /// The folder's path, as a message may name it.
    pub(super) fn path(&self) -> &std::path::Path {
        self.dir.path()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.catalog);
    }
}

impl Fixture {
    /// An editor in `place`, where every shortcut has something to act on: eight
    /// photos, more than a grid row holds, the second to fourth selected and the
    /// fourth active (in Develop, open, with an edit); the first rated 2, picked
    /// and in the Quick Collection, the last rejected; a message showing, a step
    /// to undo and one to redo; settings copied and a previous photo's to paste;
    /// and in each tool a crop, spot, red eye, mask or guide selected.
    pub(super) fn new(place: Place) -> Self {
        let mut template = Template::new();
        let mut f = template.fixture(place);
        f._dir = Some(template.dir);
        f
    }

    fn set_up(mut self, place: Place) -> Self {
        let f = &mut self;
        // Something to undo and to redo: the first photo picked (undo), and
        // labelled Green, undone (redo).
        let first = f.ids[0];
        use crate::app::library::photo_metadata::Edit;
        f.library()
            .edit_metadata(first, Edit::Flag(1), false)
            .expect("picked");
        f.library()
            .edit_metadata(first, Edit::Label("Green".into()), false)
            .expect("labelled");
        f.e.sync_undo();
        f.e.undo();
        assert_eq!((f.photo(0).flag, f.photo(0).label.as_str()), (1, ""));
        let (first, active) = (f.ids[1], f.ids[3]);
        let library = f.library();
        library.select(Some(first));
        library.select_range_to(active);
        // Copied settings to paste, and the previous photo's.
        let settings = |exposure: f32| crate::app::develop::settings_transfer::Settings {
            recipe: crate::model::recipe::Recipe {
                exposure,
                ..Default::default()
            },
            metadata: blue_photo().metadata.clone(),
        };
        f.e.clipboard = Some(crate::app::develop::settings_transfer::Clipboard {
            settings: settings(1.),
            groups: Default::default(),
        });
        f.e.previous_settings = Some(settings(-1.));
        f.e.view.selected_setting = Some(crate::model::params::ParameterId::Exposure);
        match place {
            Place::Grid | Place::GridTyping | Place::GridButton | Place::GridModal => {}
            Place::Loupe | Place::LoupePopup => f.library().open_loupe(),
            Place::Compare => f.library().open_compare(),
            Place::Survey => f.library().open_survey(),
            _ => {
                assert!(place.develop());
                f.e.module = Module::Develop;
                open_decoded(&mut f.e, active);
                // An edit for Reset to undo.
                f.e.document.edit.setup_mut().exposure = 0.5;
                f.set_up_tool(place);
            }
        }
        f.e.messages.warn(MESSAGE);
        f.surround = place.surround();
        if f.surround == Surround::Popup {
            egui::Popup::open_id(&f.e.context, popup_id());
        }
        if matches!(place, Place::GridModal | Place::DevelopModal) {
            f.e.open_preferences(crate::app::preferences::Tab::General);
        }
        // One frame lays out the views, and a text field drawn in it has the
        // keyboard in the next.
        f.frame(vec![]);
        // The Loupe shows a JPEG as it is, and knows its size, which the zoom
        // keys step from, once a worker has read it: waited for here, so no row
        // depends on when it arrives.
        if matches!(place, Place::Loupe | Place::LoupePopup) {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
            while f.e.library.as_ref().unwrap().loupe_fit().is_none() {
                assert!(
                    std::time::Instant::now() < deadline,
                    "the Loupe's photo never came"
                );
                std::thread::sleep(std::time::Duration::from_millis(1));
                f.frame(vec![]);
            }
        }
        self
    }

    fn set_up_tool(&mut self, place: Place) {
        let e = &mut self.e;
        match place {
            Place::Crop => {
                e.document.edit.setup_mut().crop = [0.1, 0.2, 0.5, 0.6];
                e.view.toggle(Tool::Crop);
            }
            Place::Remove => {
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
                    });
                e.view.toggle(Tool::Remove);
                e.view.retouch.selected = Some(0);
            }
            Place::RedEye => {
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
            }
            Place::Mask => {
                e.view.toggle(Tool::Mask);
                e.document
                    .edit
                    .setup_mut()
                    .masks
                    .push(crate::model::masks::MaskGroup {
                        components: vec![crate::model::masks::MaskComponent::new(
                            crate::model::masks::MaskShape::Radial {
                                center: [0.5, 0.5],
                                radii: [0.2, 0.1],
                                angle: 0.,
                                feather: 0.5,
                            },
                        )],
                        ..Default::default()
                    });
                e.view.masking.selected = Some(0);
                e.view.masking.component = Some(0);
            }
            Place::Guided => {
                let upright = &mut e.document.edit.setup_mut().upright;
                upright.mode = crate::model::transform::UprightMode::Guided;
                upright.guides = vec![crate::model::transform::UprightGuide {
                    a: [0.2, 0.1],
                    b: [0.25, 0.9],
                }];
                e.view.set_tool(Tool::Guided);
                e.view.guided.selected = Some(0);
            }
            _ => {}
        }
    }

    pub(super) fn library(&mut self) -> &mut crate::app::library::Library {
        self.e.library.as_mut().expect("a catalog")
    }

    /// The `i`th photo of the catalog, as the catalog has it now.
    pub(super) fn photo(&self, i: usize) -> crate::catalog::Photo {
        self.e
            .library
            .as_ref()
            .and_then(|l| l.photo(self.ids[i]))
            .cloned()
            .expect("in the catalog")
    }

    /// One frame of `events` through the whole editor, with what surrounds it.
    pub(super) fn frame(&mut self, events: Vec<egui::Event>) {
        self.frame_watching(events, None);
    }

    /// As [`Self::frame`], and whether a press of `watch` was still in the frame's
    /// input once the editor was drawn: false when something took it.
    pub(super) fn frame_watching(
        &mut self,
        events: Vec<egui::Event>,
        watch: Option<Key>,
    ) -> Option<bool> {
        self.run(events, watch, true)
    }

    /// A frame that holds a key down without the editor seeing it: what the
    /// keyboard does before its first repeat.
    pub(super) fn frame_without_editor(&mut self, events: Vec<egui::Event>) {
        self.run(events, None, false);
        if self.surround == Surround::Popup {
            egui::Popup::open_id(&self.e.context, popup_id());
        }
    }

    fn run(&mut self, events: Vec<egui::Event>, watch: Option<Key>, editor: bool) -> Option<bool> {
        let ctx = self.e.context.clone();
        let surround = self.surround;
        let mut left = None;
        let (e, field) = (&mut self.e, &mut self.field);
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(1200., 800.))),
                events,
                ..Default::default()
            },
            |ui| {
                match surround {
                    Surround::Button => {
                        egui::Area::new(egui::Id::new("keymap-pins-focus"))
                            .show(ui.ctx(), |ui| ui.button("focused").request_focus());
                    }
                    Surround::Typing => {
                        egui::Area::new(egui::Id::new("keymap-pins-field")).show(ui.ctx(), |ui| {
                            ui.text_edit_singleline(field).request_focus()
                        });
                    }
                    _ => {}
                }
                if editor {
                    e.draw(ui);
                }
                left = watch.map(|key| {
                    ui.ctx().input(|i| {
                        i.events.iter().any(|event| {
                            matches!(event, egui::Event::Key { key: k, pressed: true, .. } if *k == key)
                        })
                    })
                });
                if surround == Surround::Popup {
                    egui::Popup::new(
                        popup_id(),
                        ui.ctx().clone(),
                        Pos2::new(600., 400.),
                        egui::LayerId::new(egui::Order::Foreground, popup_id()),
                    )
                    .open_memory(None)
                    .show(|ui| ui.label("A menu"));
                }
            },
        );
        output.textures_delta.clear();
        left
    }

    /// `key` pressed with `m` held, released, then the modifiers let go, in one frame.
    pub(super) fn tap(&mut self, key: Key, m: Modifiers) {
        self.frame(tap_events(key, key, m));
    }

    /// Several taps in one frame.
    pub(super) fn taps(&mut self, keys: &[(Key, Modifiers)]) {
        let events = keys
            .iter()
            .flat_map(|(key, m)| tap_events(*key, *key, *m))
            .collect();
        self.frame(events);
    }

    /// `key` pressed with `m` held, in a frame the editor does not see: held
    /// down, as before a key repeats. egui marks a press as a repeat from the keys
    /// it saw down, so the flag alone would not make one.
    pub(super) fn hold(&mut self, key: Key, m: Modifiers) {
        self.frame_without_editor(vec![
            egui::Event::ModifiersChanged(m),
            key_event(key, key, true, m),
        ]);
    }

    /// The first repeat of `key`, held with `m` since [`Self::hold`], then its
    /// release, and whether the repeat was still in the frame's input once the
    /// editor drew.
    pub(super) fn first_repeat(&mut self, key: Key, m: Modifiers) -> Option<bool> {
        self.frame_watching(
            vec![
                key_event(key, key, true, m),
                key_event(key, key, false, m),
                egui::Event::ModifiersChanged(Modifiers::NONE),
            ],
            Some(key),
        )
    }

    pub(super) fn index(&self, id: PhotoId) -> String {
        match self.ids.iter().position(|i| *i == id) {
            Some(i) => i.to_string(),
            None => format!("new{}", id.0),
        }
    }

    pub(super) fn active(&self) -> Option<PhotoId> {
        self.e.library.as_ref().and_then(|l| l.selected())
    }

    /// Everything a shortcut can change, by name, each as a stable string.
    pub(super) fn fingerprint(&self) -> Vec<(&'static str, String)> {
        let e = &self.e;
        let view = &e.view;
        let mut out: Vec<(&'static str, String)> = Vec::new();
        let mut put = |name: &'static str, value: String| out.push((name, value));
        put("module", format!("{:?}", e.module));
        if let Some(library) = &e.library {
            let layout = library.layout();
            put("view", layout.view.clone());
            put("cells", layout.cell_style.clone());
            put(
                "filters",
                if layout.filters_off { "off" } else { "on" }.into(),
            );
            put("info", library.loupe_info());
            put(
                "active",
                library
                    .selected()
                    .map(|id| self.index(id))
                    .unwrap_or_else(|| "-".into()),
            );
            put(
                "selected",
                library
                    .selected_photos()
                    .iter()
                    .map(|id| self.index(*id))
                    .collect::<Vec<_>>()
                    .join(","),
            );
            let photos = library.session.photos();
            put(
                "photos",
                photos
                    .iter()
                    .map(|p| {
                        format!(
                            "{}:r{}f{}{}{}{}",
                            self.index(p.id),
                            p.rating,
                            p.flag,
                            p.label.chars().next().unwrap_or('_'),
                            if library.in_quick_collection(p.id) {
                                "q"
                            } else {
                                ""
                            },
                            match &p.stack {
                                Some(s) => format!("s{s:?}"),
                                None => String::new(),
                            }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(" "),
            );
            put("source", library.source_name());
            put(
                "shown",
                library
                    .shown()
                    .iter()
                    .map(|id| self.index(*id))
                    .collect::<Vec<_>>()
                    .join(","),
            );
            let (select, candidate, side) = library.compared();
            let name =
                |id: Option<PhotoId>| id.map(|id| self.index(id)).unwrap_or_else(|| "-".into());
            put(
                "compared",
                format!("{} {} {side}", name(select), name(candidate)),
            );
        }
        put(
            "open",
            e.document
                .catalog_photo
                .map(|id| self.index(id))
                .unwrap_or_else(|| "-".into()),
        );
        put("tool", format!("{:?}", view.tool()));
        put(
            "zoom",
            format!(
                "{} {} at {:?}",
                if view.zoom.on { "on" } else { "off" },
                view.zoom.level,
                view.zoom.pan
            ),
        );
        put("magnifier", view.magnifier.on.to_string());
        put("compare", format!("{:?}", view.compare));
        put("chrome", format!("{:?}", e.chrome).replace("Chrome ", ""));
        put("panels", panels(&e.panels));
        put("tab panel", e.develop_tabs_shown().to_string());
        put(
            "clipping",
            format!(
                "shadows {} highlights {}",
                view.clipping
                    .is_on(crate::app::develop::clipping::ClipSide::Shadows),
                view.clipping
                    .is_on(crate::app::develop::clipping::ClipSide::Highlights)
            ),
        );
        put("slider", format!("{:?}", view.selected_setting));
        put(
            "guides",
            format!("{:?}", view.crop_guides).replace("CropGuides ", ""),
        );
        put("aspect", format!("{} {}", view.aspect, view.chosen_aspect));
        let r = &view.retouch;
        put(
            "remove",
            format!(
                "size {} feather {} selected {:?} pins {} visualize {}",
                r.size,
                r.feather,
                r.selected,
                if r.hide_pins { "hidden" } else { "shown" },
                r.visualize
            ),
        );
        let m = &view.masking;
        let brush = &m.brushes[m.brush];
        put(
            "mask",
            format!(
                "brush {} size {} feather {} overlay {} selected {:?}/{:?} placing {}",
                m.brush,
                brush.size,
                brush.feather,
                m.overlay,
                m.selected,
                m.component,
                m.is_placing()
            ),
        );
        put(
            "red eye",
            format!(
                "size {} selected {:?}",
                view.red_eye.size, view.red_eye.selected
            ),
        );
        put("guide", format!("{:?}", view.guided.selected));
        let recipe = e.document.edit.recipe();
        put(
            "recipe",
            format!("{:016x}", fnv(&serde_json::to_vec(recipe).unwrap())),
        );
        put(
            "before",
            match &e.document.before {
                Some(before) => format!("{:016x}", fnv(&serde_json::to_vec(before).unwrap())),
                None => "-".into(),
            },
        );
        let (steps, applied) = e.document.edit.history().steps();
        put(
            "history",
            format!(
                "{} of [{}]",
                applied,
                steps
                    .iter()
                    .map(|s| s.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        );
        put("dialog", self.dialog());
        put(
            "busy",
            format!(
                "{} auto {}",
                e.activity.is_busy(),
                e.document.auto.is_running()
            ),
        );
        put("message", e.messages.line().to_string());
        put("messages", e.messages.entry_count().to_string());
        if self.surround == Surround::Typing {
            put("field", self.field.clone());
        }
        put("popup", egui::Popup::is_any_open(&e.context).to_string());
        out
    }

    /// The windows open over the editor.
    fn dialog(&self) -> String {
        let e = &self.e;
        let mut open = Vec::new();
        if e.preferences.open {
            open.push("Preferences".to_string());
        }
        if e.view.shortcuts {
            open.push("Shortcuts".into());
        }
        if e.export_modal() {
            open.push("Export".into());
        }
        match &e.modal {
            Some(Modal::CopySettings(d)) => open.push(format!("{:?}", d.purpose)),
            Some(modal) => open.push(format!("{:?}", std::mem::discriminant(modal))),
            None => {}
        }
        if e.report_issue.is_open() {
            open.push("Report Issue".into());
        }
        let file = e.file_ops.state_name();
        if file != "Idle" {
            open.push(format!("File {file}"));
        }
        if e.auto_stack_open() {
            open.push("Auto-Stack".into());
        }
        if open.is_empty() {
            "-".into()
        } else {
            open.join(", ")
        }
    }
}

/// Each module's panels, upper case when shown: Left, Right, Filmstrip, the
/// Presets tab panel and the Adjustments, then Develop's tab.
fn panels(p: &crate::app::shell::panels::WorkspacePanels) -> String {
    use crate::app::shell::panels::Visibility;
    let layout = |l: &crate::app::shell::panels::PanelLayout| -> String {
        [
            (l.left, 'l'),
            (l.right, 'r'),
            (l.filmstrip, 'f'),
            (l.presets, 'p'),
            (l.adjustments, 'a'),
        ]
        .into_iter()
        .map(|(v, c)| match v {
            Visibility::Shown => c.to_ascii_uppercase(),
            Visibility::Hidden => c,
        })
        .collect()
    };
    format!(
        "develop {} library {} tab {:?}",
        layout(&p.develop),
        layout(&p.library),
        p.develop_tab
    )
}

/// FNV-1a, the same on every platform and every Rust release.
pub(super) fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// The fields of `after` that differ from `before`, as `name=value`.
pub(super) fn changes(
    before: &[(&'static str, String)],
    after: &[(&'static str, String)],
) -> Vec<String> {
    after
        .iter()
        .filter(|(name, value)| {
            before
                .iter()
                .find(|(n, _)| n == name)
                .is_none_or(|(_, v)| v != value)
        })
        .map(|(name, value)| format!("{name}={value}"))
        .collect()
}
