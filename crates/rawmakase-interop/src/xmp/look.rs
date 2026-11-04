//! Lightroom's Profile Amount (a look's `Amount`, 0–200%), which looks that support it
//! render at (see `CameraProfile::look_amount`).
use anyhow::{Context, Result, ensure};

/// The setting a parsed preset carries a look's Amount in, beside its UUID.
pub(crate) const SETTING: &str = "RAWmakaseLookAmount";

/// A look's Amount as Lightroom stores it: 1 is 100%.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LookAmount(pub f32);

impl LookAmount {
    pub(crate) fn parse(text: &str) -> Result<Self> {
        let amount: f32 = text
            .trim()
            .trim_matches('"')
            .parse()
            .with_context(|| format!("Invalid Profile Amount {text}"))?;
        ensure!(
            (0. ..=2.).contains(&amount),
            "Profile Amount {text} is outside 0–200%"
        );
        Ok(Self(amount))
    }
}
