//! The Library's progress line for preview work.
use super::thumbnails::PreviewResult;
use crate::app::ui::widgets::plural;
use eframe::egui;

#[derive(Default)]
pub(in crate::app::library) struct Progress {
    pub(super) total: usize,
    pub(super) completed: usize,
    pub(super) failed: usize,
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
        self.failed += usize::from(result.image.is_none() && !result.skipped);
        if let Some(error) = &result.cache_error {
            self.cache_error = Some(error.clone());
        }
    }

    /// A preview could not be kept in the cache.
    pub(crate) fn cache_failed(&mut self, error: String) {
        self.cache_error = Some(error);
    }

    /// Worth a status line: still working, or something could not be prepared.
    pub(crate) fn active(&self) -> bool {
        self.completed < self.total || self.failed > 0 || self.cache_error.is_some()
    }
    /// One line of small text, the same height as the status row it sits in.
    /// In a right-to-left layout, so the spinner comes first and sits right.
    /// `ahead`: edited previews of photos not shown yet are being rendered.
    pub(crate) fn show(&self, ui: &mut egui::Ui, edits_pending: usize, ahead: bool) {
        let building = self.completed < self.total || edits_pending > 0 || ahead;
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
        if self.failed > 0 {
            ui.small(format!("{} previews unavailable", self.failed))
                .on_hover_text(
                    "The original may be offline, damaged, or have no usable embedded preview.",
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
