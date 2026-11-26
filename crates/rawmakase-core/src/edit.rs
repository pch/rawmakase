//! Changing a recipe through the model's rules: a slider as a parameter, by
//! its id, so that what depends on it follows; what a front end needs to draw
//! the slider; presets, and Auto.
use crate::{Error, Image, Photo, Result};
use rawmakase_cancel::Cancel;
use rawmakase_engine::develop::{self, Measures};
use rawmakase_interop::presets;
use rawmakase_model::model::edit::change_setting;
use rawmakase_model::model::recipe::Recipe;

pub use rawmakase_interop::xmp::Preset;
/// A develop setting a slider moves, by its id: the model's own, named by
/// [`Parameter::name`] (`exposure`, `temperature`) and found by
/// [`Parameter::named`].
pub use rawmakase_model::model::params::ParameterId as Parameter;
pub use rawmakase_model::model::params::{Control, Scale};

/// A preset applied to a photo's settings: the settings, what of the preset
/// did not fit this photo and was left as it was, and the profile used in
/// place of one the preset names that this camera lacks.
pub use rawmakase_interop::presets::Applied;

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
        let id = parameter.for_photo(Some(self.metadata()));
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
        change_setting(recipe, id, Some(self.metadata()), |r| {
            *id.value_mut(r) = value;
        });
        Ok(())
    }

    /// `parameter`'s value in `recipe`, in the recipe's units, for the setting
    /// it moves on this photo.
    pub fn value(&self, recipe: &Recipe, parameter: Parameter) -> f32 {
        parameter.for_photo(Some(self.metadata())).value(recipe)
    }

    /// `parameter`'s slider for this photo, with its value in `recipe`.
    pub fn control(&self, recipe: &Recipe, parameter: Parameter) -> Control {
        let mut control = parameter.for_photo(Some(self.metadata())).control(recipe);
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
        presets::apply_to_photo(
            preset,
            recipe,
            self.metadata(),
            self.profiles(),
            measures
                .as_ref()
                .map(|m| m as &dyn rawmakase_interop::xmp::PhotoMeasures),
            self.context().lensfun(),
        )
        .map_err(Error::of)
    }

    /// `recipe` with Exposure, Contrast, Highlights, Shadows, Whites, Blacks
    /// and Vibrance estimated from `image`, this photo's decode, as the Basic
    /// panel's Auto applies them; white balance and the rest are kept.
    pub fn auto_tone(&self, recipe: &Recipe, image: &Image, cancel: Cancel<'_>) -> Result<Recipe> {
        develop::auto_tone(image.camera(), recipe, cancel).map_err(Error::of)
    }

    /// `recipe` with the white balance that renders `image`'s near-neutral
    /// areas neutral, as the As Shot menu's Auto.
    pub fn auto_white_balance(
        &self,
        recipe: &Recipe,
        image: &Image,
        cancel: Cancel<'_>,
    ) -> Result<Recipe> {
        develop::auto_white_balance(image.camera(), recipe, cancel).map_err(Error::of)
    }
}

impl crate::Context {
    /// The presets: RAWmakase's built-in ones and those installed in the
    /// context's data folders, in the order the desktop lists them, and the
    /// files that could not be read.
    pub fn presets(&self) -> (Vec<Preset>, Vec<String>) {
        let library = self.preset_library().load();
        (library.presets, library.errors)
    }

    /// Reads the preset at `path`: an XMP preset, Lightroom's or Camera
    /// Raw's, or RAWmakase's own.
    ///
    /// [`ErrorKind::Missing`](crate::ErrorKind::Missing) when there is no
    /// file.
    pub fn load_preset(&self, path: impl AsRef<std::path::Path>) -> Result<Preset> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path).map_err(|e| Error::of(e.into()))?;
        rawmakase_interop::xmp::parse(path, &text).map_err(Error::of)
    }
}
