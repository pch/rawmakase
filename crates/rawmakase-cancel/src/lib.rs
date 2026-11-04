//! Cancellation of long-running work: decoding, rendering, measuring, file
//! operations and inference. The work's owner keeps the flag it sets (usually an
//! `Arc<AtomicBool>` in the task that runs the work) and lends it to the work as a
//! [`Cancel`], which the work reads at its own cancellation points. What a cancelled
//! piece of work returns is its own: each keeps its error.
//!
//! It depends on no other crate, so every crate, the RAW decoders and inference
//! among them, can take it.
#![forbid(unsafe_code)]
// Every public item says what it is for: this crate is a contract other crates
// and clients build on (#808 step 4.13).
#![warn(missing_docs)]
use std::sync::atomic::{AtomicBool, Ordering};

/// A request to stop work early: a borrowed flag the owner sets, or
/// [`Cancel::NEVER`] for work no one cancels.
#[derive(Clone, Copy, Debug)]
pub struct Cancel<'a>(Option<&'a AtomicBool>);

impl<'a> Cancel<'a> {
    /// Work that is never cancelled.
    pub const NEVER: Self = Self(None);

    /// Cancelled once `flag` is set.
    pub fn new(flag: &'a AtomicBool) -> Self {
        Self(Some(flag))
    }

    /// Whether the owner has asked the work to stop. A relaxed read: the flag orders
    /// nothing else, so the work may see it a little late, and stops at its next
    /// cancellation point.
    pub fn is_cancelled(self) -> bool {
        self.0.is_some_and(|flag| flag.load(Ordering::Relaxed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn never_is_never_cancelled() {
        assert!(!Cancel::NEVER.is_cancelled());
    }

    /// Copies read the owner's flag as it is now, whether it was set before or after
    /// they were made, and see it cleared again.
    #[test]
    fn copies_follow_the_owners_flag() {
        let set = AtomicBool::new(true);
        assert!(Cancel::new(&set).is_cancelled());
        let flag = AtomicBool::new(false);
        let cancel = Cancel::new(&flag);
        let copy = cancel;
        assert!(!copy.is_cancelled());
        flag.store(true, Ordering::Relaxed);
        assert!(cancel.is_cancelled() && copy.is_cancelled());
        flag.store(false, Ordering::Relaxed);
        assert!(!copy.is_cancelled());
    }
}
