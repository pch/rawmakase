//! What AI Denoise needs from decoding, with no model knowledge: the visible mosaic
//! packed as the model reads it and the result assembled back ([`denoise`]), and the
//! results' disk cache ([`cache`]).
pub mod cache;
pub mod denoise;

/// AI Denoise's failure as an export or a render reports it: "AI Denoise: "
/// and the whole chain, in one message. A run that was cancelled stays the
/// [`rawmakase_cancel::Cancelled`] it returned, so its caller still sees a
/// cancellation and not a failure.
pub fn failed(error: anyhow::Error) -> anyhow::Error {
    if error.chain().any(|c| c.is::<rawmakase_cancel::Cancelled>()) {
        error
    } else {
        anyhow::anyhow!("AI Denoise: {error:#}")
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_failure_is_one_message_and_a_cancellation_stays_one() {
        let e = super::failed(anyhow::anyhow!("no model").context("Loading"));
        assert_eq!(e.to_string(), "AI Denoise: Loading: no model");
        let cancelled = anyhow::Error::new(rawmakase_cancel::Cancelled::new("Export cancelled"));
        let e = super::failed(cancelled);
        assert!(e.is::<rawmakase_cancel::Cancelled>());
        assert_eq!(e.to_string(), "Export cancelled");
    }
}
