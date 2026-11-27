//! A scanned negative's conversion in Develop: the film base picker's click and
//! what is said of it. The Negative panel draws the controls
//! (`panels::inspector::panels::negative`); its Auto runs as the other Autos do
//! (`develop::auto`, `AutoKind::Negative`).
use crate::app::Editor;
use crate::app::develop::state::Tool;
use crate::develop::FilmBaseRefusal;

/// The Negative panel's History steps are named this, with what changed as their
/// value.
pub(in crate::app) const STEP: &str = "Negative";

/// Why a click of the film base picker read no base, as the panel and the status
/// line say it.
pub(in crate::app) fn refusal_text(refusal: FilmBaseRefusal) -> &'static str {
    match refusal {
        FilmBaseRefusal::Clipped => {
            "That spot is clipped: click the clear film edge, not the light around it"
        }
        FilmBaseRefusal::Dark => "That spot let no light through: click the clear film edge",
    }
}

impl Editor {
    /// The film base picker's click at (`u`, `v`) of the photo as shown, during the
    /// edit frame: the conversion's base read from the decoded scan there
    /// (`develop::film_base_pick`), as one History step, and the picker put away. A
    /// patch that holds no base changes nothing and leaves the picker out, and the
    /// panel and the status line say why.
    pub(in crate::app) fn pick_film_base(&mut self, u: f32, v: f32) {
        let Some(im) = self.document.full().cloned() else {
            return;
        };
        if self.document.edit.recipe().negative.is_none() {
            self.view.set_tool(Tool::None);
            return;
        }
        match crate::develop::film_base_pick(&im, self.document.edit.recipe(), u, v) {
            Ok(base) => {
                let recipe = self.document.edit.recipe_mut();
                if let Some(n) = recipe.negative.as_mut()
                    && n.base != base
                {
                    n.base = base;
                    self.step_name.name(STEP.into(), "Film Base".into());
                }
                self.view.set_tool(Tool::None);
            }
            Err(refusal) => {
                self.messages
                    .warn(format!("Film base not picked. {}", refusal_text(refusal)));
                self.view.film_base_refusal = Some(refusal);
            }
        }
    }
}

#[cfg(test)]
#[path = "negative_editor_tests.rs"]
mod editor_tests;
