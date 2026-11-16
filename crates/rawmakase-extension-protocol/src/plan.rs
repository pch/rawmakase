//! The plan an `[[importer]]` writes for `import.run`: a SQLite file with
//! exactly the tables [`SCHEMA`] creates, which the app reads, checks and
//! imports into a new catalog in one transaction
//! (`docs/extensions/protocol.md`, "The plan").
//!
//! The program creates the file at the path the app gives, runs [`SCHEMA`]
//! as it is, writes one `plan` row and fills the tables, and closes the file
//! before it answers. The app compares the file's schema with [`SCHEMA`]
//! statement for statement and refuses a plan with any other table, index,
//! view or trigger. It doesn't rely on the schema's constraints either: it
//! checks every rule below itself, and a plan that breaks one is refused
//! whole, never imported in part.
//!
//! - Ids are text the importer chooses, unique per table. A reference names
//!   an id of its table; a nullable one (a virtual copy's `master`, a
//!   collection's or keyword's `parent`) may be NULL.
//! - `plan.source_photos` is how many photos the source holds, counted on
//!   their own rather than from the rows written: a photo the importer
//!   couldn't place fails the import instead of being left out. It must
//!   equal the `photo` rows and the answer's `photos`.
//! - At most one `info`, `metadata` and `edit` row per photo, and one
//!   History step per position. A photo is in at most one stack, at a
//!   position no other photo of that stack has; a stack's photos share a
//!   folder. A virtual copy's master is no copy and is in its folder.
//! - A collection or keyword link appears once. A `set` holds no photos.
//!   A `quick` collection (Lightroom's Quick Collection) is named
//!   [`QUICK_COLLECTION`] and has no parent.
//! - Ratings are 0 to 5, flags -1 (rejected), 0 or 1 (picked). Develop
//!   text (edits, History steps, snapshots) and XMP are shorter than
//!   [`MAX_TEXT_BYTES`] bytes. Every value is a value of its column's type.
//!
//! What each column holds:
//!
//! - `root`: the absolute path a folder root had in the source.
//!   `folder.path` is relative to its root.
//! - `photo`: the file's name and absolute path in the source; the capture
//!   time as Lightroom stores it (`2021-06-06T10:00:00`, empty when
//!   unknown); colour label, file format and Lightroom orientation code
//!   (`AB`, `BC`, …); for a virtual copy its master and copy name; the
//!   file's name when it was first imported, when known.
//! - `info`: focal length in millimetres, the f-number, exposure time in
//!   seconds, ISO, and the size the photo shows at, already turned for its
//!   orientation.
//! - `metadata`: an XMP packet, applied only to fields the photo has no
//!   value for; its keywords, rating, label and flag are ignored.
//! - `edit`: the photo's develop settings in `format` `lightroom-develop`
//!   (Lightroom's develop text, uncompressed), the only format.
//! - `history` and `snapshot`: Lightroom develop text, uncompressed. A
//!   History step's `created` is seconds since 2001-01-01.
//! - `collection.kind`: `set`, `collection`, `smart`, `quick` or `system`
//!   (Lightroom's own, such as an unsaved print). `collection_photo.position`
//!   is the place in a custom order, as Lightroom keeps it.
//! - `stack_photo.position` orders a stack, the top first; the app numbers
//!   them from 1.
//! - `keyword`: Lightroom's nameless root keyword is one with an empty name
//!   and no parent. `include_on_export` and `export_containing` are 0 or 1.

/// The plan format this version reads, in `plan.version`.
pub const PLAN_VERSION: i64 = 1;

/// Develop text and XMP in a plan must be shorter than this, in bytes, as
/// the app's readers of them require.
pub const MAX_TEXT_BYTES: usize = 16_000_000;

/// The name a `quick` collection must have.
pub const QUICK_COLLECTION: &str = "quick collection";

/// The plan's tables, to run as they are on the new, empty file. Their
/// constraints catch an importer's mistakes as it writes; the app checks the
/// same rules again on its own.
pub const SCHEMA: &str = "\
CREATE TABLE plan (
    version INTEGER NOT NULL,
    source_photos INTEGER NOT NULL CHECK (source_photos >= 0)
) STRICT;
CREATE TABLE root (
    id TEXT PRIMARY KEY NOT NULL,
    path TEXT NOT NULL
) STRICT;
CREATE TABLE folder (
    id TEXT PRIMARY KEY NOT NULL,
    root TEXT NOT NULL,
    path TEXT NOT NULL
) STRICT;
CREATE TABLE photo (
    id TEXT PRIMARY KEY NOT NULL,
    folder TEXT NOT NULL,
    file_name TEXT NOT NULL,
    original_path TEXT NOT NULL,
    captured TEXT NOT NULL,
    rating INTEGER NOT NULL CHECK (rating BETWEEN 0 AND 5),
    flag INTEGER NOT NULL CHECK (flag BETWEEN -1 AND 1),
    label TEXT NOT NULL,
    format TEXT NOT NULL,
    orientation TEXT,
    master TEXT,
    copy_name TEXT NOT NULL,
    original_name TEXT
) STRICT;
CREATE TABLE info (
    photo TEXT PRIMARY KEY NOT NULL,
    camera TEXT,
    lens TEXT,
    focal REAL,
    aperture REAL,
    exposure REAL,
    iso REAL,
    width INTEGER,
    height INTEGER
) STRICT;
CREATE TABLE metadata (
    photo TEXT PRIMARY KEY NOT NULL,
    xmp TEXT NOT NULL
) STRICT;
CREATE TABLE edit (
    photo TEXT PRIMARY KEY NOT NULL,
    format TEXT NOT NULL CHECK (format = 'lightroom-develop'),
    text TEXT NOT NULL
) STRICT;
CREATE TABLE history (
    photo TEXT NOT NULL,
    position INTEGER NOT NULL,
    name TEXT NOT NULL,
    created REAL,
    text TEXT NOT NULL,
    PRIMARY KEY (photo, position)
) STRICT;
CREATE TABLE snapshot (
    photo TEXT NOT NULL,
    name TEXT NOT NULL,
    text TEXT NOT NULL
) STRICT;
CREATE TABLE collection (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    parent TEXT,
    kind TEXT NOT NULL CHECK (kind IN ('set', 'collection', 'smart', 'quick', 'system'))
) STRICT;
CREATE TABLE collection_photo (
    collection TEXT NOT NULL,
    photo TEXT NOT NULL,
    position TEXT,
    PRIMARY KEY (collection, photo)
) STRICT;
CREATE TABLE stack (
    id TEXT PRIMARY KEY NOT NULL,
    collapsed INTEGER NOT NULL CHECK (collapsed IN (0, 1))
) STRICT;
CREATE TABLE stack_photo (
    stack TEXT NOT NULL,
    photo TEXT NOT NULL UNIQUE,
    position INTEGER NOT NULL,
    UNIQUE (stack, position)
) STRICT;
CREATE TABLE keyword (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    parent TEXT,
    include_on_export INTEGER NOT NULL CHECK (include_on_export IN (0, 1)),
    export_containing INTEGER NOT NULL CHECK (export_containing IN (0, 1))
) STRICT;
CREATE TABLE photo_keyword (
    photo TEXT NOT NULL,
    keyword TEXT NOT NULL,
    PRIMARY KEY (photo, keyword)
) STRICT;
";
