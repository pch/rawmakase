//! Changing a recipe through the model's rules: a slider as a parameter, by
//! its id, so that what depends on it follows; what a front end needs to draw
//! the slider; presets, and Auto.
use crate::{Error, Image, Photo, Recipe, Result};
use rawmakase_cancel::Cancel;
use rawmakase_engine::develop::black_white::TreatmentChoice;
use rawmakase_engine::develop::{self, Measures};
use rawmakase_interop::{presets, xmp};
use rawmakase_model::model::edit::change_setting;
use rawmakase_model::model::params::ParameterId;

pub use rawmakase_model::model::params::{Control, Scale};

macro_rules! library_parameters {
    ($($id:ident $name:literal => { $($field:tt)+ })*) => {
        /// A develop setting a slider moves, named by [`Parameter::name`]
        /// (`exposure`, `temperature`) and found by [`Parameter::named`]:
        /// the model's list of them, in the desktop's order.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        #[non_exhaustive]
        pub enum Parameter {
            $(
                #[doc = concat!("The `", $name, "` setting.")]
                $id,
            )*
        }

        impl Parameter {
            /// Every setting, in the desktop's order.
            pub const ALL: [Self; [$(Self::$id),*].len()] = [$(Self::$id),*];

            pub(crate) const fn id(self) -> ParameterId {
                match self {
                    $(Self::$id => ParameterId::$id,)*
                }
            }

            const fn of(id: ParameterId) -> Self {
                match id {
                    $(ParameterId::$id => Self::$id,)*
                }
            }
        }
    };
}

rawmakase_model::with_parameters!(library_parameters);

impl Parameter {
    /// The setting's name, as RAWmakase's MIDI mappings, control socket and
    /// command line spell it: `exposure`, `sharpening_amount`.
    pub const fn name(self) -> &'static str {
        self.id().name()
    }

    /// The setting named `name`, if there is one.
    pub fn named(name: &str) -> Option<Self> {
        ParameterId::named(name).map(Self::of)
    }
}

/// A Develop preset: an XMP preset, Lightroom's or Camera Raw's, or one of
/// RAWmakase's own. Applied with [`Photo::apply_preset`].
#[derive(Clone, Debug, PartialEq)]
pub struct Preset(pub(crate) xmp::Preset);

impl Preset {
    /// What a preset is known by: `builtin:<UUID>` for one of RAWmakase's,
    /// else its file's path.
    pub fn id(&self) -> &str {
        &self.0.id
    }

    /// Its name.
    pub fn name(&self) -> &str {
        &self.0.name
    }

    /// The group it is listed in.
    pub fn group(&self) -> &str {
        &self.0.group
    }

    /// Whether it is one of RAWmakase's own, which come with it.
    pub fn is_builtin(&self) -> bool {
        self.0.builtin
    }

    /// Whether applying it measures the photo (Auto Settings, Auto white
    /// balance, an automatic B&W mix), so it needs the photo's decode.
    pub fn measures_photo(&self) -> bool {
        self.0.measures_photo()
    }
}

/// A preset applied to a photo's settings.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Applied {
    /// The settings, with the preset applied.
    pub recipe: Recipe,
    /// What of the preset did not fit this photo and was left as it was, one
    /// line each.
    pub skipped: Vec<String>,
    /// The profile used in place of one the preset names that this camera
    /// lacks, by name.
    pub substitute: Option<String>,
}

impl Photo {
    /// Sets `parameter` of `recipe` to `value`, in the recipe's units, and
    /// brings along what the change implies: a new Temp or Tint gives the white
    /// balance the renderer multiplies by, for this photo.
    ///
    /// On a rendered file (a JPEG, say) Temp and Tint are relative sliders, and
    /// `Parameter::Temperature` and `Parameter::Tint` move those.
    ///
    /// [`ErrorKind::Invalid`](crate::ErrorKind::Invalid) for a value a recipe
    /// may not hold; the recipe is then unchanged.
    pub fn set(&self, recipe: &mut Recipe, parameter: Parameter, value: f32) -> Result<()> {
        let id = parameter.id().for_photo(Some(self.facts()));
        let d = id.descriptor();
        if !value.is_finite() || !d.valid.contains(&value) {
            return Err(Error::of(
                rawmakase_model::error::Invalid::new(format!(
                    "{} must be between {} and {}",
                    d.label,
                    id.text(*d.valid.start()),
                    id.text(*d.valid.end()),
                ))
                .into(),
            ));
        }
        change_setting(&mut recipe.0, id, Some(self.facts()), |r| {
            *id.value_mut(r) = value;
        });
        Ok(())
    }

    /// Chooses `profile`, one of [`Photo::profiles`], as the desktop's
    /// Profile menu does: a newly chosen profile starts at 100%, as in
    /// Lightroom, the camera baseline and white balance controls become the
    /// new profile's, and the treatment follows it: a monochrome profile
    /// turns Black & White on, and a colour one after it turns it off.
    /// Choosing the profile the recipe has changes nothing.
    ///
    /// [`ErrorKind::Invalid`](crate::ErrorKind::Invalid) for a profile that
    /// isn't this photo's.
    pub fn set_profile(&self, recipe: &mut Recipe, profile: &crate::Profile) -> Result<()> {
        if !self
            .camera_profiles()
            .iter()
            .any(|p| std::sync::Arc::ptr_eq(p, &profile.0))
        {
            return Err(Error::of(
                rawmakase_model::error::Invalid::new(format!(
                    "{} is not one of this photo's profiles",
                    profile.name()
                ))
                .into(),
            ));
        }
        let r = &mut recipe.0;
        if r.profile.as_ref() != Some(&profile.0) {
            let old = r.profile.replace(profile.0.clone());
            r.profile_amount = 1.;
            r.profile_changed(self.facts());
            // Black & White follows a monochrome profile, and Color a colour
            // one after it. Without the decoded photo there is no Auto mix to
            // measure for a first conversion, as when the desktop fits a
            // Camera Look.
            r.follow_profile_treatment(old.as_deref(), None);
        }
        Ok(())
    }

