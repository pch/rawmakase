//! Lightroom's Reference View in Develop (Shift+R): another catalog photo shown
//! beside the one being edited, left/right or top/bottom, to match its look.
//!
//! The reference is set from the filmstrip (drag a photo onto the Reference side, or
//! right-click › Set as Reference Photo) and stays while moving between photos.
//! Leaving Develop clears it unless it is locked, as in Lightroom. It shows with its
//! own saved edit, framed by its own crop, and is never edited: panels and keys act
//! on the Active photo. It has its own zoom, Fit or 100%, as Lightroom's has. It is
//! decoded on its own worker (half size first, for Fit, then in full for 100%) and
//! rendered in Before's lane of the renderer, so it never holds up the edit.
use crate::app::Editor;
use crate::app::develop::before_after::{Axis, Compare, Pane, Panes, badge};
use crate::app::ui::design::{Tone, text};
use crate::app::view::navigator::Zoom;
use crate::app::worker::{ReferenceImage, ReferenceJob, Resolution};
use crate::model::recipe::Recipe;
use crate::{
    app::{library::DraggedPhoto, ui::theme},
    camera_data::CameraImage,
    catalog::PhotoId,
    develop::Geometry,
};
use eframe::egui::{self, Color32, Pos2, Rect, Vec2};
use std::sync::Arc;

/// Reference View's state: the reference photo, its lock, layout and zoom, and the
/// photo as last developed.
pub(in crate::app) struct ReferenceView {
    /// The catalog photo shown as the reference.
    pub(in crate::app) photo: Option<PhotoId>,
    /// Kept when leaving Develop, as Lightroom's lock in the toolbar.
    pub(in crate::app) locked: bool,
    /// Where the reference sits: left or on top. Shift+R shows the last one used.
    pub(in crate::app) layout: Axis,
    /// The reference's own Fit or 100%, and where.
    pub(in crate::app) zoom: Zoom,
    /// The reference as developed, once it is.
    loaded: Option<Loaded>,
    /// The photo and edit being developed.
    pending: Option<(PhotoId, String)>,
    load: crate::app::task::Task,
    /// Why the reference could not be shown.
    pub(in crate::app) error: Option<String>,
}
impl Default for ReferenceView {
    fn default() -> Self {
        Self {
            photo: None,
            locked: false,
            layout: Axis::LeftRight,
            zoom: Zoom::default(),
            loaded: None,
            pending: None,
            load: Default::default(),
            error: None,
        }
    }
}
/// The reference photo as developed.
struct Loaded {
    photo: PhotoId,
    /// Its edit's tag then: another edit, or other defaults, develop it again.
    tag: String,
    image: Arc<CameraImage>,
    recipe: Recipe,
}
/// What the Reference side renders: the photo and its edit.
pub(in crate::app) struct ReferenceSide {
    pub(in crate::app) image: Arc<CameraImage>,
    pub(in crate::app) recipe: Recipe,
}

impl ReferenceView {
    /// Stops loading the reference photo.
    pub(in crate::app) fn cancel_load(&mut self) {
        self.load.invalidate();
    }
    /// No reference: the side shows how to set one.
    fn clear(&mut self) {
        self.photo = None;
        self.loaded = None;
        self.pending = None;
        self.load.invalidate();
        self.error = None;
        self.zoom = Zoom::default();
    }
    /// Whether the reference is still being developed.
    pub(in crate::app) fn loading(&self) -> bool {
        self.load.is_running()
    }
}

