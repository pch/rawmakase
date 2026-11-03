//! AI Denoise, a Detail setting: whether the photo renders from RawNIND's denoised
//! demosaic and how much of it (docs/rendering-quality.md#ai-denoise). The result
//! itself is not part of the edit: every edit renders with the installed model.
use serde::{Deserialize, Serialize};

/// AI Denoise's switch and Amount (0–1, 0.5 by default as Lightroom's 50). Amount is kept
/// while the switch is off, so turning it back on restores it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AiDenoise {
    pub enabled: bool,
    pub amount: f32,
}
impl Default for AiDenoise {
    fn default() -> Self {
        Self {
            enabled: false,
            amount: 0.5,
        }
    }
}
impl AiDenoise {
    /// Off at the default Amount: not saved, so releases that predate it read the recipe.
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
    /// Whether rendering needs the AI result: on, at an Amount above 0.
    pub fn renders(&self) -> bool {
        self.enabled && self.amount > 0.
    }
    pub fn is_valid(&self) -> bool {
        self.amount.is_finite() && (0. ..=1.).contains(&self.amount)
    }
}
