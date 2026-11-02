//! Preferences > Raw Defaults (see `crate::raw_defaults`): choosing them, and keeping
//! the open photo and the Library's previews in step when they change.
use super::Editor;
use super::preferences::{group, hint};
use super::state::EditOrigin;
use super::widgets::form_row;
use crate::app::design::{Tone, radius, text};
use crate::app::theme;
use crate::raw_defaults::{
    DefaultChoice, DevelopDefaults, RawDefaults, Resolved, camera_name, same_camera, same_name,
};
use crate::xmp::Preset;
use eframe::egui;
use std::sync::Arc;

/// The camera rows of the Raw Defaults block, as chosen and not yet created.
#[derive(Default)]
pub(super) struct Form {
    /// Cameras to choose from: the catalog's and the open photo's.
    cameras: Vec<String>,
    camera: String,
    choice: DefaultChoice,
}

impl Editor {
    /// The open photo's raw defaults: as last resolved, else resolved now.
    pub(super) fn photo_defaults(&self) -> Option<Resolved> {
        if let Some(defaults) = &self.document.defaults {
            return Some(defaults.clone());
        }
        let m = self.document.metadata.as_ref()?;
        Some(self.raw_defaults.resolve(m, &self.document.profiles))
    }
    /// Resolves the open photo's raw defaults again (its profiles arrived, or
    /// the defaults changed). A photo without an edit follows them; an edit,
    /// saved, converted from Lightroom or begun since it opened, is kept.
    pub(super) fn refresh_photo_defaults(&mut self) {
        let Some(m) = &self.document.metadata else {
            self.document.defaults = None;
            return;
        };
        let resolved = self.raw_defaults.resolve(m, &self.document.profiles);
        let before_changed =
            self.document.defaults.as_ref().map(|d| &d.recipe) != Some(&resolved.recipe);
        let mut changed = before_changed && self.view.compare.shows_before();
        if self.follows_defaults() {
            if let Some(note) = &resolved.note {
                self.messages.note(note.clone());
            }
            if *self.document.edit.recipe() != resolved.recipe {
                self.document.edit.replace(resolved.recipe.clone());
                changed = true;
            }
        }
        // Stored first, so Before renders the new defaults.
        self.document.defaults = Some(resolved);
        if changed {
            self.schedule();
        }
    }
    /// Whether the open photo has no edit, so it shows the raw defaults: none
    /// when it opened, none begun since, and none saved (export options alone
    /// make a saved edit too).
    fn follows_defaults(&self) -> bool {
        let saved = match (&self.library, self.document.catalog_photo) {
            (Some(l), Some(id)) => l
                .session
                .catalog
                .edit_texts(id)
                .map_or(true, |(recipe, _)| recipe.is_some()),
            _ => false,
        };
        self.document.origin == EditOrigin::Defaults
            && self.document.edit.history().steps().0.is_empty()
            && !self.document.edit.save_state().needs_save()
            && !self.document.edit.save_state().is_protected()
            && !saved
    }
    /// Why the open photo shows Adobe Default instead of the raw default chosen
    /// for it, while it has no edit.
    pub(super) fn defaults_note(&self) -> Option<String> {
        let note = self.document.defaults.as_ref()?.note.clone()?;
        self.follows_defaults().then_some(note)
    }
    /// Makes `settings` the raw defaults, saved with the session, and shows
    /// photos without an edit with them. They apply even when the session
    /// cannot be saved, which is returned.
    pub(super) fn set_raw_defaults(&mut self, settings: RawDefaults) -> anyhow::Result<()> {
        let defaults = DevelopDefaults::load(settings);
        if defaults == *self.raw_defaults {
            return Ok(());
        }
        self.raw_defaults = Arc::new(defaults);
        let saved = self.save_session();
        self.refresh_library_defaults();
        self.refresh_photo_defaults();
        saved
    }
    /// Renders the Library's previews of photos without an edit again, e.g. as
    /// imported camera profiles change what the defaults resolve to.
    pub(super) fn refresh_library_defaults(&mut self) {
        self.refresh_library_defaults_preserving(None);
    }

