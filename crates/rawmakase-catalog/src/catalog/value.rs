//! The values the catalog stores and reads, in its own terms rather than a
//! database library's, so a second backend could carry them (issue #341).
//!
//! Decoding follows the rules the catalog has always read SQLite with:
//! integers only as integers, reals from integers too, any nonzero integer
//! as `true`, and narrower integers through checked conversions.
use super::db::Row;
use crate::ids::{CollectionId, FolderId, PhotoId, PhotoUuid, RootId, StackId};
use std::fmt;

/// One stored value, borrowed: SQL's five storage classes. Text is bytes,
/// as a damaged catalog may hold text that isn't UTF-8.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::catalog) enum ValueRef<'a> {
    Null,
    Integer(i64),
    Real(f64),
    Text(&'a [u8]),
    Blob(&'a [u8]),
}

impl ValueRef<'_> {
    fn kind(&self) -> &'static str {
        match self {
            Self::Null => "NULL",
            Self::Integer(_) => "INTEGER",
            Self::Real(_) => "REAL",
            Self::Text(_) => "TEXT",
            Self::Blob(_) => "BLOB",
        }
    }
}

/// Why a stored value couldn't be read as the type asked for.
#[derive(Debug, PartialEq)]
pub(in crate::catalog) enum ValueError {
    /// A value of another storage class, such as text where a number belongs.
    InvalidType {
        expected: &'static str,
        found: &'static str,
    },
    /// An integer the type can't hold.
    OutOfRange(i64),
    /// Text that isn't UTF-8.
    InvalidText,
    /// Text that isn't a photo's UUID.
    InvalidUuid,
}

impl fmt::Display for ValueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidType { expected, found } => {
                write!(f, "expected {expected}, found {found}")
            }
            Self::OutOfRange(value) => write!(f, "integer {value} out of range"),
            Self::InvalidText => f.write_str("text is not UTF-8"),
            Self::InvalidUuid => f.write_str("text is not a photo's UUID"),
        }
    }
}

impl std::error::Error for ValueError {}

/// A value the catalog can store.
pub(in crate::catalog) trait ToValue {
    fn to_value(&self) -> ValueRef<'_>;
}

/// A value the catalog can read from one column.
pub(in crate::catalog) trait FromValue: Sized {
    fn from_value(value: ValueRef<'_>) -> Result<Self, ValueError>;
}

fn invalid<T>(expected: &'static str, value: ValueRef<'_>) -> Result<T, ValueError> {
    Err(ValueError::InvalidType {
        expected,
        found: value.kind(),
    })
}

impl ToValue for i64 {
    fn to_value(&self) -> ValueRef<'_> {
        ValueRef::Integer(*self)
    }
}

impl FromValue for i64 {
    fn from_value(value: ValueRef<'_>) -> Result<Self, ValueError> {
        match value {
            ValueRef::Integer(i) => Ok(i),
            other => invalid("INTEGER", other),
        }
    }
}

/// Integers narrower than `i64`, written widened and read through a checked
/// conversion: an out-of-range value is an error, never truncated.
macro_rules! narrow_integer {
    ($($ty:ty),*) => {$(
        impl ToValue for $ty {
            fn to_value(&self) -> ValueRef<'_> {
                ValueRef::Integer(i64::from(*self))
            }
        }

        impl FromValue for $ty {
            fn from_value(value: ValueRef<'_>) -> Result<Self, ValueError> {
                let wide = i64::from_value(value)?;
                <$ty>::try_from(wide).map_err(|_| ValueError::OutOfRange(wide))
            }
        }
    )*};
}

narrow_integer!(i32, u32);

impl ToValue for f64 {
    fn to_value(&self) -> ValueRef<'_> {
        ValueRef::Real(*self)
    }
}

impl FromValue for f64 {
    /// A real, or an integer as a real (a photo's width and height are
    /// stored as integers and divided as reals).
    fn from_value(value: ValueRef<'_>) -> Result<Self, ValueError> {
        match value {
            ValueRef::Integer(i) => Ok(i as f64),
            ValueRef::Real(f) => Ok(f),
            other => invalid("REAL", other),
        }
    }
}

impl ToValue for bool {
    /// 1 or 0.
    fn to_value(&self) -> ValueRef<'_> {
        ValueRef::Integer(i64::from(*self))
    }
}

impl FromValue for bool {
    /// Any nonzero integer is true: not every column holding a flag is
    /// constrained to 0 and 1.
    fn from_value(value: ValueRef<'_>) -> Result<Self, ValueError> {
        i64::from_value(value).map(|i| i != 0)
    }
}

impl ToValue for str {
    fn to_value(&self) -> ValueRef<'_> {
        ValueRef::Text(self.as_bytes())
    }
}

