//! Raw defaults, as in Lightroom Classic's Preferences > Presets > Raw Defaults: the
//! settings a photo without an edit starts from, and what Reset returns to. A
//! master choice applies to every camera unless a camera has its own.
//!
//! Only photos without a RAWmakase or Lightroom edit follow these. Lightroom edits
//! are stored relative to Adobe's defaults, so they always convert from Adobe
//! Default, and a saved edit stays as it is until Reset.
use crate::model::recipe::{
    ProfilePreference, Recipe, camera_matching_names, camera_matching_profile,
};
use crate::{camera_data::Metadata, camera_profiles::CameraProfile, xmp::Preset};
use serde::{Deserialize, Deserializer, Serialize};
use std::sync::Arc;

/// The raw defaults as chosen in Preferences and kept in the session.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RawDefaults {
    #[serde(deserialize_with = "lenient")]
    pub master: DefaultChoice,
    /// "Override global setting for specific camera". Off keeps the cameras' own
    /// choices without using them, as Lightroom does.
    pub camera_overrides: bool,
    #[serde(deserialize_with = "lenient_list")]
    pub cameras: Vec<CameraDefault>,
}
/// What a photo starts from.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DefaultChoice {
    /// Lightroom's Adobe Default: Adobe Color, else Adobe Standard, else the
    /// DNG's own profile, else RAWmakase Color.
    #[default]
    Adobe,
    /// The same settings with RAWmakase Color wherever it fits the camera.
    Rawmakase,
    /// Lightroom's Camera Settings: the same settings with the imported
    /// camera-matching profile for the camera's standard look (Camera Standard,
    /// say), else Adobe Default.
    CameraSettings,
    /// A Develop preset applied over Adobe Default.
    Preset {
        /// `Preset::id`: `"builtin:<UUID>"` or the installed file's path.
        id: String,
        /// Shown while the preset cannot be found.
        name: String,
    },
}
impl DefaultChoice {
    pub fn label(&self) -> String {
        match self {
            DefaultChoice::Adobe => "Adobe Default".into(),
            DefaultChoice::Rawmakase => "RAWmakase Default".into(),
            DefaultChoice::CameraSettings => "Camera Settings".into(),
            DefaultChoice::Preset { name, .. } => crate::presets::display_name(name),
        }
    }
    pub fn preset(preset: &Preset) -> Self {
        DefaultChoice::Preset {
            id: preset.id.clone(),
            name: preset.name.clone(),
        }
    }
}
/// A camera's own choice.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CameraDefault {
    /// As Lightroom names the camera (see [`camera_name`]).
    pub camera: String,
    pub choice: DefaultChoice,
}
impl RawDefaults {
    /// The camera's own choice, when overrides are on, else the master.
    pub fn choice_for(&self, m: &Metadata) -> &DefaultChoice {
        self.camera_overrides
            .then(|| self.cameras.iter().find(|c| same_camera(&c.camera, m)))
            .flatten()
            .map_or(&self.master, |c| &c.choice)
    }
    /// Sets `camera`'s own choice, replacing any it had under this name or
    /// another for the same camera ("ILCE-7CR" and "Sony ILCE-7CR").
    pub fn set_camera(&mut self, camera: &str, choice: DefaultChoice) {
        match self.camera(camera) {
            Some(c) => {
                c.camera = camera.into();
                c.choice = choice;
            }
            None => {
                self.cameras.push(CameraDefault {
                    camera: camera.into(),
                    choice,
                });
                self.cameras.sort_by(|a, b| a.camera.cmp(&b.camera));
            }
        }
    }
    fn camera(&mut self, camera: &str) -> Option<&mut CameraDefault> {
        self.cameras
            .iter_mut()
            .find(|c| same_name(&c.camera, camera))
    }
    /// `camera`'s own choice, under this name or another for the same camera.
    pub fn camera_choice(&self, camera: &str) -> Option<&DefaultChoice> {
        self.cameras
            .iter()
            .find(|c| same_name(&c.camera, camera))
            .map(|c| &c.choice)
    }
    /// Follows a preset that moved to `id` (renamed), wherever it is chosen.
    pub fn rename_preset(&mut self, old: &str, id: &str, name: &str) {
        let choices =
            std::iter::once(&mut self.master).chain(self.cameras.iter_mut().map(|c| &mut c.choice));
        for choice in choices {
            if matches!(choice, DefaultChoice::Preset { id: was, .. } if was == old) {
                *choice = DefaultChoice::Preset {
                    id: id.into(),
                    name: name.into(),
                };
            }
        }
    }
    fn choices(&self) -> impl Iterator<Item = &DefaultChoice> {
        std::iter::once(&self.master).chain(self.cameras.iter().map(|c| &c.choice))
    }
}

