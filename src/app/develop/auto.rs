//! The Basic panel's Auto, the White Balance menu's and the Negative panel's,
//! estimated off the UI thread.
use crate::app::develop::state::NegativeHolder;
use crate::app::{
    Editor,
    history::Step,
    worker::{AutoKind, Event},
};
use crate::develop::AutoTone;
use crate::model::negative::NegativeConversion;
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
        // The levels are measured through the rest of the conversion: its film and
        // base, and the crop it samples.
        AutoKind::Negative => Recipe {
            negative: r.negative.as_ref().map(|n| NegativeConversion {
                dmin: [0.; 3],
                dmax: [0.; 3],
                ..n.clone()
            }),
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
    /// nothing while the photo is still decoding or an estimate is already running,
    /// nor the Negative panel's for a photo without a conversion.
    pub(in crate::app) fn start_auto(&mut self, kind: AutoKind) {
        let Some(im) = self.document.full().cloned() else {
            return;
        };
        if self.document.auto.is_running() {
            return;
        }
        if kind == AutoKind::Negative && self.document.edit.recipe().negative.is_none() {
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
                let mut holder_in_crop = false;
                let result = match kind {
                    AutoKind::Settings => {
                        crate::develop::auto_tone(&im, &base, Cancel::new(&cancel))
                    }
                    AutoKind::WhiteBalance => {
                        crate::develop::auto_white_balance(&im, &base, Cancel::new(&cancel))
                    }
                    AutoKind::Negative => {
                        crate::develop::auto_negative(&im, &base, Cancel::new(&cancel)).map(
                            |levels| {
                                holder_in_crop = levels.holder_in_crop;
                                Recipe {
                                    negative: Some(levels.conversion),
                                    ..base.clone()
                                }
                            },
                        )
                    }
                }
                .map(Box::new)
                .map_err(|e| format!("Auto: {e:#}"));
                let _ = tx.send(Event::Auto {
                    id,
                    kind,
                    result,
                    holder_in_crop,
                });
            },
            // A panic still sends a result, so Auto does not stay disabled.
            move |tx, _| {
                let result = Err("Auto: the estimate failed unexpectedly".to_owned());
                let _ = tx.send(Event::Auto {
                    id,
                    kind,
                    result,
                    holder_in_crop: false,
                });
            },
        );
    }

    /// Applies an Auto estimate as one History step. Only the fields Auto sets are
    /// taken, so edits made while it ran are kept. The Negative panel's says, beside
    /// its levels, when the crop seemed to hold part of the film holder
    /// (`holder_in_crop`), until the crop changes or Auto runs again.
    pub(in crate::app) fn auto_ready(
        &mut self,
        kind: AutoKind,
        result: Result<Box<Recipe>, String>,
        holder_in_crop: bool,
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
                if kind == AutoKind::Negative {
                    self.view.negative_holder = None;
                }
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
            AutoKind::Negative => Step::new("Negative", "Auto"),
        };
        let mut applied = self.document.edit.recipe().clone();
        match kind {
            AutoKind::Settings => {
                AutoTone::of(&auto).apply(&mut applied);
                self.document.auto_applied = Some(applied.clone());
                self.document.auto_effect.take();
            }
            AutoKind::WhiteBalance => apply_white_balance(&auto, &mut applied),
            AutoKind::Negative => {
                apply_negative_levels(&auto, &mut applied);
                self.view.negative_holder = holder_in_crop.then(|| NegativeHolder {
                    load: self.document.load_id(),
                    crop: applied.crop,
                });
            }
        }
        // With Auto Sync on, the other selected photos get an Auto of their own:
        // the steps before are taken first, so only Auto's is measured.
        self.sync_undo();
        self.auto_sync_measure(kind);
        self.change_edit(Some(step), |r| *r = applied);
        self.sync_undo();
        self.auto_sync_measure_done();
    }
}

/// Auto White Balance's estimate `auto` set on `to`: only the white balance.
pub(in crate::app) fn apply_white_balance(auto: &Recipe, to: &mut Recipe) {
    to.wb = auto.wb;
    to.temperature = auto.temperature;
    to.tint = auto.tint;
    // A JPEG, PNG or TIFF's white balance is relative Temp and Tint.
    to.incremental_temperature = auto.incremental_temperature;
    to.incremental_tint = auto.incremental_tint;
    to.auto_white_balance = auto.auto_white_balance;
}

/// The Negative panel's Auto estimate `auto` set on `to`: only the conversion's
/// levels, so its film and base stay as they are.
pub(in crate::app) fn apply_negative_levels(auto: &Recipe, to: &mut Recipe) {
    if let (Some(n), Some(a)) = (to.negative.as_mut(), auto.negative.as_ref()) {
        n.dmin = a.dmin;
        n.dmax = a.dmax;
    }
}

#[cfg(test)]
#[path = "auto_editor_tests.rs"]
mod editor_tests;
