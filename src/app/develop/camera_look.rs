//! Profile → Fit Camera Look: Camera Look (fitted) for the open photo, fitted to its
//! embedded JPEG off the UI thread (`camera_look::fit`, a few seconds).
use crate::app::{Editor, history::Step, worker::Event};
use crate::camera_profiles::CameraProfile;
use crate::develop::black_white::TreatmentChoice;
use rawmakase_cancel::Cancel;
use std::sync::Arc;

/// `listed`, a photo's profiles listed again (after an import), with the fitted
/// default it had (`had`, as the loader listed them) while the photo still has no
/// colour data: listing again never fits, and the defaults stay what they were.
pub(in crate::app) fn keep_fitted_default(
    had: &[Arc<CameraProfile>],
    m: &crate::camera_data::Metadata,
    mut listed: Vec<Arc<CameraProfile>>,
) -> Vec<Arc<CameraProfile>> {
    use crate::camera_profiles::fitted;
    if fitted::eligibility(m, &listed) == fitted::Eligibility::Absent {
        listed.extend(had.iter().filter(|p| fitted::is_fitted(p)).cloned());
    }
    listed
}

impl Editor {
    /// Fits the open photo's camera look; it arrives as [`Event::CameraLook`]. A fit
    /// already running for the photo is cancelled for this one.
    pub(in crate::app) fn start_camera_look(&mut self) {
        let Some(path) = self.document.path.clone() else {
            return;
        };
        let (generation, cancel) = self.document.camera_look.start();
        let id = self.document.load_id();
        let profile = self.document.edit.recipe().profile.clone();
        let sent = profile.clone();
        crate::app::task::spawn(
            self.tx.clone(),
            self.context.clone(),
            move |tx| {
                let inputs = crate::camera_look::FitInputs::default();
                // The file as it was when the fit started: one replaced or changed
                // meanwhile may have been fitted instead of the photo shown.
                let key = crate::camera_look::cache::FitKey::of(&path).ok();
                let result = crate::camera_look::fit(&path, &inputs, Cancel::new(&cancel))
                    .map(|f| Box::new(f.profile))
                    .map_err(|e| e.reason)
                    .and_then(|p| {
                        let same = key.is_some()
                            && crate::camera_look::cache::FitKey::of(&path).ok() == key;
                        match same {
                            true => Ok(p),
                            false => Err("the file changed while it was fitted".into()),
                        }
                    });
                let _ = tx.send(Event::CameraLook {
                    id,
                    generation,
                    profile,
                    result,
                });
            },
            move |tx, message| {
                let _ = tx.send(Event::CameraLook {
                    id,
                    generation,
                    profile: sent,
                    result: Err(message),
                });
            },
        );
    }

    /// Uses a fitted look as the photo's profile, at 100%, as one History step,
    /// unless the profile changed while it was fitted (a later choice wins) or a
    /// newer fit was asked for. A refusal says why, as a failure that stays until dismissed.
    pub(in crate::app) fn camera_look_ready(
        &mut self,
        generation: u64,
        fitted_over: Option<Arc<CameraProfile>>,
        result: Result<Box<CameraProfile>, String>,
    ) {
        if generation != self.document.camera_look.id() {
            return;
        }
        self.document.camera_look.finish(generation);
        let profile = match result {
            Ok(profile) => profile,
            Err(why) => {
                self.messages.fail(format!("No camera look fitted: {why}"));
                return;
            }
        };
        if self.document.edit.recipe().profile != fitted_over {
            self.messages.warn(
                "The profile changed while the camera look was fitted; fit it again to use it",
            );
            return;
        }
        let profile = Arc::new(*profile);
        let metadata = self.document.metadata.clone();
        self.change_edit(Some(Step::new("Fit Camera Look", "")), |r| {
            let old = r.profile.replace(profile);
            r.profile_amount = 1.;
            // Temp and Tint described the old profile's white: as a menu choice does,
            // they follow the new one, the white balance unchanged.
            if let Some(m) = &metadata {
                r.profile_changed(m);
            }
            // Treatment follows the profile in the same step, as a menu choice's does.
            r.follow_profile_treatment(old.as_deref(), None);
        });
        self.messages.done("Camera Look (fitted) applied");
    }

    /// Whether Profile → Fit Camera Look can run for the open photo: a RAW, decoded
    /// far enough to know it is one, with no fit already running.
    pub(in crate::app) fn can_fit_camera_look(&self) -> bool {
        self.document.path.is_some()
            && self
                .document
                .metadata
                .as_ref()
                .is_some_and(|m| !m.is_rendered())
            && !self.document.camera_look.is_running()
    }
}

#[cfg(test)]
#[path = "camera_look_editor_tests.rs"]
mod editor_tests;