/// A camera as Lightroom names it: the model, after the make unless the model
/// already names it ("Canon EOS R5", "Fujifilm X100F").
pub fn camera_name(m: &Metadata) -> String {
    let (make, model) = (m.make.trim(), m.model.trim());
    let brand = make.split(' ').next().unwrap_or_default();
    if model.is_empty() {
        make.into()
    } else if make.is_empty() || model.to_lowercase().starts_with(&brand.to_lowercase()) {
        model.into()
    } else {
        format!("{make} {model}")
    }
}
/// Camera makers as they lead a camera's name, lower case. Only these are taken
/// off a name, so "EOS M10" (Canon) stays apart from "M10" (Leica).
const MAKES: &[&str] = &[
    "apple",
    "canon",
    "dji",
    "fujifilm",
    "google",
    "gopro",
    "hasselblad",
    "huawei",
    "kodak",
    "konica minolta",
    "leaf",
    "leica",
    "mamiya",
    "minolta",
    "nikon",
    "olympus",
    "om digital solutions",
    "panasonic",
    "pentax",
    "phase one",
    "ricoh",
    "samsung",
    "sigma",
    "sony",
    "xiaomi",
];
/// Whether two camera names are the same camera: equal, or one the other after
/// its maker ("ILCE-7CR", "Sony ILCE-7CR").
pub fn same_name(a: &str, b: &str) -> bool {
    let (a, b) = (a.trim().to_lowercase(), b.trim().to_lowercase());
    let (short, long) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    !short.is_empty()
        && (short == long
            || long
                .strip_suffix(short.as_str())
                .and_then(|make| make.strip_suffix(' '))
                .is_some_and(|make| MAKES.contains(&make.trim())))
}
/// Whether `name`, from this catalog or Lightroom's, is the photo's camera. Lightroom
/// names some cameras by model alone ("ILCE-7CR"), others with the make.
pub fn same_camera(name: &str, m: &Metadata) -> bool {
    let name = name.trim();
    !name.is_empty()
        && [
            camera_name(m),
            m.model.trim().to_string(),
            format!("{} {}", m.make.trim(), m.model.trim()),
        ]
        .iter()
        .any(|c| c.eq_ignore_ascii_case(name))
}

