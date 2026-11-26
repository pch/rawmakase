//! The library's one error type: which kind of thing happened, for the kinds
//! a caller treats differently, and the chain of causes from the crates under
//! it.
use std::fmt;

/// What the library's operations return.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// The kind of a failure, for the kinds a caller acts on differently.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ErrorKind {
    /// The `Cancel` the operation was given was set. Not a failure to show: the
    /// caller asked for it.
    Cancelled,
    /// The file or the request is one RAWmakase does not handle, such as a
    /// CMYK JPEG. The message says which.
    Unsupported,
    /// Something named is not there: a file, a camera profile, a preset.
    Missing,
    /// The file is no longer the one the work was made for.
    Changed,
    /// The destination is already there, and the call was not told to replace
    /// it.
    Exists,
    /// A recipe, a setting or a piece of configuration that doesn't validate.
    Invalid,
    /// Anything else, as its message.
    Other,
}

/// An error of the library: its [`ErrorKind`] and the chain of causes.
///
/// `{}` prints the outermost message and `{:#}` the whole chain, as the command
/// line prints errors; neither adds anything of the library's own.
pub struct Error {
    kind: ErrorKind,
    cause: anyhow::Error,
}

impl Error {
    /// Which kind of failure this is.
    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    /// Whether the operation stopped because its `Cancel` was set.
    pub fn is_cancelled(&self) -> bool {
        self.kind == ErrorKind::Cancelled
    }

    /// A failure of `kind` the library found itself, saying `message`.
    pub(crate) fn new(kind: ErrorKind, message: impl fmt::Display) -> Self {
        Self {
            kind,
            cause: anyhow::anyhow!("{message}"),
        }
    }
}

/// An error from a crate under the library, of the kind its typed causes say.
///
/// The crates mark each kind with a type somewhere in the chain
/// (`rawmakase_cancel::Cancelled`, `rawmakase_model::error`'s four,
/// `rawmakase_decode::raster::Unsupported`); a missing file is the system's
/// "not found". The first cause in the chain that has a kind gives it, so a
/// failure is the kind of what went wrong, not of a cause it was handled from.
impl From<anyhow::Error> for Error {
    fn from(cause: anyhow::Error) -> Self {
        let kind = cause.chain().find_map(kind_of).unwrap_or(ErrorKind::Other);
        Self { kind, cause }
    }
}

fn kind_of(cause: &(dyn std::error::Error + 'static)) -> Option<ErrorKind> {
    use rawmakase_model::error::{Changed, Exists, Invalid, Missing};
    if cause.is::<rawmakase_cancel::Cancelled>() {
        Some(ErrorKind::Cancelled)
    } else if cause.is::<rawmakase_decode::raster::Unsupported>() {
        Some(ErrorKind::Unsupported)
    } else if cause.is::<Missing>() {
        Some(ErrorKind::Missing)
    } else if cause.is::<Changed>() {
        Some(ErrorKind::Changed)
    } else if cause.is::<Exists>() {
        Some(ErrorKind::Exists)
    } else if cause.is::<Invalid>() {
        Some(ErrorKind::Invalid)
    } else {
        let io = cause.downcast_ref::<std::io::Error>()?;
        match io.kind() {
            std::io::ErrorKind::NotFound => Some(ErrorKind::Missing),
            std::io::ErrorKind::AlreadyExists => Some(ErrorKind::Exists),
            _ => None,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.cause, f)
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Error")
            .field("kind", &self.kind)
            .field("message", &format_args!("{:#}", self.cause))
            .finish()
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.cause.source()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Context;
    use rawmakase_model::error::{Changed, Exists, Invalid, Missing};

    fn kind(e: anyhow::Error) -> ErrorKind {
        Error::from(e).kind()
    }

    #[test]
    fn each_typed_cause_gives_its_kind_under_added_context() {
        let under = |e: anyhow::Error| e.context("Opening the photo");
        let cancelled = anyhow::Error::new(rawmakase_cancel::Cancelled::new("Opening cancelled"));
        assert_eq!(kind(under(cancelled)), ErrorKind::Cancelled);
        let unsupported = rawmakase_decode::raster::Unsupported("CMYK".into());
        assert_eq!(
            kind(under(anyhow::Error::new(unsupported))),
            ErrorKind::Unsupported
        );
        for (e, want) in [
            (anyhow::Error::new(Missing::new("m")), ErrorKind::Missing),
            (anyhow::Error::new(Changed::new("c")), ErrorKind::Changed),
            (anyhow::Error::new(Exists::new("e")), ErrorKind::Exists),
            (anyhow::Error::new(Invalid::new("i")), ErrorKind::Invalid),
        ] {
            assert_eq!(kind(under(e)), want);
        }
        let gone = std::io::Error::from(std::io::ErrorKind::NotFound);
        assert_eq!(kind(under(gone.into())), ErrorKind::Missing);
        assert_eq!(kind(anyhow::anyhow!("anything")), ErrorKind::Other);
    }

    #[test]
    fn the_outermost_typed_cause_wins() {
        // An invalid setting found while handling a missing file is invalid.
        let gone = anyhow::Error::from(std::io::Error::from(std::io::ErrorKind::NotFound));
        let e = anyhow::Error::new(Invalid::over("Invalid preset", gone));
        assert_eq!(kind(e), ErrorKind::Invalid);
    }

    #[test]
    fn it_prints_the_chain_it_was_made_from() {
        let cause = || {
            anyhow::Result::<()>::Err(anyhow::anyhow!("no such profile"))
                .context("Loading the recipe")
                .unwrap_err()
        };
        let e = Error::from(cause());
        assert_eq!(e.to_string(), cause().to_string());
        assert_eq!(format!("{e:#}"), format!("{:#}", cause()));
        assert_eq!(
            std::error::Error::source(&e).map(ToString::to_string),
            Some("no such profile".into())
        );
    }

    #[test]
    fn errors_cross_threads() {
        fn send_sync<T: Send + Sync + 'static>() {}
        send_sync::<Error>();
    }
}