impl Editor {
    /// Reference View is shown.
    pub(in crate::app) fn reference_view(&self) -> bool {
        self.view.compare.reference()
    }
    /// Shift+R: Reference View in its last layout, or back to the edit alone.
    pub(in crate::app) fn toggle_reference_view(&mut self) {
        let view = self
            .view
            .compare
            .toggled(Compare::Reference(self.reference.layout));
        self.set_compare(view);
    }
    /// Reference View with the reference left or on top.
    pub(in crate::app) fn set_reference_layout(&mut self, layout: Axis) {
        self.reference.layout = layout;
        self.set_compare(Compare::Reference(layout));
    }
    /// Makes catalog photo `id` the reference and shows Reference View, as dropping
    /// it on the Reference side or Set as Reference Photo does.
    pub(in crate::app) fn set_reference(&mut self, id: PhotoId) {
        let source = self
            .library
            .as_ref()
            .and_then(|l| l.develop_source(id, self.session.demosaic));
        let Some(source) = source else {
            return;
        };
        if let Err(refusal) = source {
            self.messages.warn(format!(
                "Can't be the reference photo: {}",
                refusal.detail()
            ));
            return;
        }
        if self.reference.photo != Some(id) {
            self.reference.clear();
            self.reference.photo = Some(id);
            self.preview.before.clear();
        }
        if !self.reference_view() {
            self.set_compare(Compare::Reference(self.reference.layout));
        }
        self.load_reference();
    }
    /// Develops the reference photo when it is not, or its edit changed since; the
    /// photo being edited needs nothing, as it is already open.
    pub(in crate::app) fn load_reference(&mut self) {
        let Some(id) = self.reference.photo else {
            return;
        };
        if self.document.catalog_photo == Some(id) {
            // Shown live: open, so whatever kept it from loading is past, and a
            // load still running for it can only bring back a stale error.
            self.reference.error = None;
            if self.reference.pending.take().is_some() {
                self.reference.load.invalidate();
            }
            return;
        }
        let source = self
            .library
            .as_ref()
            .and_then(|l| l.develop_source(id, self.session.demosaic));
        let source = match source {
            // No longer in the catalog, e.g. a virtual copy removed.
            None => return self.clear_reference(),
            Some(Err(refusal)) => {
                self.reference.error = Some(refusal.detail());
                return;
            }
            Some(Ok(source)) => source,
        };
        // Available again, e.g. its volume is back.
        self.reference.error = None;
        let developed = self
            .reference
            .loaded
            .as_ref()
            .is_some_and(|l| l.photo == id && l.tag == source.tag);
        let pending = self.reference.pending.as_ref();
        if developed || pending.is_some_and(|(p, tag)| *p == id && *tag == source.tag) {
            return;
        }
        let wanted = (id, source.tag);
        let (ticket, cancel) = self.reference.load.start();
        self.reference.pending = Some(wanted);
        self.reference_loader.submit(ReferenceJob {
            ticket,
            path: source.path,
            from: source.from,
            edit: source.edit,
            cancel,
            demosaic: self.session.demosaic,
        });
    }
    /// Develops the reference photo again even when its edit and file are as they
    /// were: what they resolve to changed, e.g. camera profiles were imported. It keeps
    /// showing until the new one arrives.
    pub(in crate::app) fn reload_reference(&mut self) {
        if let Some(loaded) = &mut self.reference.loaded {
            loaded.tag.clear();
        }
        self.reference.pending = None;
        self.load_reference();
    }
    /// The reference photo developed, or why not.
    pub(in crate::app) fn reference_ready(
        &mut self,
        ticket: u64,
        result: Result<Box<ReferenceImage>, String>,
    ) {
        if ticket != self.reference.load.id() {
            return;
        }
        let Some((photo, tag)) = self.reference.pending.clone() else {
            return;
        };
        match result {
            Ok(developed) => {
                // Only the full decode counts as developed: after a half-size one
                // alone (the full one failed), it is developed again when asked.
                let full = developed.resolution == Resolution::Full;
                if full {
                    self.reference.load.finish(ticket);
                    self.reference.pending = None;
                }
                self.reference.loaded = Some(Loaded {
                    photo,
                    tag: if full { tag } else { String::new() },
                    image: developed.image,
                    recipe: developed.recipe,
                });
            }
            Err(error) => {
                self.reference.load.finish(ticket);
                self.reference.pending = None;
                self.messages.fail(format!("Reference photo: {error}"));
                self.reference.error = Some(error);
            }
        }
        self.schedule_before();
    }
    /// What the Reference side shows: the reference as developed, or the photo being
    /// edited, live, when it is the reference.
    pub(in crate::app) fn reference_side(&self) -> Option<ReferenceSide> {
        let id = self.reference.photo?;
        // The photo being edited, live, once it is decoded; until then, as it was
        // last developed as the reference.
        if self.document.catalog_photo == Some(id)
            && let Some(image) = self.document.full()
        {
            return Some(ReferenceSide {
                image: image.clone(),
                recipe: self.document.edit.recipe().clone(),
            });
        }
        let loaded = self.reference.loaded.as_ref().filter(|l| l.photo == id)?;
        Some(ReferenceSide {
            image: loaded.image.clone(),
            recipe: loaded.recipe.clone(),
        })
    }
    /// No reference photo any more; Reference View, if shown, waits for another.
    pub(in crate::app) fn clear_reference(&mut self) {
        self.reference.clear();
        self.preview.before.clear();
    }
    /// Leaving Develop: the reference goes unless locked, as in Lightroom, the edit
    /// shows alone on return, and the RGB readout stops.
    pub(in crate::app) fn left_develop(&mut self) {
        self.view.readout = Default::default();
        if self.reference_view() {
            self.view.compare = Compare::Off;
            self.preview.before.clear();
        }
        if !self.reference.locked && self.reference.photo.is_some() {
            self.clear_reference();
        }
    }
}