/// The raw defaults ready to apply: the choices, with the presets they name read.
/// Cheap to share between threads behind an `Arc`. Equal when the choices and the
/// presets as read are: a preset file can change under the same id.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DevelopDefaults {
    settings: RawDefaults,
    presets: Vec<Preset>,
}
/// A photo's starting settings, which choice they came from, and why they are
/// Adobe Default instead of the preset asked for, if they are.
#[derive(Clone, Debug)]
pub struct Resolved {
    pub recipe: Recipe,
    pub name: String,
    pub note: Option<String>,
}
impl DevelopDefaults {
    /// Reads the presets `settings` names from the preset library.
    pub fn load(settings: RawDefaults) -> Self {
        Self::with_presets(settings, crate::presets::find_preset)
    }
    /// As [`load`](Self::load), finding presets with `find`.
    pub fn with_presets(settings: RawDefaults, find: impl Fn(&str) -> Option<Preset>) -> Self {
        let mut presets: Vec<Preset> = Vec::new();
        for choice in settings.choices() {
            if let DefaultChoice::Preset { id, .. } = choice
                && !presets.iter().any(|p| p.id == *id)
                && let Some(p) = find(id)
            {
                presets.push(p);
            }
        }
        Self { settings, presets }
    }
    pub fn settings(&self) -> &RawDefaults {
        &self.settings
    }
    /// What a photo from this camera starts from. Never fails: a preset that is
    /// missing or does not apply, or a camera-matching profile that isn't
    /// imported, leaves Adobe Default, with a note saying so.
    /// Raw Defaults are for raw files: a JPEG, PNG or TIFF starts from
    /// [`Recipe::for_new_rendered`] whatever they say.
    pub fn resolve(&self, m: &Metadata, profiles: &[Arc<CameraProfile>]) -> Resolved {
        if m.is_rendered() {
            return Resolved {
                recipe: Recipe::for_new_rendered(m),
                name: DefaultChoice::Adobe.label(),
                note: None,
            };
        }
        let choice = self.settings.choice_for(m);
        let adobe = || Recipe::with_profiles(m, profiles);
        let fallback = |note: String| Resolved {
            recipe: adobe(),
            name: DefaultChoice::Adobe.label(),
            note: Some(note),
        };
        match choice {
            DefaultChoice::Adobe => Resolved {
                recipe: adobe(),
                name: choice.label(),
                note: None,
            },
            DefaultChoice::Rawmakase => Resolved {
                recipe: Recipe::with_profile_preference(m, profiles, ProfilePreference::Rawmakase),
                name: choice.label(),
                note: None,
            },
            DefaultChoice::CameraSettings => {
                if camera_matching_profile(m, profiles).is_none() {
                    return fallback(format!(
                        "Camera Settings: no {} profile for {} is imported; using Adobe Default",
                        camera_matching_names(m).join(" or "),
                        camera_name(m)
                    ));
                }
                Resolved {
                    recipe: Recipe::with_profile_preference(m, profiles, ProfilePreference::Camera),
                    name: choice.label(),
                    note: None,
                }
            }
            DefaultChoice::Preset { id, .. } => {
                let Some(preset) = self.presets.iter().find(|p| p.id == *id) else {
                    return fallback(format!(
                        "Raw default preset ‘{}’ is missing; using Adobe Default",
                        choice.label()
                    ));
                };
                // No photo: Auto settings in a default are not measured, so the
                // photo opens, resets and previews the same everywhere.
                match preset.apply(&adobe(), m, profiles, None) {
                    Ok(mut recipe) => {
                        // Upright is worked out for one photo: analysed on each
                        // (which every view would have to wait for) or stored from
                        // the preset's own. A default leaves it off.
                        recipe.upright = Default::default();
                        Resolved {
                            recipe,
                            name: choice.label(),
                            note: None,
                        }
                    }
                    Err(e) => fallback(format!(
                        "Raw default ‘{}’ doesn't apply to this photo ({e:#}); using Adobe Default",
                        choice.label()
                    )),
                }
            }
        }
    }
}

/// A value that no longer reads (from a newer release, say) is the default,
/// rather than losing the whole session.
fn lenient<'de, D: Deserializer<'de>, T: serde::de::DeserializeOwned + Default>(
    d: D,
) -> Result<T, D::Error> {
    let value = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value(value).unwrap_or_default())
}
/// Entries that no longer read are left out; the rest are kept.
fn lenient_list<'de, D: Deserializer<'de>, T: serde::de::DeserializeOwned>(
    d: D,
) -> Result<Vec<T>, D::Error> {
    let value = serde_json::Value::deserialize(d)?;
    let serde_json::Value::Array(items) = value else {
        return Ok(Vec::new());
    };
    Ok(items
        .into_iter()
        .filter_map(|v| serde_json::from_value(v).ok())
        .collect())
}
/// For the session: raw defaults that no longer read are the out-of-the-box ones.
pub fn lenient_settings<'de, D: Deserializer<'de>>(d: D) -> Result<RawDefaults, D::Error> {
    lenient(d)
}