    pub(super) fn refresh_library_defaults_preserving(
        &mut self,
        current: Option<crate::catalog::PhotoId>,
    ) {
        if let Some(library) = &mut self.library {
            library.set_defaults_preserving(self.raw_defaults.clone(), current);
        }
        // An unedited reference photo follows them too, profiles imported included.
        self.reload_reference();
    }
    /// The open photo whose Library thumbnail an import of profiles leaves
    /// valid: its Develop recipe is still `before`, so no render replaces the
    /// thumbnail, and it is still the catalog's photo at the open path.
    pub(super) fn unchanged_current_photo(
        &self,
        before: &crate::model::recipe::Recipe,
    ) -> Option<crate::catalog::PhotoId> {
        if self.document.metadata.is_none() || self.document.edit.recipe() != before {
            return None;
        }
        self.document.catalog_photo.filter(|id| {
            self.library.as_ref().is_some_and(|library| {
                library
                    .photo(*id)
                    .is_some_and(|photo| Some(&photo.path) == self.document.path.as_ref())
            })
        })
    }
    /// Lists the cameras to choose from, when Preferences opens.
    pub(super) fn measure_raw_defaults(&mut self) {
        let mut cameras = self
            .library
            .as_ref()
            .and_then(|l| l.session.catalog.cameras().ok())
            .unwrap_or_default();
        if let Some(m) = &self.document.metadata
            && !m.model.trim().is_empty()
            && !cameras.iter().any(|c| same_camera(c, m))
        {
            cameras.push(camera_name(m));
        }
        for c in &self.raw_defaults.settings().cameras {
            if !cameras.iter().any(|n| same_name(n, &c.camera)) {
                cameras.push(c.camera.clone());
            }
        }
        cameras.sort_by_key(|c| c.to_lowercase());
        let form = &mut self.preferences.raw_defaults;
        // The open photo's camera first, as Lightroom does.
        let open = self
            .document
            .metadata
            .as_ref()
            .and_then(|m| cameras.iter().find(|c| same_camera(c, m)));
        if !cameras.contains(&form.camera) {
            form.camera = open.or(cameras.first()).cloned().unwrap_or_default();
        }
        // Shows the camera's own choice, ready to update.
        if let Some(own) = self.raw_defaults.settings().camera_choice(&form.camera) {
            form.choice = own.clone();
        }
        form.cameras = cameras;
    }
    /// The Raw Defaults block of Preferences' Profiles & Presets page.
    pub(super) fn raw_defaults_block(&mut self, ui: &mut egui::Ui) {
        let mut settings = self.raw_defaults.settings().clone();
        let presets = self.presets.library.clone();
        let presets = &presets.presets;
        group(ui, "Raw Defaults");
        form_row(ui, "Master", |ui| {
            choice_combo(ui, "raw-default-master", &mut settings.master, presets);
        });
        form_row(ui, "", |ui| {
            ui.checkbox(
                &mut settings.camera_overrides,
                "Override global setting for specific camera",
            );
        });
        let form = &mut self.preferences.raw_defaults;
        let on = settings.camera_overrides;
        let existing = settings.camera_choice(&form.camera).cloned();
        form_row(ui, "Camera", |ui| {
            ui.add_enabled_ui(on && !form.cameras.is_empty(), |ui| {
                let shown = if form.camera.is_empty() {
                    "No cameras in this catalog".to_string()
                } else {
                    form.camera.clone()
                };
                let before = form.camera.clone();
                egui::ComboBox::from_id_salt("raw-default-camera")
                    .width(240.)
                    .selected_text(shown)
                    .show_ui(ui, |ui| {
                        for c in &form.cameras {
                            ui.selectable_value(&mut form.camera, c.clone(), c);
                        }
                    });
                // Picking a camera shows its own choice, to update.
                if form.camera != before
                    && let Some(own) = settings.camera_choice(&form.camera)
                {
                    form.choice = own.clone();
                }
            });
        });
        let mut create = false;
        form_row(ui, "Default", |ui| {
            ui.add_enabled_ui(on && !form.camera.is_empty(), |ui| {
                choice_combo(ui, "raw-default-camera-choice", &mut form.choice, presets);
                let label = if existing.is_some() {
                    "Update Default"
                } else {
                    "Create Default"
                };
                create = ui
                    .add_enabled(
                        existing.as_ref() != Some(&form.choice),
                        egui::Button::new(label),
                    )
                    .clicked();
            });
        });
        if create {
            settings.set_camera(&form.camera, form.choice.clone());
        }
        let mut remove = None;
        let palette = theme::palette(ui.ctx());
        for c in &settings.cameras {
            form_row(ui, "", |ui| {
                if remove_button(ui, on).clicked() {
                    remove = Some(c.camera.clone());
                }
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(format!(
                            "{}: {}",
                            c.camera,
                            choice_text(&c.choice, presets)
                        ))
                        .color(palette.tone(if on {
                            Tone::TEXT
                        } else {
                            Tone::MUTED
                        })),
                    )
                    .truncate(),
                );
            });
        }
        if let Some(camera) = remove {
            settings.cameras.retain(|c| c.camera != camera);
        }
        form_row(ui, "", |ui| {
            hint(
                ui,
                "Photos without an edit start from these, and Reset returns to them. Saved edits and Lightroom edits are kept.",
            );
        });
        if settings != *self.raw_defaults.settings() {
            // Replaces any earlier choice's note.
            match self.set_raw_defaults(settings) {
                Err(e) => self.messages.fail(format!("Raw defaults not saved: {e:#}")),
                Ok(()) => match self.photo_defaults().and_then(|d| d.note) {
                    Some(note) => self.messages.warn(note),
                    None => self.messages.done("Raw defaults saved"),
                },
            }
        }
    }
}

