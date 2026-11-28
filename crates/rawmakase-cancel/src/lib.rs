//! Cancellation of long-running work: decoding, rendering, measuring, file
//! operations and inference. The work's owner keeps the flag it sets (usually an
//! `Arc<AtomicBool>` in the task that runs the work) and lends it to the work as a
//! [`Cancel`], which the work reads at its own cancellation points. Work that
//! stops returns [`Cancelled`] through [`Cancel::check`], with its own wording,
//! so a caller can tell a cancelled job from a failed one by its type.
//!
//! It depends on no other crate, so every crate, the RAW decoders and inference
//! among them, can take it.
#![forbid(unsafe_code)]
// Every public item says what it is for: this crate is a contract other crates
// and clients build on.
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

    /// [`Cancelled`] with `message` once the owner has asked the work to stop.
    /// `message` is the work's own wording ("Development cancelled"), which is
    /// all the error prints.
    pub fn check(self, message: &'static str) -> Result<(), Cancelled> {
        match self.is_cancelled() {
            true => Err(Cancelled(message)),
            false => Ok(()),
        }
    }
}

/// Work stopped because its owner asked it to, not because it failed. It
/// prints the wording of the work that stopped, and has no cause.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cancelled(&'static str);

impl Cancelled {
    /// Cancelled work that says `message`.
    pub const fn new(message: &'static str) -> Self {
        Self(message)
    }

    /// The work's own wording.
    pub const fn message(self) -> &'static str {
        self.0
    }
}

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for Cancelled {}

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

    #[test]
    fn check_returns_the_work_s_own_wording() {
        let flag = AtomicBool::new(false);
        assert_eq!(Cancel::new(&flag).check("Opening cancelled"), Ok(()));
        flag.store(true, Ordering::Relaxed);
        let error = Cancel::new(&flag).check("Opening cancelled").unwrap_err();
        assert_eq!(error, Cancelled::new("Opening cancelled"));
        assert_eq!(error.to_string(), "Opening cancelled");
        assert!(std::error::Error::source(&error).is_none());
        assert_eq!(Cancel::NEVER.check("never"), Ok(()));
    }
}
