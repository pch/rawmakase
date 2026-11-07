//! Typed application results. Transports choose how to encode these snapshots.
use super::output::OutputState;
use crate::catalog::PhotoId;
use crate::color::curve::ToneCurve;
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Serialize)]
pub(in crate::app) struct State {
    pub protocol: u32,
    pub message: String,
    pub mode: &'static str,
    pub photo: Option<String>,
    pub photo_id: Option<PhotoId>,
    pub generation: u64,
    pub revision: u64,
    pub loaded: bool,
    /// The photo Develop could not open: it will not load, so a client waiting
    /// for `loaded` stops.
    pub open_failed: Option<PhotoId>,
    /// Why it could not be opened, kept while `message` moves on.
    pub open_error: Option<String>,
    pub busy: bool,
    pub modal: bool,
    pub black_and_white: bool,
    pub mixer_channel: &'static str,
    /// The develop preset last applied to the open photo, as the Presets panel names it.
    pub preset: Option<String>,
    pub values: BTreeMap<String, f64>,
    pub masks: Vec<MaskState>,
    pub tone_curve: Curves,
    pub selected_mask: Option<usize>,
    pub exporting: bool,
    /// Previews being built: how many are done of how many queued.
    pub building_previews: Option<BuildProgress>,
    /// Editable previews being built: how many are done of how many queued.
    pub building_editable_previews: Option<BuildProgress>,
    pub auto_running: bool,
    pub treatment_pending: bool,
    pub save_state: &'static str,
    /// Which of the open module's panels show: `left`, `right` and `filmstrip`,
    /// each `shown` or `hidden`; all hidden while the chrome is.
    pub panels: crate::app::shell::panels::PanelLayout,
    /// Whether the whole-window mode hides the chrome: `shown` or `hidden`.
    pub chrome: &'static str,
}
#[derive(Debug, Serialize)]
pub(in crate::app) struct BuildProgress {
    pub done: usize,
    pub total: usize,
}
#[derive(Debug, Serialize)]
pub(in crate::app) struct MaskState {
    pub index: usize,
    pub name: String,
    pub hidden: bool,
    pub values: BTreeMap<String, f32>,
}
#[derive(Debug, Serialize)]
pub(in crate::app) struct Curves {
    pub rgb: ToneCurve,
    pub red: ToneCurve,
    pub green: ToneCurve,
    pub blue: ToneCurve,
}
#[derive(Debug, Serialize)]
pub(in crate::app) struct Reply {
    pub state: State,
    pub result: Outcome,
    pub status: &'static str,
}
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub(in crate::app) enum Outcome {
    Empty,
    Capabilities(Capabilities),
    Photos {
        total: usize,
        offset: usize,
        photos: Vec<PhotoSummary>,
    },
    Search {
        query: String,
    },
    Presets {
        presets: Vec<super::preset::PresetSummary>,
    },
    Preset {
        applied: crate::app::presets::AppliedPreset,
    },
    Opened {
        opened: PhotoIdentity,
    },
    Saved {
        saved: bool,
    },
    Output(OutputState),
    /// Previews queued by `build_previews` or `build_editable_previews`;
    /// queueing is not building.
    Previews {
        queued: usize,
        skipped: usize,
    },
    /// What `discard_editable_previews` did: the photos (masters) whose
    /// editable previews went, the builds of them it stopped, and the photos
    /// whose editable previews it kept as their originals are offline.
    EditablePreviewsDiscarded {
        discarded: usize,
        stopped: usize,
        kept_offline: usize,
    },
}
#[derive(Debug, Serialize)]
pub(in crate::app) struct PhotoIdentity {
    pub id: PhotoId,
    pub filename: String,
    pub path: std::path::PathBuf,
}
#[derive(Debug, Serialize)]
pub(in crate::app) struct PhotoSummary {
    #[serde(flatten)]
    pub identity: PhotoIdentity,
    pub rating: i32,
    pub flag: i32,
    pub label: String,
}
impl From<&crate::catalog::Photo> for PhotoIdentity {
    fn from(p: &crate::catalog::Photo) -> Self {
        Self {
            id: p.id,
            filename: p.filename.clone(),
            path: p.path.clone(),
        }
    }
}
impl From<&crate::catalog::Photo> for PhotoSummary {
    fn from(p: &crate::catalog::Photo) -> Self {
        Self {
            identity: p.into(),
            rating: p.rating,
            flag: p.flag,
            label: p.label.clone(),
        }
    }
}
#[derive(Debug, Serialize)]
pub(in crate::app) struct Capabilities {
    pub protocol: u32,
    pub actions: Vec<&'static str>,
    pub parameters: Vec<Parameter>,
    pub commands: &'static [&'static str],
    pub tone_curve: CurveCapabilities,
    pub target_fields: &'static [&'static str],
    pub completion: &'static str,
    pub headless: bool,
}
#[derive(Debug, Serialize)]
pub(in crate::app) struct CurveCapabilities {
    pub channels: [&'static str; 4],
    pub point_count: [usize; 2],
    pub coordinate_range: [u8; 2],
    pub minimum_input_spacing: f32,
    pub interpolation: &'static str,
}
#[derive(Debug, Serialize)]
pub(in crate::app) struct Parameter {
    pub name: String,
    pub unit: &'static str,
    pub min: f32,
    pub max: f32,
    pub mask: bool,
    pub mask_unit: &'static str,
    pub mask_min: f32,
    pub mask_max: f32,
}