impl ToValue for String {
    fn to_value(&self) -> ValueRef<'_> {
        self.as_str().to_value()
    }
}

impl FromValue for String {
    fn from_value(value: ValueRef<'_>) -> Result<Self, ValueError> {
        match value {
            ValueRef::Text(bytes) => std::str::from_utf8(bytes)
                .map(str::to_owned)
                .map_err(|_| ValueError::InvalidText),
            other => invalid("TEXT", other),
        }
    }
}

impl ToValue for std::borrow::Cow<'_, str> {
    fn to_value(&self) -> ValueRef<'_> {
        self.as_ref().to_value()
    }
}

impl ToValue for [u8] {
    fn to_value(&self) -> ValueRef<'_> {
        ValueRef::Blob(self)
    }
}

impl ToValue for Vec<u8> {
    fn to_value(&self) -> ValueRef<'_> {
        self.as_slice().to_value()
    }
}

impl FromValue for Vec<u8> {
    fn from_value(value: ValueRef<'_>) -> Result<Self, ValueError> {
        match value {
            ValueRef::Blob(bytes) => Ok(bytes.to_vec()),
            other => invalid("BLOB", other),
        }
    }
}

impl<T: ToValue + ?Sized> ToValue for &T {
    fn to_value(&self) -> ValueRef<'_> {
        (**self).to_value()
    }
}

impl<T: ToValue> ToValue for Option<T> {
    /// NULL for `None`.
    fn to_value(&self) -> ValueRef<'_> {
        self.as_ref().map_or(ValueRef::Null, ToValue::to_value)
    }
}

impl<T: FromValue> FromValue for Option<T> {
    /// `None` for NULL.
    fn from_value(value: ValueRef<'_>) -> Result<Self, ValueError> {
        match value {
            ValueRef::Null => Ok(None),
            other => T::from_value(other).map(Some),
        }
    }
}

/// The bytes of a column some rows store as TEXT and others as BLOB, such
/// as Lightroom's settings text; `None` for a value of any other class,
/// which the reader skips or replaces as it sees fit.
#[derive(Clone, Debug, Default, PartialEq)]
pub(in crate::catalog) struct TextOrBlob(pub Option<Vec<u8>>);

impl FromValue for TextOrBlob {
    fn from_value(value: ValueRef<'_>) -> Result<Self, ValueError> {
        Ok(Self(match value {
            ValueRef::Text(bytes) | ValueRef::Blob(bytes) => Some(bytes.to_vec()),
            _ => None,
        }))
    }
}

/// Typed ids are stored as their integer.
macro_rules! row_id {
    ($($id:ty),*) => {$(
        impl ToValue for $id {
            fn to_value(&self) -> ValueRef<'_> {
                ValueRef::Integer(self.0)
            }
        }

        impl FromValue for $id {
            fn from_value(value: ValueRef<'_>) -> Result<Self, ValueError> {
                i64::from_value(value).map(Self)
            }
        }
    )*};
}

row_id!(CollectionId, FolderId, PhotoId, RootId, StackId);

/// A photo's UUID is stored as its text.
impl ToValue for PhotoUuid {
    fn to_value(&self) -> ValueRef<'_> {
        self.as_str().to_value()
    }
}

impl FromValue for PhotoUuid {
    fn from_value(value: ValueRef<'_>) -> Result<Self, ValueError> {
        PhotoUuid::parse(&String::from_value(value)?).ok_or(ValueError::InvalidUuid)
    }
}

/// A row the catalog can read: usually a named struct, or a single value
/// from a one-column query.
pub(in crate::catalog) trait FromRow: Sized {
    fn from_row(row: &Row<'_>) -> anyhow::Result<Self>;
}

impl<T: FromValue> FromRow for T {
    fn from_row(row: &Row<'_>) -> anyhow::Result<Self> {
        row.get(0)
    }
}

/// A struct read from a query's row, one field per column in order.
///
/// ```ignore
/// row! {
///     struct Located { root: RootId, path: String }
/// }
/// let rows: Vec<Located> = db.read(sql!("SELECT root, path FROM …"), &[])?;
/// ```
macro_rules! row {
    ($(#[$meta:meta])* $vis:vis struct $name:ident {
        $($(#[$field_meta:meta])* $field:ident: $ty:ty),* $(,)?
    }) => {
        $(#[$meta])*
        $vis struct $name {
            $($(#[$field_meta])* $field: $ty),*
        }

        impl $crate::catalog::value::FromRow for $name {
            fn from_row(row: &$crate::catalog::db::Row<'_>) -> anyhow::Result<Self> {
                let mut columns = 0..;
                Ok(Self {
                    $($field: row.get(columns.next().expect("unbounded"))?),*
                })
            }
        }
    };
}

pub(in crate::catalog) use row;
