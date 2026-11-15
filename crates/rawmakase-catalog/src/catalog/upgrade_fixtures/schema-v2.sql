-- Every table of a RAWmakase catalog (SQLite application id 0x4f4d4152,
-- user_version 1). Idempotent: `Catalog::create` runs it on a new file and
-- `Catalog::open` on every open, so a catalog from an earlier release gains the
-- tables added since. Tables are only ever added; a change to an existing one
-- needs a new user_version and a migration.

CREATE TABLE IF NOT EXISTS sources (
    id INTEGER PRIMARY KEY,
    path TEXT NOT NULL,
    imported_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    original_size INTEGER NOT NULL,
    original_catalog BLOB NOT NULL
);

CREATE TABLE IF NOT EXISTS roots (
    id INTEGER PRIMARY KEY,
    original_path TEXT NOT NULL,
    mapped_path TEXT
);

CREATE TABLE IF NOT EXISTS folders (
    id INTEGER PRIMARY KEY,
    root INTEGER NOT NULL REFERENCES roots(id),
    relative_path TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS photos (
    id INTEGER PRIMARY KEY,
    folder INTEGER NOT NULL REFERENCES folders(id),
    filename TEXT NOT NULL,
    original_path TEXT NOT NULL,
    captured TEXT NOT NULL DEFAULT '',
    rating INTEGER NOT NULL DEFAULT 0 CHECK(rating BETWEEN 0 AND 5),
    flag INTEGER NOT NULL DEFAULT 0 CHECK(flag BETWEEN -1 AND 1),
    label TEXT NOT NULL DEFAULT '',
    format TEXT NOT NULL DEFAULT '',
    copy_name TEXT NOT NULL DEFAULT '',
    master_id INTEGER,
    orientation TEXT,
    lightroom_develop TEXT,
    recipe TEXT,
    export_options TEXT,
    identity TEXT,
    edited_at TEXT
);

CREATE INDEX IF NOT EXISTS photos_folder ON photos(folder);

CREATE INDEX IF NOT EXISTS photos_captured ON photos(captured);

CREATE INDEX IF NOT EXISTS photos_master ON photos(master_id);

CREATE TABLE IF NOT EXISTS collections (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    parent INTEGER,
    kind TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS collection_photos (
    collection INTEGER NOT NULL REFERENCES collections(id),
    photo INTEGER NOT NULL REFERENCES photos(id),
    position TEXT,
    PRIMARY KEY(collection,photo)
);

CREATE TABLE IF NOT EXISTS keywords (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    parent INTEGER
);

CREATE TABLE IF NOT EXISTS photo_keywords (
    photo INTEGER NOT NULL REFERENCES photos(id),
    keyword INTEGER NOT NULL REFERENCES keywords(id),
    PRIMARY KEY(photo,keyword)
);

CREATE TABLE IF NOT EXISTS folder_mappings (
    folder INTEGER PRIMARY KEY REFERENCES folders(id),
    path TEXT NOT NULL
);

-- Added after version 1 shipped.

-- Lightroom's develop history per photo: one full settings snapshot per step.
CREATE TABLE IF NOT EXISTS lightroom_history (
    photo INTEGER NOT NULL,
    position INTEGER NOT NULL,
    name TEXT NOT NULL DEFAULT '',
    created REAL,
    text TEXT NOT NULL,
    PRIMARY KEY(photo, position)
);

-- Spots and masks of a photo's edit (experimental), as `LocalEdits` JSON: kept out
-- of the recipe column so releases before them still read every edit.
CREATE TABLE IF NOT EXISTS local_edits (
    photo INTEGER PRIMARY KEY,
    data TEXT NOT NULL
);

-- A photo's Develop History (see `develop_history`), saved with its edit.
CREATE TABLE IF NOT EXISTS develop_history (
    photo INTEGER PRIMARY KEY REFERENCES photos(id) ON DELETE CASCADE,
    data BLOB NOT NULL
);

-- Develop Snapshots (see `snapshots`): a RAWmakase recipe as JSON, or Lightroom's
-- settings text for snapshots imported from Lightroom.
CREATE TABLE IF NOT EXISTS develop_snapshots (
    id INTEGER PRIMARY KEY,
    photo INTEGER NOT NULL REFERENCES photos(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    recipe TEXT,
    lightroom BLOB
);
CREATE INDEX IF NOT EXISTS develop_snapshots_photo ON develop_snapshots(photo);

-- Compressed bitmaps referenced by hash from saved recipes (see `storage::bitmaps`).
CREATE TABLE IF NOT EXISTS bitmaps (
    hash TEXT PRIMARY KEY,
    data BLOB NOT NULL
);

-- Camera settings and size of a photo, from the Lightroom catalog it came from
-- or read from its file; a row of NULLs records a file that had none.
CREATE TABLE IF NOT EXISTS photo_info (
    photo INTEGER PRIMARY KEY,
    camera TEXT,
    lens TEXT,
    focal REAL,
    aperture REAL,
    exposure REAL,
    iso REAL,
    width INTEGER,
    height INTEGER
);

-- Facts about the catalog itself, by name.
CREATE TABLE IF NOT EXISTS meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- Descriptive metadata set in RAWmakase or imported, per photo: a virtual copy
-- keeps its own rows, never its master's. A field with no row is the file's
-- own. Rows go with their photo, also when a release that does not know these
-- tables removes it, so a later photo reusing the id inherits nothing.

-- Whether a field (title, caption, creator, copyright) is set or cleared.
CREATE TABLE IF NOT EXISTS photo_fields (
    photo INTEGER NOT NULL REFERENCES photos(id) ON DELETE CASCADE,
    field TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('set', 'cleared')),
    PRIMARY KEY(photo, field)
);

-- The languages of a set title, caption or copyright: "x-default" and any
-- others imported, in their order (the first is the default when there is
-- no "x-default").
CREATE TABLE IF NOT EXISTS photo_text (
    photo INTEGER NOT NULL REFERENCES photos(id) ON DELETE CASCADE,
    field TEXT NOT NULL,
    lang TEXT NOT NULL,
    position INTEGER NOT NULL,
    value TEXT NOT NULL,
    PRIMARY KEY(photo, field, lang)
);

-- A set creator's names, in order.
CREATE TABLE IF NOT EXISTS photo_creators (
    photo INTEGER NOT NULL REFERENCES photos(id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    name TEXT NOT NULL,
    PRIMARY KEY(photo, position)
);

-- A capture time from elsewhere than the file (a Lightroom catalog, a
-- sidecar): local "YYYY-MM-DDTHH:MM:SS", the subsecond digits as read, and
-- the offset ("+02:00") when known. `photos.captured` is the sort key.
CREATE TABLE IF NOT EXISTS photo_capture (
    photo INTEGER PRIMARY KEY REFERENCES photos(id) ON DELETE CASCADE,
    captured TEXT NOT NULL,
    subsec TEXT,
    "offset" TEXT
);

-- A location from elsewhere than the file, or the file's cleared.
CREATE TABLE IF NOT EXISTS photo_location (
    photo INTEGER PRIMARY KEY REFERENCES photos(id) ON DELETE CASCADE,
    lat REAL,
    lon REAL,
    alt REAL,
    cleared INTEGER NOT NULL DEFAULT 0
);

-- Keywords are looked up by parent and name; not unique, as Lightroom
-- catalogs can hold duplicates.
CREATE INDEX IF NOT EXISTS keywords_parent_name ON keywords(parent, name);

-- Lightroom's export options of a keyword, where one is off: Include on
-- Export (`include`) and Export Containing Keywords (`parents`). A keyword
-- with no row exports with its parents.
CREATE TABLE IF NOT EXISTS keyword_export (
    keyword INTEGER PRIMARY KEY REFERENCES keywords(id) ON DELETE CASCADE,
    include INTEGER NOT NULL,
    parents INTEGER NOT NULL
);

-- Where each computer finds the catalog's folders (see `locations`). The
-- legacy `roots.mapped_path` and `folder_mappings` stay as releases before
-- these tables read them, and are still written.

-- A computer that opened the catalog: a random id made once per install,
-- and a name to show. `adopted_at` records when it copied the legacy
-- mappings into its own rows; from then on they no longer apply to it.
CREATE TABLE IF NOT EXISTS computers (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    adopted_at TEXT
);

-- A folder's logical path in its root: names joined by '/'. No system allows
-- '/' in a name, so a '\' here is always part of one.
CREATE TABLE IF NOT EXISTS folder_paths (
    folder INTEGER PRIMARY KEY REFERENCES folders(id) ON DELETE CASCADE,
    path TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS folder_paths_path ON folder_paths(path);

-- Where a root ('') or a folder and its subfolders (a logical path) are on
-- one computer.
CREATE TABLE IF NOT EXISTS folder_locations (
    root INTEGER NOT NULL REFERENCES roots(id) ON DELETE CASCADE,
    relative_path TEXT NOT NULL,
    computer TEXT NOT NULL REFERENCES computers(id) ON DELETE CASCADE,
    path TEXT NOT NULL,
    PRIMARY KEY(root, relative_path, computer)
);
