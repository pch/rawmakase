//! Holding a slider's label shows the photo with that slider at its default until
//! the button is released. A label held ([`SliderEvent::Peeked`]) sets the default
//! in the settings like a reset would, side effects on related settings included;
//! [`Editor::with_peek`] takes the settings as drawn for the preview and puts the
//! edit back before anything else in the frame runs, so History, saving and
//! commands never see it.
//!
//! [`SliderEvent::Peeked`]: crate::app::ui::widgets::SliderEvent::Peeked
use crate::app::{
    Editor,
    ui::widgets::{PeekScope, take_peeked},
};
use crate::model::recipe::Recipe;
use eframe::egui;

/// What a held slider label shows in place of the edit.
#[derive(Default)]
pub(in crate::app) struct Peek {
    recipe: Option<Recipe>,
    /// The sliders were drawn this frame, and said whether one is held.
    drawn: bool,
}
impl Peek {
    pub(in crate::app) fn is_peeking(&self) -> bool {
        self.recipe.is_some()
    }
}

impl Editor {
    /// The settings the photo is shown with: a held slider label's, else a hovered
    /// preset's, else the edit's.
    pub(in crate::app) fn settings_on_screen(&self) -> &Recipe {
        self.preview
            .peek
            .recipe
            .as_ref()
            .or(self.presets.preview.as_ref())
            .unwrap_or(self.document.edit.recipe())
    }
    /// Draws the edit's sliders with `add`, whose labels peek while held.
    pub(in crate::app) fn with_peek<T>(
        &mut self,
        ui: &mut egui::Ui,
        add: impl FnOnce(&mut Self, &mut egui::Ui) -> T,
    ) -> T {
        // Only a held button peeks: other frames copy no settings.
        let before = ui
            .input(|i| i.pointer.primary_down())
            .then(|| (self.document.load_id(), self.document.edit.recipe().clone()));
        let (out, peeked) = {
            let _scope = PeekScope::allow(ui);
            take_peeked(ui);
            let out = add(self, ui);
            (out, take_peeked(ui))
        };
        if peeked {
            // A peek is no edit: its name would go to the next one.
            self.step_name.discard();
        }
        let recipe = match before {
            Some((document, before)) if peeked && document == self.document.load_id() => {
                self.document.edit.take_peek(before)
            }
            _ => None,
        };
        self.preview.peek.drawn = true;
        self.show_peek(recipe);
        out
    }
    /// Ends a peek whose sliders this frame did not draw.
    pub(in crate::app) fn end_peek_not_drawn(&mut self) {
        if !std::mem::take(&mut self.preview.peek.drawn) {
            self.show_peek(None);
        }
    }
    fn show_peek(&mut self, recipe: Option<Recipe>) {
        if self.preview.peek.recipe != recipe {
            self.preview.peek.recipe = recipe;
            self.schedule();
        }
    }
}

#[cfg(test)]
#[path = "peek_editor_tests.rs"]
mod editor_tests;
