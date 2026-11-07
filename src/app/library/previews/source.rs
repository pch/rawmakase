//! What an edited preview is rendered from, and the tag it is kept under.
use crate::edits::Start;
use crate::model::recipe::Recipe;
use rawmakase_cancel::Cancel;
use std::{path::Path, sync::Arc};

/// What an edited preview is rendered from.
#[derive(Clone)]
pub(in crate::app) enum EditSource {
    /// A saved RAWmakase recipe, as JSON.
    Recipe(String),
    /// Lightroom develop settings from an imported catalog.
    Lightroom(String),
    /// No edit: the raw defaults, as Develop would open the photo.
    Defaults(Arc<crate::raw_defaults::DevelopDefaults>),
}
impl EditSource {
    /// The recipe the photo at `path` is rendered with: its edit, or the defaults
    /// Develop would open it with (Adobe Default unless given), a fitted camera look
    /// included for a photo without colour data.
    pub(crate) fn recipe(
        edit: Option<&Self>,
        m: &crate::camera_data::Metadata,
        path: &Path,
        cancel: Cancel<'_>,
    ) -> anyhow::Result<Recipe> {
        let (profiles, _) = crate::camera_profiles::installed(m);
        let profiles = crate::camera_look::with_default(path, m, profiles, cancel);
        Self::recipe_with(edit, m, &profiles)
    }
    /// The recipe `edit` gives a photo with metadata `m` and camera `profiles`,
    /// as [`Self::recipe`] works it out once it has them.
    pub(crate) fn recipe_with(
        edit: Option<&Self>,
        m: &crate::camera_data::Metadata,
        profiles: &[std::sync::Arc<crate::camera_profiles::CameraProfile>],
    ) -> anyhow::Result<Recipe> {
        let start = match edit {
            Some(Self::Recipe(json)) => Start::RecipeJson(json.clone()),
            Some(Self::Lightroom(text)) => Start::Lightroom(text.clone()),
            Some(Self::Defaults(defaults)) => Start::Defaults(defaults.clone()),
            None => Start::AdobeDefault,
        };
        Ok(start.resolve(m, profiles)?.recipe)
    }
    /// Identifies this edit, developed by this build's RAW decoder
    /// (`raw::developed`), in the preview cache.
    pub(crate) fn tag(&self) -> String {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        match self {
            Self::Recipe(text) => ("recipe", text).hash(&mut h),
            Self::Lightroom(text) => ("lightroom", text).hash(&mut h),
            Self::Defaults(defaults) => ("defaults", format!("{defaults:?}")).hash(&mut h),
        }
        crate::raw::developed().hash(&mut h);
        format!("edit-{:016x}", h.finish())
    }
    /// Identifies this edit's Library preview in the preview cache: [`Self::tag`],
    /// how the preview is rendered (Develop's Fit path), so previews rendered
    /// another way are made again rather than shown beside new ones, and, for an
    /// edit with AI Denoise, whether it shows the installed model's result.
    pub(crate) fn preview_tag(&self, ai: PreviewAi) -> String {
        self.preview_tag_of(ai, &crate::ai_denoise::model_tag())
    }
    /// [`preview_tag`](Self::preview_tag) with AI Denoise's result from `model`.
    pub(super) fn preview_tag_of(&self, ai: PreviewAi, model: &str) -> String {
        let tag = self.tag();
        match ai {
            PreviewAi::Off => format!("{tag}-fit"),
            // Not "-fit": before AI Denoise was told apart, Develop kept its renders
            // with the result under the edit's plain tag. They are not shown again.
            PreviewAi::Plain => format!("{tag}-fit-noai"),
            PreviewAi::Finished => format!("{tag}-fit-ai-{model}"),
        }
    }
    /// Whether this edit renders AI Denoise, as far as the Library tells without
    /// opening the file: a RAWmakase edit with it on. A Lightroom edit or the
    /// defaults render without it here, as they did before.
    pub(in crate::app) fn renders_ai(&self) -> bool {
        matches!(self, Self::Recipe(json) if serde_json::from_str::<Recipe>(json)
            .is_ok_and(|r| r.as_rendered().ai_denoise.renders()))
    }
}
/// What a Library preview shows of AI Denoise (docs/rendering-quality.md#ai-denoise).
/// A preview made with a partial result is never kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum PreviewAi {
    /// The edit has no AI Denoise.
    Off,
    /// The edit has AI Denoise, rendered without a result: none was finished.
    Plain,
    /// The edit rendered with a finished result of the installed model.
    Finished,
}
