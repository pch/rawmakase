//! What a photo without an edit starts from: the raw defaults, as the
//! desktop's Preferences choose them, and read, ready for each photo.
use crate::{Context, Preset};
use rawmakase_interop::raw_defaults::{self as desktop, DevelopDefaults};
use std::sync::Arc;

/// What photos without an edit start from, for every camera or for each, as
/// the desktop's Preferences choose them. [`Context::raw_defaults`] reads the
/// presets they name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RawDefaults {
    /// The choice for every camera.
    pub master: DefaultChoice,
    /// Whether the cameras' own choices are used: off keeps them without
    /// using them, as Lightroom's "Override global setting for specific
    /// camera" does.
    pub camera_overrides: bool,
    /// Each camera's own choice.
    pub cameras: Vec<CameraDefault>,
}

/// What a photo starts from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum DefaultChoice {
    /// Lightroom's Adobe Default: Adobe Color, else Adobe Standard, else the
    /// DNG's own profile, else RAWmakase Color.
    #[default]
    Adobe,
    /// The same settings with RAWmakase Color wherever it fits the camera.
    Rawmakase,
    /// Lightroom's Camera Settings: the same settings with the camera
    /// maker's standard look, when its profile is installed, else Adobe
    /// Default.
    CameraSettings,
    /// A Develop preset applied over Adobe Default.
    Preset {
        /// The preset's [`Preset::id`].
        id: String,
        /// Its name, for showing the choice.
        name: String,
    },
}

impl DefaultChoice {
    /// Choosing `preset`.
    pub fn preset(preset: &Preset) -> Self {
        Self::Preset {
            id: preset.id().into(),
            name: preset.name().into(),
        }
    }

    /// The choice as the desktop names it: "Adobe Default", a preset's name.
    pub fn label(&self) -> String {
        self.desktop().label()
    }

    fn desktop(&self) -> desktop::DefaultChoice {
        match self {
            Self::Adobe => desktop::DefaultChoice::Adobe,
            Self::Rawmakase => desktop::DefaultChoice::Rawmakase,
            Self::CameraSettings => desktop::DefaultChoice::CameraSettings,
            Self::Preset { id, name } => desktop::DefaultChoice::Preset {
                id: id.clone(),
                name: name.clone(),
            },
        }
    }
}

/// A camera's own choice.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CameraDefault {
    /// The camera as Lightroom names it: its maker and model, "Sony
    /// ILCE-7CR", or the model alone.
    pub camera: String,
    /// What its photos start from.
    pub choice: DefaultChoice,
}

impl RawDefaults {
    fn desktop(&self) -> desktop::RawDefaults {
        desktop::RawDefaults {
            master: self.master.desktop(),
            camera_overrides: self.camera_overrides,
            cameras: self
                .cameras
                .iter()
                .map(|c| desktop::CameraDefault {
                    camera: c.camera.clone(),
                    choice: c.choice.desktop(),
                })
                .collect(),
        }
    }
}

/// Raw defaults, read: the choices with the presets they name, ready for
/// [`Photo::starting_recipe`](crate::Photo::starting_recipe). Made by
/// [`Context::raw_defaults`]. Cheap to clone, and `Send` and `Sync`.
///
/// The presets are read once, as these are made: a preset changed or
/// removed afterwards is seen by the next ones made, as the desktop reads
/// them again when its Preferences change.
#[derive(Clone)]
pub struct Defaults(pub(crate) Arc<DevelopDefaults>);

impl Defaults {
    /// The choices these were made from.
    pub fn choices(&self) -> RawDefaults {
        let settings = self.0.settings();
        let choice = |c: &desktop::DefaultChoice| match c {
            desktop::DefaultChoice::Adobe => DefaultChoice::Adobe,
            desktop::DefaultChoice::Rawmakase => DefaultChoice::Rawmakase,
            desktop::DefaultChoice::CameraSettings => DefaultChoice::CameraSettings,
            desktop::DefaultChoice::Preset { id, name } => DefaultChoice::Preset {
                id: id.clone(),
                name: name.clone(),
            },
        };
        RawDefaults {
            master: choice(&settings.master),
            camera_overrides: settings.camera_overrides,
            cameras: settings
                .cameras
                .iter()
                .map(|c| CameraDefault {
                    camera: c.camera.clone(),
                    choice: choice(&c.choice),
                })
                .collect(),
        }
    }
}

impl std::fmt::Debug for Defaults {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Defaults").field(&self.choices()).finish()
    }
}

impl Context {
    /// `defaults`, read: the presets they name are read now, from the file
    /// each preset's id names (or RAWmakase's own, for a built-in one's). A
    /// preset that is gone gives Adobe Default, as on the desktop. An app
    /// whose preferences change makes new ones; what was started from the
    /// old ones keeps them.
    pub fn raw_defaults(&self, defaults: &RawDefaults) -> Defaults {
        Defaults(Arc::new(
            DevelopDefaults::load(defaults.desktop()).with_lensfun(self.lensfun().clone()),
        ))
    }
}
