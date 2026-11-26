//! The Library's progress line for preview work.
use super::thumbnails::PreviewResult;
use crate::app::ui::widgets::plural;
use eframe::egui;
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

#[derive(Default)]
pub(in crate::app::library) struct Progress {
    pub(super) total: usize,
    pub(super) completed: usize,
    /// Files no preview could be made of: neither their embedded one nor,
    /// for those developed in its place, a render.
    pub(super) failed: HashSet<PathBuf>,
    pub(super) cache_error: Option<String>,
}

impl Progress {
    pub(crate) fn queued(&mut self) {
        if self.completed == self.total {
            *self = Self::default();
        }
        self.total += 1;
    }

    pub(crate) fn finish(&mut self, result: &PreviewResult) {
        self.completed += 1;
        if result.image.is_none() && !result.skipped {
            self.failed.insert(result.path.clone());
        }
        if let Some(error) = &result.cache_error {
            self.cache_error = Some(error.clone());
        }
    }

    /// The file at `path` is being developed in place of its embedded
    /// preview, so it is not unavailable unless that fails too.
    pub(crate) fn developing(&mut self, path: &Path) {
        self.failed.remove(path);
    }
    /// A preview of the file at `path` is shown, if not its embedded one.
    pub(crate) fn shown(&mut self, path: &Path) {
        self.failed.remove(path);
    }
    /// Developing the file at `path` in place of its embedded preview failed.
    pub(crate) fn not_developed(&mut self, path: &Path) {
        self.failed.insert(path.to_path_buf());
    }

    /// A preview could not be kept in the cache.
    pub(crate) fn cache_failed(&mut self, error: String) {
        self.cache_error = Some(error);
    }

    /// Worth a status line: still working, or something could not be prepared.
    pub(crate) fn active(&self) -> bool {
        self.completed < self.total || !self.failed.is_empty() || self.cache_error.is_some()
    }
    /// One line of small text, the same height as the status row it sits in.
    /// In a right-to-left layout, so the spinner comes first and sits right.
    /// `ahead`: edited previews of photos not shown yet are being rendered.
    /// `developing`: photos without an edit being developed in place of an
    /// embedded preview their files can't give.
    pub(crate) fn show(
        &self,
        ui: &mut egui::Ui,
        edits_pending: usize,
        ahead: bool,
        developing: usize,
    ) {
        let building = self.completed < self.total || edits_pending > 0 || ahead || developing > 0;
        if building {
            crate::app::ui::widgets::spinner(ui, 11.);
        }
        if edits_pending > 0 {
            ui.small(format!(
                "Rendering {}",
                plural(edits_pending, "edited preview", "edited previews")
            ))
            .on_hover_text(
                "Photos with Lightroom or RAWmakase edits are rendered with them, \
                 visible ones first. Each is kept, so this happens once.",
            );
        }
        if developing > 0 {
            ui.small(format!(
                "Rendering {}",
                plural(developing, "preview", "previews")
            ))
            .on_hover_text(
                "Photos whose files have no embedded preview RAWmakase can show are \
                 rendered with your raw defaults, as Develop would open them. Each is \
                 kept, so this happens once.",
            );
        }
        if ahead && edits_pending == 0 {
            ui.small("Rendering edited previews ahead").on_hover_text(
                "While the Library is in front, edited previews of photos not shown yet \
                 are rendered and kept, so they are ready when you scroll to them.",
            );
        }
        if self.completed < self.total {
            ui.small(format!("Preparing previews {} / {}", self.completed, self.total))
                .on_hover_text("Cached previews load first; missing ones are built in the background as you browse.");
        }
        if !self.failed.is_empty() {
            ui.small(format!(
                "{} unavailable",
                plural(self.failed.len(), "preview", "previews")
            ))
            .on_hover_text(
                "The original may be offline or damaged, or neither its embedded preview \
                 nor the photo itself could be read.",
            );
        }
        if let Some(error) = &self.cache_error {
            ui.colored_label(
                ui.visuals().warn_fg_color,
                egui::RichText::new("Preview cache could not be updated").small(),
            )
            .on_hover_text(error);
        }
    }
}