/// A synthetic preset that raises Exposure by 0.7, with this id.
#[cfg(any(test, feature = "test-support"))]
fn brighter_preset(id: &str) -> Preset {
    let text = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" crs:PresetType="Normal" crs:Version="15.0" crs:Exposure2012="+0.70"><crs:Name><rdf:Alt><rdf:li xml:lang="x-default">Brighter</rdf:li></rdf:Alt></crs:Name></rdf:Description></rdf:RDF></x:xmpmeta>"#;
    let mut p = crate::xmp::parse(std::path::Path::new(id), text).unwrap();
    p.id = id.into();
    p
}
/// Raw defaults whose master is that preset.
#[cfg(any(test, feature = "test-support"))]
pub fn brighter_defaults() -> DevelopDefaults {
    DevelopDefaults::with_presets(
        RawDefaults {
            master: DefaultChoice::Preset {
                id: "brighter".into(),
                name: "Brighter".into(),
            },
            ..Default::default()
        },
        |id| (id == "brighter").then(|| brighter_preset(id)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera_profiles::open;

    #[allow(clippy::approx_constant)] // Exact camera matrix coefficients, not mathematical constants.
    fn x100f() -> Metadata {
        Metadata {
            make: "Fujifilm".into(),
            model: "X100F".into(),
            wb: [2.0198677, 1., 1.8874172],
            cam_xyz: [
                [1.1434, -0.4948, -0.121],
                [-0.3746, 1.2042, 0.1903],
                [-0.0666, 0.1479, 0.5235],
            ],
            ..Default::default()
        }
    }
    fn other_camera() -> Metadata {
        Metadata {
            make: "Canon".into(),
            model: "EOS R5".into(),
            ..x100f()
        }
    }
    /// RAWmakase's own profiles, and Adobe Color made from one of them.
    fn profiles(m: &Metadata) -> Vec<Arc<CameraProfile>> {
        let mut adobe = open::standard(m).unwrap();
        adobe.name = "Adobe Color".into();
        [open::standard(m), open::color(m), Some(adobe)]
            .into_iter()
            .flatten()
            .map(Arc::new)
            .collect()
    }
    fn preset_choice(id: &str) -> DefaultChoice {
        DefaultChoice::Preset {
            id: id.into(),
            name: "Brighter".into(),
        }
    }
    fn load(settings: RawDefaults) -> DevelopDefaults {
        DevelopDefaults::with_presets(settings, |id| {
            (id == "brighter").then(|| brighter_preset(id))
        })
    }

    #[test]
    fn defaults_are_equal_only_when_their_presets_read_the_same() {
        let settings = RawDefaults {
            master: preset_choice("brighter"),
            ..RawDefaults::default()
        };
        assert_eq!(load(settings.clone()), load(settings.clone()));
        assert_ne!(load(settings.clone()), DevelopDefaults::default());
        // The same id, the file changed since.
        let edited = DevelopDefaults::with_presets(settings, |id| {
            let mut preset = brighter_preset(id);
            preset
                .settings
                .insert("Exposure2012".into(), "+0.50".into());
            Some(preset)
        });
        assert_ne!(load(edited.settings.clone()), edited);
    }

    #[test]
    fn out_of_the_box_defaults_are_adobe_default() {
        let m = x100f();
        let profiles = profiles(&m);
        let r = DevelopDefaults::default().resolve(&m, &profiles);
        assert_eq!(r.recipe, Recipe::with_profiles(&m, &profiles));
        assert_eq!(r.recipe.profile.unwrap().name, "Adobe Color");
        assert_eq!(r.name, "Adobe Default");
        assert!(r.note.is_none());
    }

    #[test]
    fn rawmakase_default_uses_rawmakase_color_even_with_adobe_profiles() {
        let m = x100f();
        let profiles = profiles(&m);
        let defaults = load(RawDefaults {
            master: DefaultChoice::Rawmakase,
            ..Default::default()
        });
        let r = defaults.resolve(&m, &profiles);
        assert_eq!(r.recipe.profile.as_ref().unwrap().name, open::COLOR);
        // Only the profile differs from Adobe Default.
        let adobe = Recipe::with_profiles(&m, &profiles);
        assert_eq!(
            Recipe {
                profile: adobe.profile.clone(),
                ..r.recipe
            },
            adobe
        );
        // Without RAWmakase Color for this camera, as Adobe Default.
        let only_adobe: Vec<_> = profiles
            .iter()
            .filter(|p| p.name != open::COLOR)
            .cloned()
            .collect();
        let r = defaults.resolve(&m, &only_adobe);
        assert_eq!(r.recipe.profile.unwrap().name, "Adobe Color");
    }

    #[test]
    fn raw_defaults_leave_a_rendered_file_as_it_is() {
        let rendered = Metadata {
            source: crate::camera_data::Source::Rendered,
            ..x100f()
        };
        let profiles = profiles(&rendered);
        let mut per_camera = RawDefaults {
            master: DefaultChoice::Adobe,
            camera_overrides: true,
            ..Default::default()
        };
        per_camera.set_camera("Fujifilm X100F", preset_choice("brighter"));
        for settings in [
            RawDefaults {
                master: preset_choice("brighter"),
                ..Default::default()
            },
            RawDefaults {
                master: DefaultChoice::CameraSettings,
                ..Default::default()
            },
            per_camera,
        ] {
            let r = load(settings).resolve(&rendered, &profiles);
            assert_eq!(r.recipe, Recipe::for_new_rendered(&rendered));
            assert_eq!(r.name, DefaultChoice::Adobe.label());
            assert!(r.note.is_none());
        }
    }

    #[test]
    fn a_camera_override_beats_the_master_while_overrides_are_on() {
        let (m, other) = (x100f(), other_camera());
        let mut settings = RawDefaults {
            master: DefaultChoice::Rawmakase,
            camera_overrides: true,
            ..Default::default()
        };
        settings.set_camera("Fujifilm X100F", preset_choice("brighter"));
        let defaults = load(settings.clone());
        let r = defaults.resolve(&m, &profiles(&m));
        assert_eq!(r.name, "Brighter");
        assert_eq!(r.recipe.exposure, 0.7);
        assert_eq!(r.recipe.profile.unwrap().name, "Adobe Color");
        // Another camera has the master.
        let r = defaults.resolve(&other, &profiles(&other));
        assert_eq!(r.name, "RAWmakase Default");
        assert_eq!(r.recipe.exposure, 0.);
        // Overrides off: the master for every camera, the camera's choice kept.
        settings.camera_overrides = false;
        let defaults = load(settings);
        assert_eq!(
            defaults.resolve(&m, &profiles(&m)).name,
            "RAWmakase Default"
        );
        assert_eq!(defaults.settings().cameras.len(), 1);
    }

    #[test]
    fn a_missing_or_unusable_preset_falls_back_to_adobe_default_with_a_note() {
        let m = x100f();
        let profiles = profiles(&m);
        let adobe = Recipe::with_profiles(&m, &profiles);
        let defaults = load(RawDefaults {
            master: preset_choice("gone"),
            ..Default::default()
        });
        let r = defaults.resolve(&m, &profiles);
        assert_eq!(r.recipe, adobe);
        assert_eq!(r.name, "Adobe Default");
        assert!(r.note.unwrap().contains("‘Brighter’ is missing"));
        // A preset for another camera only.
        let mut restricted = brighter_preset("brighter");
        restricted
            .blockers
            .push("This preset is for another camera".into());
        let defaults = DevelopDefaults::with_presets(
            RawDefaults {
                master: preset_choice("brighter"),
                ..Default::default()
            },
            |_| Some(restricted.clone()),
        );
        let r = defaults.resolve(&m, &profiles);
        assert_eq!(r.recipe, adobe);
        assert!(r.note.unwrap().contains("another camera"));
    }

    /// `profiles` plus a camera-matching profile by this name.
    fn with_camera_profile(m: &Metadata, name: &str) -> Vec<Arc<CameraProfile>> {
        let mut camera = open::standard(m).unwrap();
        camera.name = name.into();
        let mut all = profiles(m);
        all.push(Arc::new(camera));
        all
    }
    fn sony() -> Metadata {
        Metadata {
            make: "Sony".into(),
            model: "ILCE-7M2".into(),
            ..x100f()
        }
    }

    #[test]
    fn camera_settings_start_from_the_camera_matching_profile() {
        let defaults = load(RawDefaults {
            master: DefaultChoice::CameraSettings,
            ..Default::default()
        });
        let m = sony();
        let profiles = with_camera_profile(&m, "Camera Standard");
        let r = defaults.resolve(&m, &profiles);
        assert_eq!(r.name, "Camera Settings");
        assert!(r.note.is_none());
        assert_eq!(r.recipe.profile.as_ref().unwrap().name, "Camera Standard");
        // Only the profile differs from Adobe Default.
        let adobe = Recipe::with_profiles(&m, &profiles);
        assert_eq!(
            Recipe {
                profile: adobe.profile.clone(),
                ..r.recipe
            },
            adobe
        );
        // Fujifilm names its standard look after the film simulation.
        let m = x100f();
        let profiles = with_camera_profile(&m, "Camera PROVIA/Standard");
        let r = defaults.resolve(&m, &profiles);
        assert_eq!(r.recipe.profile.unwrap().name, "Camera PROVIA/Standard");
    }

    #[test]
    fn camera_settings_without_an_imported_camera_profile_are_adobe_default_with_a_note() {
        let defaults = load(RawDefaults {
            master: DefaultChoice::CameraSettings,
            ..Default::default()
        });
        let m = sony();
        let profiles = profiles(&m);
        let r = defaults.resolve(&m, &profiles);
        assert_eq!(r.recipe, Recipe::with_profiles(&m, &profiles));
        assert_eq!(r.name, "Adobe Default");
        let note = r.note.unwrap();
        assert!(note.contains("Camera Standard"), "{note}");
        assert!(note.contains("Sony ILCE-7M2"), "{note}");
        // Another camera's Camera Standard doesn't fit this one.
        let mut other = open::standard(&other_camera()).unwrap();
        other.name = "Camera Standard".into();
        other.camera = "Canon EOS R5".into();
        let mut with_other = profiles;
        with_other.push(Arc::new(other));
        assert_eq!(defaults.resolve(&m, &with_other).name, "Adobe Default");
        // Nothing imported at all: RAWmakase's own profile, as Adobe Default.
        let r = defaults.resolve(&m, &[]);
        assert_eq!(r.recipe, Recipe::with_profiles(&m, &[]));
        assert!(r.note.is_some());
    }

    #[test]
    fn camera_settings_can_be_one_cameras_own_default() {
        let mut settings = RawDefaults {
            camera_overrides: true,
            ..Default::default()
        };
        settings.set_camera("ILCE-7M2", DefaultChoice::CameraSettings);
        let defaults = load(settings);
        let m = sony();
        let r = defaults.resolve(&m, &with_camera_profile(&m, "Camera Standard"));
        assert_eq!(r.recipe.profile.unwrap().name, "Camera Standard");
        // Another camera keeps the master, Adobe Default.
        let other = other_camera();
        let r = defaults.resolve(&other, &with_camera_profile(&other, "Camera Standard"));
        assert_eq!(r.name, "Adobe Default");
        assert_eq!(r.recipe.profile.unwrap().name, "Adobe Color");
    }

    #[test]
    fn a_camera_has_one_default_whichever_name_it_was_set_under() {
        let mut settings = RawDefaults::default();
        settings.set_camera("ILCE-7CR", DefaultChoice::Rawmakase);
        settings.set_camera("Sony ILCE-7CR", preset_choice("brighter"));
        assert_eq!(settings.cameras.len(), 1);
        assert_eq!(settings.cameras[0].camera, "Sony ILCE-7CR");
        assert_eq!(
            settings.camera_choice("ilce-7cr"),
            Some(&preset_choice("brighter"))
        );
        settings.set_camera("ILCE-7C", DefaultChoice::Rawmakase);
        assert_eq!(settings.cameras.len(), 2);
        // Another maker's model by the same name stays apart.
        settings.set_camera("M10", DefaultChoice::Rawmakase);
        settings.set_camera("EOS M10", DefaultChoice::Adobe);
        assert_eq!(settings.cameras.len(), 4);
        assert!(same_name("Canon EOS M10", "EOS M10"));
    }

    #[test]
    fn a_renamed_preset_stays_the_default() {
        let mut settings = RawDefaults {
            master: preset_choice("a.xmp"),
            ..Default::default()
        };
        settings.set_camera("Canon EOS R5", preset_choice("a.xmp"));
        settings.set_camera("Fujifilm X100F", preset_choice("other.xmp"));
        settings.rename_preset("a.xmp", "b.xmp", "Brightest");
        let renamed = DefaultChoice::Preset {
            id: "b.xmp".into(),
            name: "Brightest".into(),
        };
        assert_eq!(settings.master, renamed);
        assert_eq!(settings.camera_choice("Canon EOS R5"), Some(&renamed));
        assert_eq!(
            settings.camera_choice("Fujifilm X100F"),
            Some(&preset_choice("other.xmp"))
        );
    }

    #[test]
    fn a_default_preset_leaves_upright_off() {
        let m = x100f();
        let profiles = profiles(&m);
        let mut upright = brighter_preset("upright");
        upright
            .settings
            .insert("PerspectiveUpright".into(), "1".into());
        // The preset itself sets Auto Upright, to be analysed on the photo.
        let applied = upright
            .apply(&Recipe::with_profiles(&m, &profiles), &m, &profiles, None)
            .unwrap();
        assert_ne!(
            applied.upright.mode,
            crate::model::transform::UprightMode::Off
        );
        let defaults = DevelopDefaults::with_presets(
            RawDefaults {
                master: preset_choice("upright"),
                ..Default::default()
            },
            |_| Some(upright.clone()),
        );
        let r = defaults.resolve(&m, &profiles);
        assert_eq!(r.recipe.upright, Default::default());
        assert_eq!(r.recipe.exposure, 0.7);
    }

    #[test]
    fn cameras_match_by_lightroom_name_or_model() {
        let m = x100f();
        assert_eq!(camera_name(&m), "Fujifilm X100F");
        assert_eq!(camera_name(&other_camera()), "Canon EOS R5");
        let sony = Metadata {
            make: "Sony".into(),
            model: "ILCE-7CR".into(),
            ..Default::default()
        };
        assert!(same_camera("ILCE-7CR", &sony));
        assert!(same_camera("sony ilce-7cr", &sony));
        assert!(same_camera("Canon EOS R5", &other_camera()));
        assert!(!same_camera("Canon EOS R6", &other_camera()));
        assert!(!same_camera("", &Metadata::default()));
    }

    #[test]
    fn settings_round_trip_and_read_old_or_newer_files() {
        let mut settings = RawDefaults {
            master: preset_choice("brighter"),
            camera_overrides: true,
            ..Default::default()
        };
        settings.set_camera("Canon EOS R5", DefaultChoice::Rawmakase);
        settings.set_camera("Sony ILCE-7M2", DefaultChoice::CameraSettings);
        let json = serde_json::to_string(&settings).unwrap();
        assert!(json.contains(r#"{"kind":"camera_settings"}"#), "{json}");
        assert_eq!(
            serde_json::from_str::<RawDefaults>(&json).unwrap(),
            settings
        );
        // Before raw defaults: none.
        assert_eq!(
            serde_json::from_str::<RawDefaults>("{}").unwrap(),
            RawDefaults::default()
        );
        // A choice from a newer release is Adobe Default; its camera entries that
        // don't read are left out, the rest kept.
        let newer = r#"{"master":{"kind":"auto_tone"},"camera_overrides":true,
            "cameras":[{"camera":"A","choice":{"kind":"hdr"}},
                       {"camera":"B","choice":{"kind":"rawmakase"}}],"later":1}"#;
        let read: RawDefaults = serde_json::from_str(newer).unwrap();
        assert_eq!(read.master, DefaultChoice::Adobe);
        assert_eq!(read.cameras.len(), 1);
        assert_eq!(read.cameras[0].camera, "B");
    }
}
