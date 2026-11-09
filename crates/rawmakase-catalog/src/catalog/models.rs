use std::path::PathBuf;
#[derive(Clone, Debug, Default)]
pub struct Photo {
    pub id: super::PhotoId,
    pub folder: super::FolderId,
    pub path: PathBuf,
    pub filename: String,
    pub captured: String,
    pub rating: i32,
    pub flag: i32,
    pub label: String,
    pub format: String,
    pub copy_name: String,
    /// The photo this one is a virtual copy of; `None` for a master.
    pub master: Option<super::PhotoId>,
    /// Its place in a folder stack, if it is in one.
    pub stack: Option<super::StackPlace>,
    /// The names of its keywords, each whole: a name read from XMP may
    /// hold a comma.
    pub keywords: Vec<String>,
    pub has_lightroom_edits: bool,
}
impl Photo {
    /// The capture time as Lightroom shows it, "29/06/2016 18:24:27.000";
    /// empty when there is none.
    pub fn capture_text(&self) -> String {
        let c = &self.captured;
        let part = |range: std::ops::Range<usize>| c.get(range).unwrap_or("");
        if c.len() < 19 {
            return c.clone();
        }
        format!(
            "{}/{}/{} {}",
            part(8..10),
            part(5..7),
            part(0..4),
            part(11..c.len())
        )
    }
}
#[derive(Clone, Debug)]
pub struct Folder {
    pub relative: String,
    pub id: super::FolderId,
    pub root: super::RootId,
    pub path: PathBuf,
    pub count: usize,
}
#[derive(Clone, Debug)]
pub struct Collection {
    pub id: super::CollectionId,
    pub name: String,
    pub parent: Option<super::CollectionId>,
    pub kind: CollectionKind,
}
/// The name Lightroom gives its Quick Collection.
pub const QUICK_COLLECTION: &str = "quick collection";
/// What a Lightroom collection row is, from its `creationId`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CollectionKind {
    /// A collection set: holds collections and other sets, never photos.
    Set,
    /// A collection, or a saved print, book, slideshow or web creation.
    Collection,
    /// A smart collection, whose rules are not evaluated yet.
    Smart,
    /// Lightroom's own: the Quick Collection and unsaved creations.
    System,
}
impl CollectionKind {
    pub fn from_lightroom(creation_id: &str, name: &str) -> Self {
        match creation_id {
            "com.adobe.ag.library.group" => Self::Set,
            "com.adobe.ag.library.smart_collection" => Self::Smart,
            // Lightroom keeps its Quick Collection as a plain collection by this name.
            "com.adobe.ag.library.collection" if name == QUICK_COLLECTION => Self::System,
            id if id.ends_with(".unsaved") => Self::System,
            _ => Self::Collection,
        }
    }
}
