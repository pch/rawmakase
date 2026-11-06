//! Develop: the open photo, its edit, its rendering and the views of it.
pub(crate) mod ai_denoise;
pub(in crate::app) mod auto;
pub(in crate::app) mod autosave;
pub(in crate::app) mod before_after;
pub(in crate::app) mod camera_look;
pub(in crate::app) mod camera_thumbnail;
pub(in crate::app) mod clipping;
pub(in crate::app) mod editing;
pub(in crate::app) mod hdr_preview;
pub(in crate::app) mod lensfun;
#[cfg(test)]
pub(in crate::app) mod one_shot_worker_tests;
pub(in crate::app) mod panels;
pub(in crate::app) mod profile_download;
pub(in crate::app) mod readout;
pub(in crate::app) mod reference;
pub(in crate::app) mod settings_transfer;
pub(in crate::app) mod stand_in;
pub(in crate::app) mod state;
pub(in crate::app) mod step_name;
#[cfg(test)]
pub(in crate::app) mod step_name_tests;
pub(in crate::app) mod toolbar;
pub(in crate::app) mod tools;
pub(in crate::app) mod treatment;
pub(in crate::app) mod upright;
pub(in crate::app) mod viewport;
pub(in crate::app) mod workflow;
