//! Typed catalog ids, so a collection's id can never be passed where a photo's
//! is meant. Each row id is the table's integer key, stored and serialized as
//! that integer. [`PhotoUuid`] is not a row id: it is a photo's own UUID,
//! stored as text, for what outlasts a session or a catalog. A leaf module:
//! edit resolution and export, which sit below the catalog, name photos with
//! them too; `catalog` re-exports them.
use serde::{Deserialize, Serialize};

macro_rules! row_id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub i64);
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}

row_id!(
    /// A collection or collection set (`collections.id`).
    CollectionId
);
row_id!(
    /// A photo or one of its virtual copies (`photos.id`).
    PhotoId
);
row_id!(
    /// A folder of photos (`folders.id`).
    FolderId
);
row_id!(
    /// A folder stack (`stacks.id`): photos of one folder shown under the
    /// first.
    StackId
);
row_id!(
    /// A root folder, as a catalog records it (`roots.id`); its folders are paths
    /// under it.
    RootId
);

/// A photo's own id, for extensions and other catalogs: a UUID the catalog
/// made once for one photo or virtual copy (`photo_uuids.uuid`), never changed
/// or given to another photo, where an older release may give a removed
/// photo's [`PhotoId`] to another. Copies of a catalog file share them.
///
/// Only the catalog makes and stores one; outside it a value comes from the
/// catalog or from [`PhotoUuid::parse`], as when an extension sends one back.
/// Always lowercase and hyphenated.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PhotoUuid(String);

impl PhotoUuid {
    /// Reads a UUID in its hyphenated form, in either case. Any version is
    /// taken, since nothing depends on it; the nil UUID is not, since no
    /// photo has it.
    pub fn parse(text: &str) -> Option<Self> {
        let bytes = text.as_bytes();
        let well_formed = bytes.len() == 36
            && bytes.iter().enumerate().all(|(i, b)| match i {
                8 | 13 | 18 | 23 => *b == b'-',
                _ => b.is_ascii_hexdigit(),
            });
        let nil = bytes.iter().all(|b| matches!(b, b'0' | b'-'));
        (well_formed && !nil).then(|| Self(text.to_ascii_lowercase()))
    }
    /// The UUID, lowercase and hyphenated.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for PhotoUuid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_id_is_serialized_and_shown_as_its_integer() {
        let id = CollectionId(42);
        assert_eq!(serde_json::to_string(&id).unwrap(), "42");
        assert_eq!(serde_json::from_str::<CollectionId>("42").unwrap(), id);
        assert_eq!(id.to_string(), "42");
    }

    #[test]
    fn a_photo_uuid_is_read_in_either_case_and_kept_lowercase() {
        let uuid = PhotoUuid::parse("3F2504E0-4F89-41D3-9A0C-0305E82C3301").unwrap();
        assert_eq!(uuid.as_str(), "3f2504e0-4f89-41d3-9a0c-0305e82c3301");
        assert_eq!(uuid.to_string(), uuid.as_str());
        assert_eq!(
            PhotoUuid::parse("3f2504e0-4f89-41d3-9a0c-0305e82c3301"),
            Some(uuid)
        );
    }

    #[test]
    fn only_a_hyphenated_non_nil_uuid_is_a_photo_uuid() {
        for text in [
            "",
            "00000000-0000-0000-0000-000000000000",
            "3f2504e04f8941d39a0c0305e82c3301",
            "{3f2504e0-4f89-41d3-9a0c-0305e82c3301}",
            "3f2504e0-4f89-41d3-9a0c-0305e82c330",
            "3f2504e0-4f89-41d3-9a0c-0305e82c33011",
            "3f2504e0-4f89-41d3-9a0c+0305e82c3301",
            "3g2504e0-4f89-41d3-9a0c-0305e82c3301",
            " 3f2504e0-4f89-41d3-9a0c-0305e82c3301",
        ] {
            assert_eq!(PhotoUuid::parse(text), None, "{text:?}");
        }
    }
}