/// A choice's name, marked when its preset is no longer installed.
fn choice_text(choice: &DefaultChoice, presets: &[Preset]) -> String {
    let missing = matches!(choice, DefaultChoice::Preset { id, .. }
        if !presets.iter().any(|p| p.id == *id));
    // Until the preset library has loaded, nothing is known to be missing.
    if missing && !presets.is_empty() {
        format!("{} (missing)", choice.label())
    } else {
        choice.label()
    }
}
/// A small ✕ that removes a camera's row.
fn remove_button(ui: &mut egui::Ui, enabled: bool) -> egui::Response {
    let palette = theme::palette(ui.ctx());
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(egui::Vec2::splat(22.), sense);
    let hovered = enabled && response.hovered();
    if hovered {
        ui.painter()
            .rect_filled(rect, radius::CONTROL, palette.tone(Tone::OUTLINE));
    }
    let tint = palette.tone(match (enabled, hovered) {
        (false, _) => Tone::OUTLINE_STRONG,
        (true, false) => Tone::SECONDARY,
        (true, true) => Tone::STRONG,
    });
    super::icons::paint_at(
        ui.painter(),
        super::icons::Icon::Close,
        rect.center(),
        13.,
        tint,
    );
    response.on_hover_text("Remove this camera's default")
}
/// Adobe Default, Camera Settings, RAWmakase Default or a Develop preset, by group.
fn choice_combo(ui: &mut egui::Ui, id: &str, choice: &mut DefaultChoice, presets: &[Preset]) {
    egui::ComboBox::from_id_salt(id)
        .width(240.)
        .height(360.)
        .selected_text(choice_text(choice, presets))
        .show_ui(ui, |ui| {
            for fixed in [
                DefaultChoice::Adobe,
                DefaultChoice::CameraSettings,
                DefaultChoice::Rawmakase,
            ] {
                let label = fixed.label();
                ui.selectable_value(choice, fixed, label);
            }
            let caption = theme::palette(ui.ctx()).tone(Tone::FAINT);
            let mut last_group = None;
            for p in presets {
                if last_group != Some(&p.group) {
                    ui.separator();
                    let title = if p.group.is_empty() {
                        "User Presets"
                    } else {
                        &p.group
                    };
                    ui.label(egui::RichText::new(title).size(text::SMALL).color(caption));
                    last_group = Some(&p.group);
                }
                let value = DefaultChoice::preset(p);
                let label = crate::presets::display_name(&p.name);
                ui.selectable_value(choice, value, label);
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::worker::{Event, LoadedHeader};
    use crate::camera_data::Metadata;
    use crate::camera_profiles::{CameraProfile, open};
    use crate::model::recipe::Recipe;

    #[allow(clippy::approx_constant)] // Exact camera matrix coefficients, not mathematical constants.
    fn x100f() -> Metadata {
        Metadata {
            make: "Fujifilm".into(),
            model: "X100F".into(),
            wb: [2.0198677, 1., 1.8874172],
            cam_xyz: [
                [1.1434, -0.4948, -0.121],
                [-0.3746, 1.2042, 0.1903],
                [-0.0666, 0.1479, 0.5235],
            ],
            ..Default::default()
        }
    }
    fn profiles(m: &Metadata) -> Vec<Arc<CameraProfile>> {
        [open::standard(m), open::color(m)]
            .into_iter()
            .flatten()
            .map(Arc::new)
            .collect()
    }
    /// A built-in preset as the master raw default.
    fn lighten() -> RawDefaults {
        let (presets, _) = crate::presets::builtin::presets();
        let preset = presets
            .iter()
            .find(|p| p.group == "Curve" && p.name == "Lighten")
            .unwrap();
        RawDefaults {
            master: DefaultChoice::preset(preset),
            ..Default::default()
        }
    }

    #[test]
    fn reset_returns_to_the_raw_defaults() {
        let ctx = egui::Context::default();
        let mut editor = Editor::with_context(&ctx, None, Default::default(), None);
        let m = x100f();
        editor.document.profiles = profiles(&m);
        editor.document.metadata = Some(m.clone());
        editor.raw_defaults = Arc::new(crate::raw_defaults::brighter_defaults());
        editor.document.edit.setup_mut().exposure = -1.;
        editor.reset_settings();
        assert_eq!(editor.document.edit.recipe().exposure, 0.7);
        assert_eq!(
            *editor.document.edit.recipe(),
            editor.raw_defaults.resolve(&m, &profiles(&m)).recipe
        );
    }

    #[test]
    fn photos_without_an_edit_follow_the_raw_defaults_and_saved_edits_are_kept()
    -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let raw = dir.path().join("photo.ARW");
        std::fs::write(&raw, b"identity fixture")?;
        let path = dir.path().join("photos.rawmakase");
        let mut catalog = crate::catalog::Catalog::create(&path)?;
        catalog.add_folder(dir.path())?;
        let id = catalog.photos()?[0].id;
        drop(catalog);
        let ctx = egui::Context::default();
        let m = x100f();
        let adobe = Recipe::with_profiles(&m, &profiles(&m));
        let open = |editor: &mut Editor| {
            editor.document.reset(Some(id));
            let (generation, _) = editor.load.start();
            for event in [
                Event::Header(Box::new(LoadedHeader {
                    id: generation,
                    path: raw.clone(),
                    metadata: m.clone(),
                    recipe: adobe.clone(),
                    export: Default::default(),
                    status: "Original".into(),
                })),
                Event::Profiles {
                    id: generation,
                    profiles: profiles(&m),
                    errors: Vec::new(),
                    groups: Default::default(),
                    imported: false,
                },
            ] {
                editor.tx.send(event).unwrap();
            }
            editor.events(&ctx);
        };
        let mut editor = Editor::with_context(&ctx, None, Default::default(), None);
        editor.library = Some(Box::new(crate::app::library::Library::load(
            &path,
            ctx.clone(),
        )?));
        open(&mut editor);
        assert_eq!(*editor.document.edit.recipe(), adobe);
        // Changing the defaults changes the photo without an edit, and saves nothing.
        editor.set_raw_defaults(lighten()).unwrap();
        let lightened = editor.raw_defaults.resolve(&m, &profiles(&m)).recipe;
        assert_ne!(lightened, adobe);
        assert_eq!(*editor.document.edit.recipe(), lightened);
        assert!(!editor.document.edit.save_state().needs_save());
        // Opened again, it starts from them.
        open(&mut editor);
        assert_eq!(*editor.document.edit.recipe(), lightened);
        // Once edited and saved, the edit stays as it is.
        let before = editor.document.edit.recipe().clone();
        editor.document.edit.setup_mut().exposure = 0.5;
        editor.commit_edit(before, None);
        assert!(editor.flush());
        let edited = editor.document.edit.recipe().clone();
        editor.set_raw_defaults(RawDefaults::default()).unwrap();
        assert_eq!(*editor.document.edit.recipe(), edited);
        open(&mut editor);
        assert_eq!(*editor.document.edit.recipe(), edited);
        editor.set_raw_defaults(lighten()).unwrap();
        assert_eq!(*editor.document.edit.recipe(), edited);
        let library = editor.library.as_ref().unwrap();
        assert_eq!(
            library.session.catalog.load_edit(id, &raw)?.unwrap().recipe,
            edited
        );
        Ok(())
    }

    #[test]
    fn export_options_alone_make_a_saved_edit_that_defaults_leave_alone() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let raw = dir.path().join("photo.ARW");
        std::fs::write(&raw, b"identity fixture")?;
        let path = dir.path().join("photos.rawmakase");
        let mut catalog = crate::catalog::Catalog::create(&path)?;
        catalog.add_folder(dir.path())?;
        let id = catalog.photos()?[0].id;
        drop(catalog);
        let ctx = egui::Context::default();
        let m = x100f();
        let adobe = Recipe::with_profiles(&m, &profiles(&m));
        let mut editor = Editor::with_context(&ctx, None, Default::default(), None);
        editor.library = Some(Box::new(crate::app::library::Library::load(
            &path,
            ctx.clone(),
        )?));
        editor.document.reset(Some(id));
        let (generation, _) = editor.load.start();
        editor
            .tx
            .send(Event::Header(Box::new(LoadedHeader {
                id: generation,
                path: raw,
                metadata: m,
                recipe: adobe.clone(),
                export: Default::default(),
                status: "Original".into(),
            })))
            .unwrap();
        editor.events(&ctx);
        editor.document.export.quality = 50;
        editor.document.edit.save_state_mut().mark_changed();
        assert!(editor.flush());
        editor.set_raw_defaults(lighten()).unwrap();
        assert_eq!(*editor.document.edit.recipe(), adobe);
        Ok(())
    }

    #[test]
    fn a_lightroom_edit_that_cannot_be_read_leaves_adobe_default() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let raw = dir.path().join("photo.ARW");
        std::fs::write(&raw, b"identity fixture")?;
        let path = dir.path().join("photos.rawmakase");
        let mut catalog = crate::catalog::Catalog::create(&path)?;
        catalog.add_folder(dir.path())?;
        let id = catalog.photos()?[0].id;
        drop(catalog);
        rusqlite::Connection::open(&path)?.execute(
            "UPDATE photos SET lightroom_develop='s = { Exposure2012 = ' WHERE id=?",
            [id.0],
        )?;
        let ctx = egui::Context::default();
        let m = x100f();
        let mut editor = Editor::with_context(&ctx, None, Default::default(), None);
        editor.library = Some(Box::new(crate::app::library::Library::load(
            &path,
            ctx.clone(),
        )?));
        editor.raw_defaults = Arc::new(crate::raw_defaults::brighter_defaults());
        editor.document.reset(Some(id));
        let (generation, _) = editor.load.start();
        for event in [
            Event::Header(Box::new(LoadedHeader {
                id: generation,
                path: raw,
                metadata: m.clone(),
                // As the loader resolves it.
                recipe: editor.raw_defaults.resolve(&m, &profiles(&m)).recipe,
                export: Default::default(),
                status: "Original".into(),
            })),
            Event::Profiles {
                id: generation,
                profiles: profiles(&m),
                errors: Vec::new(),
                groups: Default::default(),
                imported: false,
            },
        ] {
            editor.tx.send(event).unwrap();
        }
        editor.events(&ctx);
        assert!(editor.document.lightroom_notice.contains("not applied"));
        assert_eq!(
            *editor.document.edit.recipe(),
            Recipe::with_profiles(&m, &profiles(&m))
        );
        Ok(())
    }

    #[test]
    fn raw_defaults_that_cannot_be_saved_say_so() {
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("file");
        std::fs::write(&blocker, b"").unwrap();
        let ctx = egui::Context::default();
        let mut editor = Editor::with_context(
            &ctx,
            None,
            Default::default(),
            Some(blocker.join("session.json")),
        );
        assert!(editor.set_raw_defaults(lighten()).is_err());
        // Still in use until the app quits.
        assert_eq!(*editor.raw_defaults.settings(), lighten());
    }

    #[test]
    fn before_shows_new_defaults_at_once() {
        let ctx = egui::Context::default();
        let mut editor = Editor::with_context(&ctx, None, Default::default(), None);
        let m = x100f();
        editor.document.profiles = profiles(&m);
        editor.document.metadata = Some(m.clone());
        editor.document.origin = EditOrigin::Saved;
        editor.refresh_photo_defaults();
        editor.view.compare = crate::app::before_after::Compare::BeforeOnly;
        editor.set_raw_defaults(lighten()).unwrap();
        let lightened = editor.raw_defaults.resolve(&m, &profiles(&m)).recipe;
        assert_eq!(editor.effective_recipe().curve, lightened.curve);
    }

    #[test]
    fn a_fallback_note_outlasts_the_decode_status() {
        let ctx = egui::Context::default();
        let mut editor = Editor::with_context(&ctx, None, Default::default(), None);
        let m = x100f();
        editor.document.profiles = profiles(&m);
        editor.document.metadata = Some(m);
        editor.raw_defaults = Arc::new(DevelopDefaults::with_presets(
            RawDefaults {
                master: DefaultChoice::Preset {
                    id: "gone".into(),
                    name: "Gone".into(),
                },
                ..Default::default()
            },
            |_| None,
        ));
        editor.refresh_photo_defaults();
        let (generation, _) = editor.load.start();
        editor
            .tx
            .send(Event::Ready {
                id: generation,
                full: Arc::new(crate::camera_data::CameraImage {
                    recovered: Default::default(),
                    width: 12,
                    height: 8,
                    pixels: vec![[0.1; 3]; 96],
                    metadata: Metadata {
                        width: 12,
                        height: 8,
                        ..x100f()
                    },
                    fast: false,
                    scale_factor: 1.,
                    scale_clipped: 0,
                }),
                status: "Developed in 0.20s".into(),
            })
            .unwrap();
        editor.events(&ctx);
        assert!(editor.messages.line().starts_with("Developed in 0.20s · "));
        assert!(
            editor.messages.line().contains("‘Gone’ is missing"),
            "{}",
            editor.messages.line()
        );
    }

    #[test]
    fn the_loader_opens_a_photo_with_the_raw_defaults() {
        let chart = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/corpus/charts/synthetic-d65.dng");
        let (tx, rx) = std::sync::mpsc::channel();
        let loader = crate::app::worker::Loader::new(
            tx,
            egui::Context::default(),
            crate::decode_cache::DecodeCache::default(),
        );
        let defaults = Arc::new(crate::raw_defaults::brighter_defaults());
        loader.submit(crate::app::worker::LoadJob {
            id: 1,
            path: chart,
            cancel: Default::default(),
            prefetch: None,
            defaults: defaults.clone(),
            demosaic: crate::camera_data::Demosaic::default(),
        });
        let header = loop {
            match rx.recv_timeout(std::time::Duration::from_secs(30)).unwrap() {
                Event::Header(header) => break header,
                Event::Failed { error, .. } => panic!("{error}"),
                _ => {}
            }
        };
        let (profiles, _) = crate::camera_profiles::installed(&header.metadata);
        assert_eq!(header.recipe.exposure, 0.7);
        assert_eq!(
            header.recipe,
            defaults.resolve(&header.metadata, &profiles).recipe
        );
    }
}
