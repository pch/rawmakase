//! What an edited preview is rendered from, and the tag it is kept under.
use crate::edits::Start;
use crate::model::recipe::Recipe;
use rawmakase_cancel::Cancel;
use std::{path::Path, sync::Arc};

/// What an edited preview is rendered from.
#[derive(Clone)]
pub(in crate::app) enum EditSource {
    /// A saved RAWmakase recipe.
    Recipe(SavedRecipe),
    /// A saved RAWmakase recipe that can't be read, and why: its preview fails, as
    /// developing it would.
    Unreadable(String),
    /// Lightroom develop settings from an imported catalog.
    Lightroom(String),
    /// No edit: the raw defaults, as Develop would open the photo.
    Defaults(Arc<crate::raw_defaults::DevelopDefaults>),
}
/// A saved recipe and what identifies it: a hash of its text in the catalog's
/// stored form, which names its profile and calibration by content. The same edit
/// therefore has the same identity whether it was read from the catalog or comes
/// from Develop, whatever order a stored row's keys are in.
#[derive(Clone)]
pub(in crate::app) struct SavedRecipe {
    recipe: Arc<Recipe>,
    identity: u64,
}

impl EditSource {
    /// The source of a saved RAWmakase recipe.
    pub(in crate::app) fn saved(recipe: Recipe) -> Self {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        crate::model::stored_recipe::to_text(&recipe).hash(&mut h);
        Self::Recipe(SavedRecipe {
            recipe: Arc::new(recipe),
            identity: h.finish(),
        })
    }
    /// The saved recipe this source holds, for tests.
    #[cfg(test)]
    pub(in crate::app) fn saved_recipe(&self) -> Option<&Recipe> {
        match self {
            Self::Recipe(saved) => Some(&saved.recipe),
            _ => None,
        }
    }
    /// The source of the edit the catalog's previews render a photo with.
    pub(in crate::app) fn of(edit: crate::catalog::PreviewEdit) -> Option<Self> {
        use crate::catalog::PreviewEdit;
        match edit {
            PreviewEdit::Saved(Ok(recipe)) => Some(Self::saved(*recipe)),
            PreviewEdit::Saved(Err(why)) => Some(Self::Unreadable(why)),
            PreviewEdit::Lightroom(text) => Some(Self::Lightroom(text)),
            PreviewEdit::None => None,
        }
    }
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
        let start = match edit {
            Some(Self::Recipe(saved)) => Start::Recipe(saved.recipe.clone()),
            Some(Self::Unreadable(why)) => anyhow::bail!("{why}"),
            Some(Self::Lightroom(text)) => Start::Lightroom(text.clone()),
            Some(Self::Defaults(defaults)) => Start::Defaults(defaults.clone()),
            None => Start::AdobeDefault,
        };
        Ok(start.resolve(m, &profiles)?.recipe)
    }
    /// Identifies this edit, developed by this build's RAW decoder
    /// (`raw::developed`), in the preview cache.
    pub(crate) fn tag(&self) -> String {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        match self {
            Self::Recipe(saved) => ("stored recipe", saved.identity).hash(&mut h),
            Self::Unreadable(why) => ("unreadable", why).hash(&mut h),
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
        matches!(self, Self::Recipe(saved) if saved.recipe.as_rendered().ai_denoise.renders())
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
