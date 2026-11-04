//! Develop's adjustment panels, one file each, in the order `Editor::controls`
//! draws them. A panel edits the recipe it is given directly, during the
//! editor's edit frame, so its changes become History steps as any slider's;
//! what it can't do itself, such as starting Auto or opening a file dialog, it
//! returns as [`Action`]s for the editor to carry out after every panel is drawn.
pub(super) mod basic;
pub(super) mod calibration;
pub(super) mod color_grading;
pub(super) mod detail;
pub(super) mod effects;
pub(super) mod lens_corrections;
pub(super) mod mixer;
pub(super) mod tone_curve;
pub(super) mod transform;

use super::{ai_denoise, lensfun};
use crate::app::develop::panels::curve_menu::CurveChoice;
use crate::app::develop::profile_download::OfferAction;
use crate::app::presets::bulk_import::ImportKind;
use crate::app::worker::AutoKind;
use crate::camera_profiles::CameraProfile;
use crate::develop::targeted::Target;
use crate::model::recipe::Treatment;
use std::sync::Arc;
use transform::GuidedAction;

/// What a panel asks of the editor.
#[derive(Clone, Debug)]
pub(super) enum Action {
    /// Basic's Auto, or Auto in its White Balance menu.
    Auto(AutoKind),
    /// The profile changed this frame; it was this one before.
    ProfileChanged(Option<Arc<CameraProfile>>),
    Treatment(Treatment),
    FitCameraLook,
    /// B&W's Auto mix.
    AutoMix,
    /// A choice in the Point Curve menu.
    Curve(CurveChoice),
    /// A Targeted Adjustment Tool button.
    Targeted(Target),
    /// Analyse the photo for Upright.
    Upright,
    Guided(GuidedAction),
    ImportProfiles,
    ShowCommunityProfiles,
    Offer(OfferAction),
    ImportLens,
    AiDenoise(ai_denoise::Action),
    Lensfun(lensfun::Action),
    ImportFolder(ImportKind),
    ImportLuts,
    ImportAdobe,
}

/// The actions of one frame's panels, one of each kind (the last asked), which
/// the editor carries out in a fixed order.
#[derive(Default)]
pub(super) struct Requests {
    pub auto: Option<AutoKind>,
    pub profile_changed_from: Option<Option<Arc<CameraProfile>>>,
    pub treatment: Option<Treatment>,
    pub fit_camera_look: bool,
    pub auto_mix: bool,
    pub curve: Option<CurveChoice>,
    pub targeted: Option<Target>,
    pub upright: bool,
    pub guided: Option<GuidedAction>,
    pub import_profiles: bool,
    pub show_community_profiles: bool,
    pub offer: Option<OfferAction>,
    pub import_lens: bool,
    pub ai_denoise: Option<ai_denoise::Action>,
    pub lensfun: Option<lensfun::Action>,
    pub import_folder: Option<ImportKind>,
    pub import_luts: bool,
    pub import_adobe: bool,
}

impl Requests {
    /// Adds what a panel asked for, in the order it asked.
    pub(super) fn add(&mut self, actions: Vec<Action>) {
        for action in actions {
            match action {
                Action::Auto(kind) => self.auto = Some(kind),
                Action::ProfileChanged(old) => self.profile_changed_from = Some(old),
                Action::Treatment(t) => self.treatment = Some(t),
                Action::FitCameraLook => self.fit_camera_look = true,
                Action::AutoMix => self.auto_mix = true,
                Action::Curve(choice) => self.curve = Some(choice),
                Action::Targeted(target) => self.targeted = Some(target),
                Action::Upright => self.upright = true,
                Action::Guided(guided) => self.guided = Some(guided),
                Action::ImportProfiles => self.import_profiles = true,
                Action::ShowCommunityProfiles => self.show_community_profiles = true,
                Action::Offer(offer) => self.offer = Some(offer),
                Action::ImportLens => self.import_lens = true,
                Action::AiDenoise(ai) => self.ai_denoise = Some(ai),
                Action::Lensfun(lensfun) => self.lensfun = Some(lensfun),
                Action::ImportFolder(kind) => self.import_folder = Some(kind),
                Action::ImportLuts => self.import_luts = true,
                Action::ImportAdobe => self.import_adobe = true,
            }
        }
    }
}
