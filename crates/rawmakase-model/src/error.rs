//! The kinds of failure a caller treats differently, as types a crate's
//! `anyhow` error can carry and a caller can find with
//! [`anyhow::Error::downcast_ref`].
//!
//! Each prints exactly what the failure printed before it had a type: its own
//! message, or the error it was made from, with that error's causes after it.
//! It is never a context laid over a second cause, so an error chain printed
//! with `{:#}` reads the same with the type as without it. A cancelled job is
//! `rawmakase_cancel::Cancelled`'s, and a file a decoder can't read
//! `rawmakase-decode`'s `Unsupported`.
#![warn(missing_docs)]

use std::fmt;

enum Cause {
    /// A message with no cause.
    Message(String),
    /// A message over the error that caused it, as `anyhow`'s `context` prints.
    Over(String, anyhow::Error),
    /// The error itself, which already says what happened.
    Error(anyhow::Error),
}

impl Cause {
    fn display(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Message(message) | Self::Over(message, _) => f.write_str(message),
            Self::Error(error) => fmt::Display::fmt(&**error, f),
        }
    }
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Message(_) => None,
            Self::Over(_, error) => Some(&**error),
            Self::Error(error) => error.source(),
        }
    }
}

macro_rules! kind {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        pub struct $name(Cause);

        impl $name {
            /// This kind of failure, which says `message`.
            pub fn new(message: impl Into<String>) -> Self {
                Self(Cause::Message(message.into()))
            }
            /// This kind of failure, which says `message` and was caused by
            /// `cause`: printed as `cause.context(message)` would print.
            pub fn over(message: impl Into<String>, cause: anyhow::Error) -> Self {
                Self(Cause::Over(message.into(), cause))
            }
            /// `error`, which already says what happened, as this kind: it
            /// prints as `error` does, causes and all.
            pub fn from_error(error: anyhow::Error) -> Self {
                Self(Cause::Error(error))
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.display(f)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({:?})", stringify!($name), self.to_string())
            }
        }

        impl std::error::Error for $name {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                self.0.source()
            }
        }
    };
}

kind!(
    /// A recipe or setting that doesn't validate.
    Invalid
);
kind!(
    /// Something a job needs isn't there: a camera profile, a preset.
    Missing
);
kind!(
    /// The file is no longer the one the job was made for.
    Changed
);
kind!(
    /// The destination is already there, and the call was not told to replace
    /// it.
    Exists
);

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::{Context, anyhow};

    fn printed(e: &anyhow::Error) -> [String; 2] {
        [format!("{e}"), format!("{e:#}")]
    }

    #[test]
    fn each_prints_what_the_error_it_replaces_printed() {
        let message = anyhow::Error::new(Missing::new("No preset is called “Warm”"));
        assert_eq!(
            printed(&message),
            printed(&anyhow!("No preset is called “Warm”"))
        );

        let plain: anyhow::Result<()> = Err(anyhow!("not in the library"));
        let over = anyhow::Error::new(Missing::over(
            "Missing camera profile ‘X’",
            anyhow!("not in the library"),
        ));
        let context = plain.context("Missing camera profile ‘X’").unwrap_err();
        assert_eq!(printed(&over), printed(&context));
        assert_eq!(over.chain().count(), context.chain().count());

        let deep = || anyhow!("the curve's points").context("Invalid tone curve");
        let typed = anyhow::Error::new(Invalid::from_error(deep()));
        assert_eq!(printed(&typed), printed(&deep()));
        assert_eq!(typed.chain().count(), deep().chain().count());
    }

    #[test]
    fn a_caller_finds_the_kind_under_context() {
        let e = anyhow::Error::new(Exists::new("Destination already exists")).context("Export");
        assert!(e.chain().any(|c| c.is::<Exists>()));
        assert!(!e.chain().any(|c| c.is::<Changed>()));
    }
}
