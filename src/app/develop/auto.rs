//! The Basic panel's Auto, estimated off the UI thread.
use crate::app::{
    Editor,
    history::Step,
    worker::{AutoKind, Event},
};
use crate::develop::AutoTone;
use crate::model::recipe::Recipe;
use rawmakase_cancel::Cancel;

/// Everything an estimate of `kind` was fitted against: `r` without the settings it
/// chooses. Auto tone measures the photo before its adjustments, as its profile, white
/// balance and geometry render it, so only those are inputs.
fn inputs(kind: AutoKind, r: &Recipe) -> Recipe {
    match kind {
        AutoKind::Settings => crate::develop::auto_tone_basis(r),
        AutoKind::WhiteBalance => Recipe {
            wb: [1.; 3],
            temperature: 0.,
            tint: 0.,
            incremental_temperature: 0.,
            incremental_tint: 0.,
            auto_white_balance: None,
            ..r.clone()
        },
    }
}

impl Editor {
    /// Whether the tone sliders, Vibrance and Saturation are as Auto last set them and
    /// nothing Auto measures has changed since, so running it again would change nothing. Adjustments Auto does not
    /// measure (curves, presence, color and the like) leave it in effect.
    pub(in crate::app) fn auto_in_effect(&self) -> bool {
        let r = self.document.edit.recipe();
        if let Some((seen, in_effect)) = &*self.document.auto_effect.borrow()
            && seen == r
        {
            return *in_effect;
        }
        let in_effect = self.document.auto_applied.as_ref().is_some_and(|a| {
            AutoTone::of(a) == AutoTone::of(r)
                && inputs(AutoKind::Settings, a) == inputs(AutoKind::Settings, r)
        });
        *self.document.auto_effect.borrow_mut() = Some((r.clone(), in_effect));
        in_effect
    }

    /// Starts Auto for the open photo; the estimate arrives as [`Event::Auto`]. Does
    /// nothing while the photo is still decoding or an estimate is already running.
    pub(in crate::app) fn start_auto(&mut self, kind: AutoKind) {
        let Some(im) = self.document.full().cloned() else {
            return;
        };
        if self.document.auto.is_running() {
            return;
        }
        let (_, cancel) = self.document.auto.start();
        let id = self.document.load_id();
        let base = self.document.edit.recipe().clone();
        self.document.auto_input = Some(inputs(kind, &base));
        crate::app::task::spawn(
            self.tx.clone(),
            self.context.clone(),
            move |tx| {
                let result = match kind {
                    AutoKind::Settings => {
                        crate::develop::auto_tone(&im, &base, Cancel::new(&cancel))
                    }
                    AutoKind::WhiteBalance => {
                        crate::develop::auto_white_balance(&im, &base, Cancel::new(&cancel))
                    }
                }
                .map(Box::new)
                .map_err(|e| format!("Auto: {e:#}"));
                let _ = tx.send(Event::Auto { id, kind, result });
            },
            // A panic still sends a result, so Auto does not stay disabled.
            move |tx, _| {
                let result = Err("Auto: the estimate failed unexpectedly".to_owned());
                let _ = tx.send(Event::Auto { id, kind, result });
            },
        );
    }

    /// Applies an Auto estimate as one History step. Only the fields Auto sets are
    /// taken, so edits made while it ran are kept.
    pub(in crate::app) fn auto_ready(
        &mut self,
        kind: AutoKind,
        result: Result<Box<Recipe>, String>,
    ) {
        self.document.auto.invalidate();
        // The estimate was fitted to the photo as it was when Auto started. If any other
        // setting changed since, it no longer fits, and neither does an error it hit:
        // run again with the edit kept.
        let fitted = self
            .document
            .auto_input
            .take()
            .or_else(|| result.as_deref().ok().map(|r| inputs(kind, r)));
        if fitted.is_some_and(|f| f != inputs(kind, self.document.edit.recipe())) {
            self.start_auto(kind);
            return;
        }
        let auto = match result {
            Ok(auto) => auto,
            Err(e) => {
                self.messages.fail(e);
                return;
            }
        };
        // A drag still under way is recorded first, so undoing it keeps Auto.
        if self.document.edit.history().in_gesture() {
            self.document.edit.finish_gesture();
        }
        let step = match kind {
            AutoKind::Settings => Step::new("Auto Settings", ""),
            AutoKind::WhiteBalance => Step::new("White Balance", "Auto"),
        };
        let mut applied = self.document.edit.recipe().clone();
        match kind {
            AutoKind::Settings => {
                AutoTone::of(&auto).apply(&mut applied);
                self.document.auto_applied = Some(applied.clone());
                self.document.auto_effect.take();
            }
            AutoKind::WhiteBalance => {
                applied.wb = auto.wb;
                applied.temperature = auto.temperature;
                applied.tint = auto.tint;
                // A JPEG, PNG or TIFF's white balance is relative Temp and Tint.
                applied.incremental_temperature = auto.incremental_temperature;
                applied.incremental_tint = auto.incremental_tint;
                applied.auto_white_balance = auto.auto_white_balance;
            }
        }
        self.change_edit(Some(step), |r| *r = applied);
    }
}

#[cfg(test)]
#[path = "auto_editor_tests.rs"]
mod editor_tests;