    /// `parameter`'s value in `recipe`, in the recipe's units, for the setting
    /// it moves on this photo.
    pub fn value(&self, recipe: &Recipe, parameter: Parameter) -> f32 {
        parameter
            .id()
            .for_photo(Some(self.facts()))
            .value(&recipe.0)
    }

    /// `parameter`'s slider for this photo, with its value in `recipe`.
    pub fn control(&self, recipe: &Recipe, parameter: Parameter) -> Control {
        let mut control = parameter
            .id()
            .for_photo(Some(self.facts()))
            .control(&recipe.0);
        // Named as asked: a rendered file's relative Temp is still Temp's.
        control.id = parameter.name();
        control
    }

    /// `preset` applied at 100% to `recipe`, as Develop applies one: what
    /// fits this photo, the rest reported. Its Auto settings (Auto Tone, Auto
    /// White Balance, an automatic B&W mix) are measured on `image`, this
    /// photo's decode; without it they leave the settings as they are.
    pub fn apply_preset(
        &self,
        recipe: &Recipe,
        preset: &Preset,
        image: Option<&Image>,
    ) -> Result<Applied> {
        let measures = image.map(|i| Measures(i.camera()));
        let applied = presets::apply_to_photo(
            &preset.0,
            &recipe.0,
            self.facts(),
            self.camera_profiles(),
            measures
                .as_ref()
                .map(|m| m as &dyn rawmakase_interop::xmp::PhotoMeasures),
            self.context().lensfun(),
        )
        .map_err(Error::of)?;
        Ok(Applied {
            recipe: Recipe(applied.recipe),
            skipped: applied.skipped,
            substitute: applied.substitute,
        })
    }

    /// Saves `recipe`, this photo's edit, as a RAWmakase preset at `path`,
    /// which [`Context::load_recipe`](crate::Context::load_recipe) and
    /// `rawmakase render --recipe` read.
    ///
    /// A preset is not the whole edit: spot removal stays with its photo, and
    /// a mask made from a selection is refused.
    /// [`ErrorKind::Invalid`](crate::ErrorKind::Invalid) for a recipe that does
    /// not validate.
    pub fn save_recipe(&self, recipe: &Recipe, path: impl AsRef<std::path::Path>) -> Result<()> {
        recipe.validate()?;
        presets::save_preset(path.as_ref(), &recipe.0, self.facts()).map_err(Error::of)
    }

    /// `recipe` with Exposure, Contrast, Highlights, Shadows, Whites, Blacks
    /// and Vibrance estimated from `image`, this photo's decode, as the Basic
    /// panel's Auto applies them; white balance and the rest are kept.
    pub fn auto_tone(&self, recipe: &Recipe, image: &Image, cancel: Cancel<'_>) -> Result<Recipe> {
        develop::auto_tone(image.camera(), &recipe.0, cancel)
            .map(Recipe)
            .map_err(Error::of)
    }

    /// `recipe` with the white balance that renders `image`'s near-neutral
    /// areas neutral, as the As Shot menu's Auto.
    pub fn auto_white_balance(
        &self,
        recipe: &Recipe,
        image: &Image,
        cancel: Cancel<'_>,
    ) -> Result<Recipe> {
        develop::auto_white_balance(image.camera(), &recipe.0, cancel)
            .map(Recipe)
            .map_err(Error::of)
    }
}

impl crate::Context {
    /// The presets: RAWmakase's built-in ones and those installed in the
    /// context's data folders, in the order the desktop lists them, and the
    /// files that could not be read.
    pub fn presets(&self) -> (Vec<Preset>, Vec<String>) {
        let library = self.preset_library().load();
        (
            library.presets.into_iter().map(Preset).collect(),
            library.errors,
        )
    }

    /// Reads a recipe from the file [`Photo::save_recipe`] writes: a
    /// RAWmakase preset, as `rawmakase render --recipe` takes.
    ///
    /// [`ErrorKind::Missing`](crate::ErrorKind::Missing) when there is no
    /// file.
    pub fn load_recipe(&self, path: impl AsRef<std::path::Path>) -> Result<Recipe> {
        presets::load_preset(path.as_ref())
            .map(|preset| Recipe(preset.recipe))
            .map_err(Error::of)
    }

    /// Reads the preset at `path`: an XMP preset, Lightroom's or Camera
    /// Raw's, or RAWmakase's own.
    ///
    /// [`ErrorKind::Missing`](crate::ErrorKind::Missing) when there is no
    /// file.
    pub fn load_preset(&self, path: impl AsRef<std::path::Path>) -> Result<Preset> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path).map_err(|e| Error::of(e.into()))?;
        xmp::parse(path, &text).map(Preset).map_err(Error::of)
    }
}
