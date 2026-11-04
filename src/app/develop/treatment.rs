//! The Basic panel's Treatment and the B&W panel's Auto. Each runs during an edit
//! frame, which records it as one History step under the name it gives.
use crate::app::{Editor, history::Step};
use crate::develop::black_white::TreatmentChoice;
use crate::model::recipe::Treatment;
use crate::{
    camera_data::Metadata,
    develop::{AutoMix, ColorSpread},
};

/// What converting to black & white does to a mix that was never set: Lightroom's
/// "Apply auto mix when first converting to black and white" preference, on by
/// default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::app) enum FirstConversion {
    #[default]
    AutoMix,
    KeepMix,
}

/// What Auto black & white measures for the open photo: its colors and camera.
pub(in crate::app) struct PhotoColors {
    spread: ColorSpread,
    metadata: Metadata,
}

impl PhotoColors {
    pub(in crate::app) fn auto_mix(&self) -> AutoMix<'_> {
        AutoMix {
            spread: &self.spread,
            metadata: &self.metadata,
        }
    }
}

impl Editor {
    /// The open photo's colors, once it is decoded.
    pub(in crate::app) fn photo_colors(&self) -> Option<PhotoColors> {
        Some(PhotoColors {
            spread: self.document.color_spread()?,
            metadata: self.document.metadata.clone()?,
        })
    }

    /// The Auto mix a first conversion applies: none while the preference is off.
    pub(in crate::app) fn first_conversion_colors(&self) -> Option<PhotoColors> {
        match self.session.first_conversion {
            FirstConversion::AutoMix => self.photo_colors(),
            FirstConversion::KeepMix => None,
        }
    }

    /// Lightroom's Treatment switcher, and V: converting to black & white applies
    /// the Auto mix the first time, if the preference asks for it. Called during an
    /// edit frame, which records the change as one History step under this name.
    /// While the photo is still decoding, a conversion that needs the Auto mix waits
    /// for it ([`Editor::finish_pending_treatment`]).
    pub(in crate::app) fn set_treatment(&mut self, treatment: Treatment) {
        self.document.pending_treatment = None;
        if self.document.edit.recipe().treatment() == treatment {
            return;
        }
        let needs_auto = treatment == Treatment::BlackWhite
            && self.session.first_conversion == FirstConversion::AutoMix
            && self.document.edit.recipe().effects.gray_mix == [0.; 8];
        // Measured only when the conversion uses it.
        let colors = needs_auto.then(|| self.photo_colors()).flatten();
        if needs_auto && colors.is_none() {
            self.document.pending_treatment = Some(crate::app::develop::state::PendingTreatment {
                treatment,
                recipe: self.document.edit.recipe().clone(),
            });
            self.messages
                .note("Converting to Black & White once the photo is decoded");
            return;
        }
        let first = colors.as_ref().map(PhotoColors::auto_mix);
        let color_profile = self.photo_defaults().and_then(|d| d.recipe.profile);
        let document = &mut self.document;
        match &document.metadata {
            Some(m) => {
                document
                    .edit
                    .recipe_mut()
                    .choose_treatment(treatment, first, color_profile, m)
            }
            None => document.edit.recipe_mut().set_treatment(treatment, first),
        }
        let name = match treatment {
            Treatment::Color => "Convert to Color",
            Treatment::BlackWhite => "Convert to Black & White",
        };
        self.document.edit.name_next_step(Step::new(name, ""));
    }

    /// Converts as asked while the photo was decoding, once it is decoded. Called
    /// during an edit frame.
    pub(in crate::app) fn finish_pending_treatment(&mut self) {
        if self.document.full().is_some()
            && let Some(pending) = self.document.pending_treatment.take()
            && pending.recipe == *self.document.edit.recipe()
        {
            self.set_treatment(pending.treatment);
        }
    }

    /// Keeps the Treatment with a newly chosen profile (see
    /// [`TreatmentChoice::follow_profile_treatment`]). Called during the edit frame that
    /// changed the profile, so it is part of that step.
    pub(in crate::app) fn follow_profile_treatment(
        &mut self,
        old: Option<&crate::camera_profiles::CameraProfile>,
    ) {
        let r = self.document.edit.recipe();
        // Measured only for a first conversion by a black & white profile.
        let converts = crate::model::recipe::is_monochrome(r.profile.as_deref())
            && !crate::model::recipe::is_monochrome(old)
            && !r.effects.monochrome
            && r.effects.gray_mix == [0.; 8];
        let colors = converts.then(|| self.first_conversion_colors()).flatten();
        let first = colors.as_ref().map(PhotoColors::auto_mix);
        self.document
            .edit
            .recipe_mut()
            .follow_profile_treatment(old, first);
    }

    /// V: switches between Color and Black & White.
    pub(in crate::app) fn toggle_treatment(&mut self) {
        // A conversion still waiting for the photo counts as done: V again cancels it.
        let shown = self
            .document
            .pending_treatment
            .take()
            .map_or_else(|| self.document.edit.recipe().treatment(), |p| p.treatment);
        self.set_treatment(match shown {
            Treatment::Color => Treatment::BlackWhite,
            Treatment::BlackWhite => Treatment::Color,
        });
    }

    /// The B&W panel's Auto: sets the black & white mix from the photo's colors.
    /// Called during an edit frame, which records it as one History step.
    pub(in crate::app) fn auto_black_white_mix(&mut self) {
        let Some(colors) = self.photo_colors() else {
            return;
        };
        let mix = colors.auto_mix().for_recipe(self.document.edit.recipe());
        if self.document.edit.recipe().effects.gray_mix != mix {
            self.document.edit.recipe_mut().effects.gray_mix = mix;
            self.document
                .edit
                .name_next_step(Step::new("Black & White Mix", "Auto"));
        }
    }
}

#[cfg(test)]
#[path = "treatment_editor_tests.rs"]
mod editor_tests;
