//! Lightroom's Profile Amount (a look's `Amount`, 0–200%). RAWmakase renders a look at
//! 100% or not at all, so other amounts render at the nearer of the two and are reported.
use anyhow::{Context, Result, ensure};

/// A look's Amount as Lightroom stores it: 1 is 100%.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LookAmount(f32);

/// Whether the look renders.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LookUse {
    Apply,
    Omit,
}

/// How a look at some Amount renders, and what to report when that is only near it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LookRendering {
    pub look: LookUse,
    pub warning: Option<String>,
}

impl LookAmount {
    pub fn parse(text: &str) -> Result<Self> {
        let amount: f32 = text
            .trim()
            .parse()
            .with_context(|| format!("Invalid Profile Amount {text}"))?;
        ensure!(
            (0. ..=2.).contains(&amount),
            "Profile Amount {text} is outside 0–200%"
        );
        Ok(Self(amount))
    }
    pub fn rendering(self) -> LookRendering {
        let percent = (self.0 * 100.).round();
        let (look, warning) = match self.0 {
            1. => (LookUse::Apply, None),
            0. => (LookUse::Omit, None),
            a if a < 0.5 => (
                LookUse::Omit,
                Some(format!(
                    "Profile Amount {percent}% is not supported yet; rendered without the look"
                )),
            ),
            _ => (
                LookUse::Apply,
                Some(format!(
                    "Profile Amount {percent}% is not supported yet; rendered at 100%"
                )),
            ),
        };
        LookRendering { look, warning }
    }
}