impl Editor {
    /// Draws the Reference side: the reference photo at its own zoom, a stand-in
    /// while it is developed, or how to set one. Returns where the photo is drawn.
    pub(in crate::app) fn reference_pane_ui(
        &mut self,
        ui: &egui::Ui,
        panes: &Panes,
        pane: Pane,
    ) -> Option<Rect> {
        let palette = theme::palette(ui.ctx());
        badge(ui, panes.after.clip, "Active");
        let rect = self.reference_rect(ui, pane);
        match (rect, self.reference.photo) {
            (Some((rect, g)), _) => self.paint_second_side(ui, pane, rect, &g),
            (None, Some(id)) => self.reference_stand_in(ui, pane, id),
            (None, None) => {
                let text = "Drag a photo from the filmstrip here,\nor right-click one and choose\nSet as Reference Photo";
                ui.painter().with_clip_rect(pane.clip).text(
                    pane.clip.center(),
                    egui::Align2::CENTER_CENTER,
                    text,
                    egui::FontId::proportional(text::BODY),
                    palette.tone(Tone::SECONDARY),
                );
            }
        }
        if let Some(error) = &self.reference.error {
            ui.painter().with_clip_rect(pane.clip).text(
                pane.clip.center_bottom() - Vec2::new(0., 24.),
                egui::Align2::CENTER_CENTER,
                error,
                egui::FontId::proportional(text::LABEL),
                palette.tone(Tone::LABEL),
            );
        }
        badge(ui, pane.clip, "Reference");
        rect.map(|(rect, _)| rect)
    }
    /// Where the reference photo is drawn in `pane` at its own zoom, and its
    /// geometry, once it is developed.
    fn reference_rect(&self, ui: &egui::Ui, pane: Pane) -> Option<(Rect, Geometry)> {
        let side = self.reference_side()?;
        let g = Geometry::new(&side.image, &side.recipe, 0);
        let size = Vec2::new(g.width as f32, g.height as f32);
        let ppp = ui.ctx().pixels_per_point();
        Some((self.reference.zoom.photo_rect(pane.area, size, ppp), g))
    }
    /// The reference's Library preview, fitted, while it is developed.
    fn reference_stand_in(&self, ui: &egui::Ui, pane: Pane, id: PhotoId) {
        let texture = self.library.as_ref().and_then(|l| l.thumbnail(id));
        if let Some(texture) = texture {
            let size = texture.size_vec2();
            let k = (pane.area.width() / size.x).min(pane.area.height() / size.y);
            let rect = Rect::from_center_size(pane.area.center(), size * k);
            let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1., 1.));
            ui.painter()
                .with_clip_rect(pane.clip)
                .image(texture.id(), rect, uv, Color32::WHITE);
        }
        if self.reference.loading() {
            let at = Rect::from_center_size(pane.clip.center(), Vec2::splat(18.));
            crate::app::ui::widgets::paint_spinner(ui, at, None);
        }
    }
    /// Clicks, drags and drops on the Reference side: a click zooms the reference
    /// alone between Fit and 100%, a drag pans it, and a photo dragged from the
    /// filmstrip becomes the reference. Returns whether the pointer is the
    /// reference's, so the Active side leaves it alone.
    pub(in crate::app) fn reference_pointer(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        panes: &Panes,
        photo: Option<Rect>,
    ) -> bool {
        let Some(pane) = panes.before.filter(|_| self.reference_view()) else {
            return false;
        };
        let drop = ui.interact(
            pane.clip,
            ui.id().with("reference-drop"),
            egui::Sense::hover(),
        );
        if drop.dnd_hover_payload::<DraggedPhoto>().is_some() {
            ui.painter().rect_stroke(
                pane.clip.shrink(2.),
                0.,
                egui::Stroke::new(2., theme::palette(ui.ctx()).accent()),
                egui::StrokeKind::Inside,
            );
        }
        if let Some(dropped) = drop.dnd_release_payload::<DraggedPhoto>() {
            self.set_reference(dropped.0);
            return true;
        }
        let origin = ui.input(|i| i.pointer.press_origin());
        let ours = origin.is_some_and(|p| pane.clip.contains(p) && !panes.after.clip.contains(p));
        if !ours {
            return false;
        }
        let (Some(rect), Some(side)) = (photo, self.reference_side()) else {
            return true;
        };
        let g = Geometry::new(&side.image, &side.recipe, 0);
        let zoom = &mut self.reference.zoom;
        if zoom.on && response.dragged() {
            let delta = ui.input(|i| i.pointer.delta());
            zoom.pan[0] = (zoom.pan[0] - delta.x / rect.width()).clamp(0., 1.);
            zoom.pan[1] = (zoom.pan[1] - delta.y / rect.height()).clamp(0., 1.);
        }
        if response.clicked()
            && !response.double_clicked()
            && let Some(pos) = response.interact_pointer_pos()
            && rect.contains(pos)
        {
            let size = Vec2::new(g.width as f32, g.height as f32);
            let ppp = ui.ctx().pixels_per_point();
            zoom.toggle_at(pos, rect, pane.area, size, ppp);
        }
        self.schedule_before();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::worker::Event;
    use eframe::egui;
    use std::path::Path;

    /// A catalog of two copies of the synthetic chart, the second edited, opened
    /// in an editor with the first photo open.
    struct Fixture {
        _dir: tempfile::TempDir,
        editor: Editor,
        open: PhotoId,
        other: PhotoId,
    }
    fn fixture() -> anyhow::Result<Fixture> {
        let dir = tempfile::tempdir()?;
        let photos = dir.path().join("photos");
        std::fs::create_dir(&photos)?;
        let chart =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/charts/synthetic-d65.dng");
        for name in ["a.dng", "b.dng"] {
            std::fs::copy(&chart, photos.join(name))?;
        }
        let catalog = dir.path().join("test.rawmakase");
        let mut c = crate::catalog::Catalog::create(&catalog)?;
        c.add_folder(&photos)?;
        drop(c);
        let ctx = egui::Context::default();
        let mut library = crate::app::library::Library::load(&catalog, ctx.clone())?;
        let id = |name: &str| {
            library
                .session
                .photos()
                .iter()
                .find(|p| p.filename == name)
                .unwrap()
                .id
        };
        let (open, other) = (id("a.dng"), id("b.dng"));
        let edit = Recipe {
            exposure: 1.5,
            ..Default::default()
        };
        library.session.save_edit(
            other,
            &photos.join("b.dng"),
            &crate::storage::Identity::read(&photos.join("b.dng"))?,
            &edit,
            &Default::default(),
            crate::catalog::HistoryUpdate::Keep,
        )?;
        let mut editor = Editor::with_context(
            &ctx,
            None,
            crate::app::preferences::session::Session::default(),
            None,
        );
        editor.library = Some(Box::new(library));
        editor.document.catalog_photo = Some(open);
        editor.document.opened_at(photos.join("a.dng"));
        editor.view.viewport = eframe::egui::Vec2::new(200., 150.);
        Ok(Fixture {
            _dir: dir,
            editor,
            open,
            other,
        })
    }
    /// Takes the editor's events until the reference is developed in full.
    fn wait_for_reference(editor: &mut Editor) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while editor.reference.loading() {
            assert!(
                std::time::Instant::now() < deadline,
                "reference not developed"
            );
            match editor.rx.recv_timeout(std::time::Duration::from_secs(60)) {
                Ok(Event::Reference { ticket, result }) => editor.reference_ready(ticket, result),
                Ok(_) => {}
                Err(e) => panic!("{e}"),
            }
        }
    }

    #[test]
    fn the_reference_shows_its_own_edit_and_edits_reach_the_active_photo_only() -> anyhow::Result<()>
    {
        // The fixture keeps its folder for as long as it lives.
        let mut fixture = fixture()?;
        let (open, other) = (fixture.open, fixture.other);
        let editor = &mut fixture.editor;
        editor.view.tool = crate::app::develop::state::Tool::Mask;
        editor.set_reference(other);
        // Setting a reference shows Reference View, which closes tools.
        assert_eq!(editor.view.compare, Compare::Reference(Axis::LeftRight));
        assert_eq!(editor.view.tool, crate::app::develop::state::Tool::None);
        assert!(editor.reference.loading());
        wait_for_reference(editor);
        // Developed in full, for 100%.
        let side = editor.reference_side().unwrap();
        assert_eq!(side.recipe.exposure, 1.5);
        // Editing changes the Active photo, never the reference.
        editor.document.edit.setup_mut().exposure = -0.5;
        assert_eq!(editor.reference_side().unwrap().recipe.exposure, 1.5);
        // The reference renders in Before's lane, from its own photo.
        editor.schedule_before();
        assert!(editor.preview.before.task.is_running());
        // Opening the reference photo itself keeps it, shown live as it is edited.
        let image = editor.reference_side().unwrap().image;
        editor.document.reset(Some(other));
        editor.document.set_image(image);
        editor.load_reference();
        assert!(!editor.reference.loading());
        editor.document.edit.setup_mut().exposure = 0.25;
        assert_eq!(editor.reference_side().unwrap().recipe.exposure, 0.25);
        editor.document.reset(Some(open));
        assert_eq!(editor.reference_side().unwrap().recipe.exposure, 1.5);
        // An edit saved to it since develops it again.
        let library = editor.library.as_mut().unwrap();
        let path = library.photo(other).unwrap().path.clone();
        let edit = Recipe {
            exposure: -1.,
            ..Default::default()
        };
        library.session.save_edit(
            other,
            &path,
            &crate::storage::Identity::read(&path)?,
            &edit,
            &Default::default(),
            crate::catalog::HistoryUpdate::Keep,
        )?;
        editor.load_reference();
        wait_for_reference(editor);
        assert_eq!(editor.reference_side().unwrap().recipe.exposure, -1.);
        Ok(())
    }

    #[test]
    fn moving_between_photos_keeps_the_reference_on_screen() -> anyhow::Result<()> {
        let mut fixture = fixture()?;
        let (open, other) = (fixture.open, fixture.other);
        let editor = &mut fixture.editor;
        editor.set_reference(other);
        wait_for_reference(editor);
        let shown =
            crate::app::develop::state::Picture::presented(egui::TextureId::Managed(1), [1, 1]);
        editor.preview.before.texture = Some(shown);
        // Left/Right moves the Active photo; the reference stays where it was.
        editor.reference.error = Some("was offline".into());
        // A load still running for it is dropped once it opens.
        editor.reload_reference();
        let stale = editor.reference.load.id();
        editor.develop_catalog_photo(other);
        assert!(!editor.reference.loading());
        editor.reference_ready(stale, Err("stale".into()));
        assert_eq!(editor.document.catalog_photo, Some(other));
        assert!(editor.preview.before.texture.is_some());
        // Open now, it can't be offline; until it decodes, it shows as developed.
        assert_eq!(editor.reference.error, None);
        assert_eq!(editor.reference_side().unwrap().recipe.exposure, 1.5);
        assert_eq!(editor.view.compare, Compare::Reference(Axis::LeftRight));
        // Before beside the edit belongs to the photo, and goes with it.
        editor.set_compare(Compare::SideBySide(Axis::LeftRight));
        let shown =
            crate::app::develop::state::Picture::presented(egui::TextureId::Managed(1), [1, 1]);
        editor.preview.before.texture = Some(shown);
        editor.develop_catalog_photo(open);
        assert!(editor.preview.before.texture.is_none());
        Ok(())
    }

    #[test]
    fn the_reference_follows_its_edit_the_defaults_and_its_copy() -> anyhow::Result<()> {
        let mut fixture = fixture()?;
        let (open, other) = (fixture.open, fixture.other);
        let editor = &mut fixture.editor;
        // An unedited reference follows the raw defaults, as Develop does.
        editor.document.reset(Some(other));
        editor.set_reference(open);
        wait_for_reference(editor);
        editor.set_raw_defaults(crate::raw_defaults::RawDefaults {
            master: crate::raw_defaults::DefaultChoice::Rawmakase,
            ..Default::default()
        })?;
        assert!(editor.reference.loading());
        wait_for_reference(editor);
        // A saved edit is checked as Develop checks it: once the file is replaced,
        // the edit is protected and the reference shows the defaults.
        editor.document.reset(Some(open));
        editor.set_reference(other);
        wait_for_reference(editor);
        assert_eq!(editor.reference_side().unwrap().recipe.exposure, 1.5);
        let path = editor
            .library
            .as_ref()
            .unwrap()
            .photo(other)
            .unwrap()
            .path
            .clone();
        // Gone for a while, it says why and keeps its picture; back, it says nothing.
        let away = path.with_extension("away");
        std::fs::rename(&path, &away)?;
        editor.load_reference();
        assert!(editor.reference.error.is_some());
        assert!(editor.reference_side().is_some());
        std::fs::rename(&away, &path)?;
        editor.load_reference();
        assert_eq!(editor.reference.error, None);
        assert!(!editor.reference.loading());
        // What its edit resolves to changed (profiles imported): developed again.
        editor.reload_reference();
        assert!(editor.reference.loading());
        wait_for_reference(editor);
        let replacement =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/charts/synthetic-a.dng");
        std::fs::copy(replacement, &path)?;
        editor.load_reference();
        wait_for_reference(editor);
        assert_eq!(editor.reference_side().unwrap().recipe.exposure, 0.);
        // A virtual copy as the reference goes when the copy is removed.
        let copy = editor
            .library
            .as_mut()
            .unwrap()
            .create_virtual_copy(other)?
            .value;
        editor.set_reference(copy);
        wait_for_reference(editor);
        editor.remove_virtual_copy(copy);
        assert_eq!(editor.reference.photo, None);
        assert!(editor.reference_side().is_none());
        Ok(())
    }

    #[test]
    fn a_reference_whose_full_decode_failed_is_developed_again() -> anyhow::Result<()> {
        let mut fixture = fixture()?;
        let other = fixture.other;
        let editor = &mut fixture.editor;
        editor.set_reference(other);
        wait_for_reference(editor);
        let image = editor.reference_side().unwrap().image;
        editor.reload_reference();
        let ticket = editor.reference.load.id();
        let half = ReferenceImage {
            image,
            recipe: Recipe::default(),
            resolution: Resolution::Half,
        };
        editor.reference_ready(ticket, Ok(Box::new(half)));
        editor.reference_ready(ticket, Err("full decode failed".into()));
        assert!(!editor.reference.loading());
        editor.load_reference();
        assert!(editor.reference.loading());
        wait_for_reference(editor);
        Ok(())
    }

    #[test]
    fn leaving_develop_clears_the_reference_unless_it_is_locked() -> anyhow::Result<()> {
        // The fixture keeps its folder for as long as it lives.
        let mut fixture = fixture()?;
        let other = fixture.other;
        let editor = &mut fixture.editor;
        editor.set_reference(other);
        editor.left_develop();
        assert_eq!(editor.view.compare, Compare::Off);
        assert_eq!(editor.reference.photo, None);
        assert!(!editor.reference.loading());
        // Locked, it stays for the next visit to Develop.
        editor.set_reference(other);
        editor.reference.locked = true;
        editor.left_develop();
        assert_eq!(editor.reference.photo, Some(other));
        editor.toggle_reference_view();
        assert_eq!(editor.view.compare, Compare::Reference(Axis::LeftRight));
        // Top/bottom is remembered for Shift+R.
        editor.set_reference_layout(Axis::TopBottom);
        editor.toggle_reference_view();
        assert_eq!(editor.view.compare, Compare::Off);
        editor.toggle_reference_view();
        assert_eq!(editor.view.compare, Compare::Reference(Axis::TopBottom));
        Ok(())
    }

    #[test]
    fn a_photo_develop_cannot_open_is_not_a_reference() -> anyhow::Result<()> {
        // The fixture keeps its folder for as long as it lives.
        let mut fixture = fixture()?;
        let other = fixture.other;
        let editor = &mut fixture.editor;
        let path = editor
            .library
            .as_ref()
            .unwrap()
            .photo(other)
            .unwrap()
            .path
            .clone();
        std::fs::remove_file(path)?;
        editor.set_reference(other);
        assert_eq!(editor.reference.photo, None);
        assert_eq!(editor.view.compare, Compare::Off);
        assert!(editor.messages.line().contains("reference"));
        Ok(())
    }

    #[test]
    fn the_reference_has_its_own_zoom() -> anyhow::Result<()> {
        // The fixture keeps its folder for as long as it lives.
        let mut fixture = fixture()?;
        let other = fixture.other;
        let editor = &mut fixture.editor;
        editor.set_reference(other);
        wait_for_reference(editor);
        editor
            .preview
            .before
            .task
            .finish(editor.preview.before.task.id());
        let fit = editor.preview.before.task.id();
        // The Active photo zoomed in, the reference still fitted.
        editor.view.zoom.on = true;
        editor.schedule_before();
        assert_eq!(editor.preview.before.task.id(), fit);
        // The reference at 100%: a 100% region of it, the Active photo unchanged.
        editor.view.zoom.on = false;
        editor.reference.zoom.on = true;
        editor.schedule_before();
        assert!(editor.preview.before.task.id() > fit);
        assert!(editor.preview.before.zoomed());
        assert!(!editor.view.zoom.on);
        Ok(())
    }

    #[test]
    fn shift_r_shows_reference_view_and_r_still_crops() {
        let ctx = egui::Context::default();
        let mut e = Editor::with_context(
            &ctx,
            None,
            crate::app::preferences::session::Session::default(),
            None,
        );
        let press = |modifiers: egui::Modifiers, e: &mut Editor| {
            let key = |pressed| egui::Event::Key {
                key: egui::Key::R,
                physical_key: Some(egui::Key::R),
                pressed,
                repeat: false,
                modifiers,
            };
            let input = egui::RawInput {
                events: vec![
                    egui::Event::ModifiersChanged(modifiers),
                    key(true),
                    key(false),
                ],
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |_| e.develop_shortcuts(&ctx));
            output.textures_delta.clear();
        };
        press(egui::Modifiers::SHIFT, &mut e);
        assert_eq!(e.view.compare, Compare::Reference(Axis::LeftRight));
        assert_eq!(e.view.tool, crate::app::develop::state::Tool::None);
        press(egui::Modifiers::SHIFT, &mut e);
        assert_eq!(e.view.compare, Compare::Off);
        press(egui::Modifiers::NONE, &mut e);
        assert_eq!(e.view.tool, crate::app::develop::state::Tool::Crop);
    }
}
