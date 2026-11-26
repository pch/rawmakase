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

impl Error {
    /// An error from a crate under the library, of the kind its typed causes
    /// say.
    ///
    /// The crates mark each kind with a type somewhere in the chain
    /// (`rawmakase_cancel::Cancelled`, `rawmakase_model::error`'s four, a
    /// missing mask raster or catalog asset, the raster and RAW decoders' and
    /// AI Denoise's `Unsupported`); a missing file is the system's "not found". Work that
    /// was cancelled is cancelled whatever else its chain says. Otherwise the
    /// first cause in the chain that has a kind gives it, so a failure is the
    /// kind of what went wrong, not of a cause it was handled from.
    #[allow(
        dead_code,
        reason = "the operations that return the crates' errors join in step 5"
    )]
    pub(crate) fn of(cause: anyhow::Error) -> Self {
        let cancelled = cause.chain().any(|c| {
            c.is::<rawmakase_cancel::Cancelled>()
                || c.downcast_ref::<Error>().is_some_and(Error::is_cancelled)
        });
        let kind = if cancelled {
            ErrorKind::Cancelled
        } else {
            cause.chain().find_map(kind_of).unwrap_or(ErrorKind::Other)
        };
        Self { kind, cause }
    }
}

#[allow(dead_code, reason = "read by `Error::of`")]
fn kind_of(cause: &(dyn std::error::Error + 'static)) -> Option<ErrorKind> {
    use rawmakase_model::error::{Changed, Exists, Invalid, Missing};
    use rawmakase_model::storage::{mask_assets, recipe_assets};
    if let Some(e) = cause.downcast_ref::<Error>() {
        Some(e.kind)
    } else if cause.is::<rawmakase_decode::raster::Unsupported>()
        || cause.is::<rawmakase_decode::raw::Unsupported>()
        || cause.is::<rawmakase_decode::ai::denoise::Unsupported>()
    {
        Some(ErrorKind::Unsupported)
    } else if cause.is::<Missing>() {
        Some(ErrorKind::Missing)
    } else if cause.is::<Changed>() {
        Some(ErrorKind::Changed)
    } else if cause.is::<Exists>() {
        Some(ErrorKind::Exists)
    } else if cause.is::<Invalid>() {
        Some(ErrorKind::Invalid)
    } else if let Some(e) = cause.downcast_ref::<mask_assets::AssetError>() {
        use mask_assets::AssetError::{Missing, PatchMissing};
        matches!(e, Missing(_) | PatchMissing(_)).then_some(ErrorKind::Missing)
    } else if let Some(e) = cause.downcast_ref::<recipe_assets::AssetError>() {
        matches!(e, recipe_assets::AssetError::Missing(..)).then_some(ErrorKind::Missing)
    } else {
        // Only "not found": "already exists" from the system names no
        // destination (a cache folder blocked by a file says it too), so a
        // destination that exists is typed where it is refused.
        let io = cause.downcast_ref::<std::io::Error>()?;
        (io.kind() == std::io::ErrorKind::NotFound).then_some(ErrorKind::Missing)
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
        Error::of(e).kind()
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
        let taken = rawmakase_model::error::Exists::from_error(
            std::io::Error::from(std::io::ErrorKind::AlreadyExists).into(),
        );
        assert_eq!(kind(under(anyhow::Error::new(taken))), ErrorKind::Exists);
        assert_eq!(kind(anyhow::anyhow!("anything")), ErrorKind::Other);
    }

    #[test]
    fn existing_typed_causes_give_their_kinds() {
        use rawmakase_model::storage::{mask_assets, recipe_assets};
        let raster = mask_assets::AssetError::Missing("abc".into());
        assert_eq!(kind(anyhow::Error::new(raster)), ErrorKind::Missing);
        let patch = mask_assets::AssetError::PatchMissing("abc".into());
        assert_eq!(kind(anyhow::Error::new(patch)), ErrorKind::Missing);
        let damaged = mask_assets::AssetError::Corrupt("abc".into(), "short".into());
        assert_eq!(kind(anyhow::Error::new(damaged)), ErrorKind::Other);
        let asset = recipe_assets::AssetError::NoSource;
        assert_eq!(kind(anyhow::Error::new(asset)), ErrorKind::Other);
        let ai = rawmakase_decode::ai::denoise::Unsupported("a mosaic AI Denoise can't read");
        assert_eq!(kind(anyhow::Error::new(ai)), ErrorKind::Unsupported);
        let dir = tempfile::tempdir().unwrap();
        let not_raw = dir.path().join("notes.CR2");
        std::fs::write(&not_raw, b"text").unwrap();
        let refused = rawmakase_decode::raw::Raw::open_file(&not_raw)
            .err()
            .unwrap();
        assert_eq!(kind(refused), ErrorKind::Unsupported);
        // "Already exists" from the system is no destination's.
        let blocked = std::io::Error::from(std::io::ErrorKind::AlreadyExists);
        assert_eq!(kind(blocked.into()), ErrorKind::Other);
    }

    #[test]
    fn cancelled_work_is_cancelled_whatever_else_it_says() {
        let cancelled = || anyhow::Error::new(rawmakase_cancel::Cancelled::new("Cancelled"));
        let over = anyhow::Error::new(Invalid::over("Invalid preset", cancelled()));
        assert_eq!(kind(over), ErrorKind::Cancelled);
    }

    #[test]
    fn the_library_s_own_error_keeps_its_kind_through_anyhow() {
        let invalid = Error::new(ErrorKind::Invalid, "A cache needs a build id");
        let back = Error::of(anyhow::Error::new(invalid).context("Starting"));
        assert_eq!(back.kind(), ErrorKind::Invalid);
        let cancelled = Error::of(anyhow::Error::new(rawmakase_cancel::Cancelled::new("c")));
        assert_eq!(
            Error::of(anyhow::Error::new(cancelled)).kind(),
            ErrorKind::Cancelled
        );
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
        let e = Error::of(cause());
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
