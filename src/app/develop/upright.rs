//! The Transform panel's Upright analysis, run off the UI thread.
use crate::app::{Editor, worker::Event};
use crate::model::recipe::Recipe;
use crate::model::transform::UprightMode;

/// What the analysis measures: the photo's orientation and lens correction.
pub(in crate::app) fn inputs(r: &Recipe) -> (u8, bool, bool, crate::model::transform::LensInputs) {
    (
        r.rotation,
        r.flip_x,
        r.flip_y,
        crate::model::transform::LensInputs::of(r),
    )
}

impl Editor {
    /// Analyses the open photo for Upright; the corrections arrive as
    /// [`Event::Upright`]. Does nothing while the photo is still decoding.
    pub(in crate::app) fn start_upright(&mut self) {
        let Some(im) = self.document.full().cloned() else {
            return;
        };
        let (generation, _) = self.document.upright.start();
        let id = self.document.load_id();
        let base = self.document.edit.recipe().clone();
        let analysed = base.clone();
        crate::app::task::spawn(
            self.tx.clone(),
            self.context.clone(),
            move |tx| {
                let result = Ok(crate::develop::upright::analyse(&im, &base));
                let _ = tx.send(Event::Upright {
                    id,
                    generation,
                    analysed: Box::new(base),
                    result,
                });
            },
            // A panic still sends a result, so Upright does not wait forever.
            move |tx, _| {
                let _ = tx.send(Event::Upright {
                    id,
                    generation,
                    analysed: Box::new(analysed),
                    result: Err("Upright: the analysis failed unexpectedly".to_owned()),
                });
            },
        );
    }

    /// Measures what the edit needs from the photo and lacks: a Straighten Auto copied
    /// from another photo, and Upright's corrections.
    pub(in crate::app) fn ensure_analysis(&mut self) {
        self.ensure_straighten();
        self.ensure_upright();
    }

    /// Analyses the photo when an Upright mode is chosen but has no correction yet, as
    /// after undoing to a state from before an analysis, or opening such a photo.
    /// Guided's guides are solved once the other modes' corrections are there to sit
    /// beside its own.
    pub(in crate::app) fn ensure_upright(&mut self) {
        let u = &self.document.edit.recipe().upright;
        if !u.needs_analysis() || self.document.upright.is_running() {
            return;
        }
        if u.mode == UprightMode::Guided && u.corrections.len() == UprightMode::Guided.code() {
            if let Some(im) = self.document.full().cloned()
                && let Some(issue) =
                    crate::develop::guided::store(self.document.edit.analysed_mut(), &im.metadata)
            {
                self.messages.warn(issue.message());
            }
            return;
        }
        self.start_upright();
    }

    /// Stores Upright's corrections for every mode. The mode was chosen when the analysis
    /// started, so the photo only changes if a mode is selected.
    pub(in crate::app) fn upright_ready(
        &mut self,
        generation: u64,
        analysed: &Recipe,
        result: Result<Vec<[f32; 9]>, String>,
    ) {
        if generation != self.document.upright.id() {
            return;
        }
        self.document.upright.finish(generation);
        // Settings applied meanwhile (a preset, a History step) may bring their own
        // corrections; those win.
        if self.document.edit.recipe().upright.corrections != analysed.upright.corrections {
            self.ensure_upright();
            return;
        }
        if inputs(analysed) != inputs(self.document.edit.recipe()) {
            self.start_upright();
            return;
        }
        let corrections = match result {
            Ok(c) => c,
            Err(e) => {
                self.messages.fail(e);
                return;
            }
        };
        // The analysis is part of the photo, not an edit: every state in History that
        // it fits gets it too, so undoing the mode choice leaves nothing half-applied.
        // States with corrections of their own (imported from Lightroom, or another
        // analysis) keep them.
        let fits = |r: &Recipe| {
            inputs(r) == inputs(analysed) && r.upright.corrections == analysed.upright.corrections
        };
        let metadata = self.document.full().map(|im| im.metadata.clone());
        let mut issue = None;
        for (i, r) in self
            .document
            .edit
            .states_mut()
            // A Before copied from the edit before the analysis arrived.
            .chain(self.document.before.as_mut())
            .enumerate()
        {
            if fits(r) {
                let solved = crate::develop::upright::store(r, &corrections, metadata.as_ref());
                // Only the photo as shown speaks in the top bar.
                if i == 0 {
                    issue = solved;
                }
            }
        }
        if let Some(issue) = issue {
            self.messages.warn(issue.message());
        }
        self.document.edit.save_state_mut().mark_changed();
        self.schedule();
    }
}

#[cfg(test)]
#[path = "upright_editor_tests.rs"]
mod editor_tests;
