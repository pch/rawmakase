//! Importing a Lightroom library (Lightroom, not Classic) into a new catalog:
//! every photo of it, with its edit, rating, pick or reject, and its albums
//! as collections. The library is read from a copy of each account's catalog
//! taken while Lightroom has it closed (`library_db`), and its documents
//! through `lr_library`.
//!
//! Lightroom keeps on this computer only the originals it was asked to; the
//! rest are in Adobe's cloud. An original is looked for by its name and size,
//! first in the folders given (where the photos are kept, a NAS say) and then
//! in the library's own `originals`, where a second photo of a day and name
//! is kept as `name-2`; when several files have the name and size, by its
//! SHA-256. The files found are added as Add Folder adds them, so they arrive
//! with everything a photo added here has.
//!
//! A photo whose original isn't found is added where Lightroom will put it
//! when it downloads it (`lr_library::original_path`), so it is complete as
//! soon as its file is there; until then it is in Adobe's cloud, shown from
//! Lightroom's own preview of it where the library has one
//! ([`Catalog::settle_lightroom_downloads`] follows the files arriving). It
//! has no defaults for new photos: those are decided from the camera's EXIF
//! in its file. Only a photo without a capture date, or with a name that
//! isn't a file name, is left out.
use super::library_db;
use crate::catalog::db::{Reads, Write, sql, sqlite_sql};
use crate::catalog::ingest::{ImportKind, folder_id, walk};
use crate::catalog::value::{TextOrBlob, row};
use crate::catalog::{Catalog, CollectionId, PhotoId, RootId};
use crate::lr_library::{self, Album, Asset, Document};
use anyhow::{Context, Result, ensure};
use std::collections::{HashMap, HashSet};
use std::io::Read as _;
use std::path::{Path, PathBuf};

const LIGHTROOM_OPEN: &str = "Lightroom has its library open. Quit Lightroom, then import again";

/// The longest side of Lightroom's previews that are taken for a photo in
/// the cloud: the largest it has up to this, for a 640-pixel thumbnail.
const PREVIEW_SIDE: i64 = 2048;

/// What importing a library did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LibraryImport {
    pub catalog: PathBuf,
    /// The library's photos.
    pub photos: usize,
    /// Photos added with their original on this computer, and its bytes.
    pub added: usize,
    pub added_bytes: u64,
    /// Photos added in Adobe's cloud, to be complete when Lightroom
    /// downloads their originals, and their originals' bytes.
    pub cloud: usize,
    pub cloud_bytes: u64,
    /// Of `cloud`, photos with files of their name and size here of which
    /// none could be told to be theirs: several, and no SHA-256 to choose by,
    /// or one another photo's original too.
    pub ambiguous: usize,
    /// Photos left out: no capture date, or a name that isn't a file name,
    /// so Lightroom's place for their original can't be known (or a hundred
    /// photos of its day and name before it).
    pub left_out: usize,
    /// Photos that have an edit, and of those the ones whose edit's XMP
    /// isn't in the library yet: Lightroom downloads it with the original,
    /// and it is read then ([`Catalog::settle_lightroom_downloads`]).
    pub edits: usize,
    pub edits_missing: usize,
    pub albums: usize,
    /// Documents that couldn't be read, left out.
    pub unreadable: usize,
}

impl LibraryImport {
    /// One line: "Imported 9 photos · 4 on this computer (12 MB) · 5 in
    /// Adobe's cloud (1.2 GB) · …".
    pub fn message(&self) -> String {
        let plural =
            |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
        let mut parts = vec![format!(
            "Imported {}",
            plural(self.added + self.cloud, "photo", "photos")
        )];
        parts.push(format!(
            "{} on this computer ({})",
            self.added,
            bytes(self.added_bytes)
        ));
        if self.cloud > 0 {
            parts.push(format!(
                "{} in Adobe's cloud ({}): Download from Lightroom… in the catalog menu brings them here",
                self.cloud,
                bytes(self.cloud_bytes)
            ));
        }
        if self.ambiguous > 0 {
            parts.push(format!(
                "{} of them have files here that can't be told apart",
                self.ambiguous
            ));
        }
        if self.left_out > 0 {
            parts.push(format!(
                "{} left out: where Lightroom keeps their originals isn't known",
                self.left_out
            ));
        }
        if self.edits > 0 {
            parts.push(plural(self.edits, "edit", "edits"));
        }
        if self.edits_missing > 0 {
            parts.push(format!(
                "{} of them to come with their originals",
                self.edits_missing
            ));
        }
        if self.albums > 0 {
            parts.push(plural(self.albums, "album", "albums"));
        }
        if self.unreadable > 0 {
            parts.push(format!("{} unreadable records skipped", self.unreadable));
        }
        parts.join(" · ")
    }
}

/// `n` bytes in decimal units, as a file browser shows them.
fn bytes(n: u64) -> String {
    let n = n as f64;
    match n {
        n if n >= 1e12 => format!("{:.1} TB", n / 1e12),
        n if n >= 1e9 => format!("{:.1} GB", n / 1e9),
        n if n >= 1e6 => format!("{:.0} MB", n / 1e6),
        n => format!("{:.0} KB", n / 1e3),
    }
}

row! {
    struct DocRow {
        local: i64,
        id: String,
        kind: String,
        subtype: String,
        content: TextOrBlob,
    }
}

row! {
    struct PreviewRow {
        local: f64,
        side: f64,
        path: String,
    }
}

/// The documents of one account, with the folder they came from.
struct Account {
    folder: PathBuf,
    assets: Vec<Asset>,
    albums: Vec<Album>,
    members: Vec<(String, String)>,
    /// Each asset's id by its row in the catalog, which Lightroom's
    /// previews name it by.
    locals: HashMap<i64, String>,
    /// Lightroom's preview of each asset, by its id.
    previews: HashMap<String, PathBuf>,
}

/// Imports the Lightroom library at `library` into a new catalog published
/// at `destination`, looking for originals in `folders` and then in the
/// library. Nothing is published when it fails, and the library is only
/// read.
pub fn import_lightroom_library(
    library: &Path,
    folders: &[PathBuf],
    destination: &Path,
    defaults: &crate::catalog::NewPhotoDefaults,
) -> Result<LibraryImport> {
    ensure!(
        !destination.exists(),
        "Destination exists; choose a new catalog filename"
    );
    let accounts = lr_library::accounts(library);
    ensure!(
        !accounts.is_empty(),
        "Not a Lightroom library: no {} in it",
        lr_library::CATALOG
    );
    let parent = crate::storage::parent_dir(destination);
    std::fs::create_dir_all(parent)?;
    let tmpdir = tempfile::tempdir_in(parent)?;
    let working = tmpdir.path().join("import.rawmakase");
    let mut catalog = Catalog::create(&working)?;
    let mut report = LibraryImport::default();
    let mut size = 0;
    let mut read = Vec::new();
    for folder in &accounts {
        let snapshot = library_db::snapshot(&folder.join(lr_library::CATALOG), LIGHTROOM_OPEN)?;
        size += snapshot.bytes;
        let mut account = read_account(&mut catalog, folder, &snapshot.path, &mut report)?;
        account.previews = read_previews(&mut catalog, folder, &account)?;
        read.push(account);
    }
    let roots = search_roots(&read, folders)?;
    let found = find_originals(&read, &roots)?;
    add_photos(&mut catalog, &roots, &found.files, defaults)?;
    let ids: HashMap<PathBuf, PhotoId> = catalog
        .photos()?
        .into_iter()
        .map(|p| (p.path, p.id))
        .collect();
    catalog.db.write(|w| {
        w.execute(
            sql!("INSERT INTO sources(path,original_size,original_catalog) VALUES(?,?,?)"),
            &[
                &library.to_string_lossy(),
                &(size as i64),
                &Vec::<u8>::new(),
            ],
        )?;
        let mut photos: HashMap<(usize, &str), PhotoId> = HashMap::new();
        for file in &found.files {
            let Some(&photo) = ids.get(&file.file) else {
                continue;
            };
            photos.insert((file.account, file.asset.id.as_str()), photo);
            report.added += 1;
            report.added_bytes += file.asset.file_size;
        }
        let cloud = add_cloud_photos(w, &read, &roots, &found, &mut report)?;
        photos.extend(cloud.iter().map(|(k, (id, _))| (*k, *id)));
        for (i, account) in read.iter().enumerate() {
            for asset in &account.assets {
                let Some(&photo) = photos.get(&(i, asset.id.as_str())) else {
                    continue;
                };
                let edit_path = asset
                    .edit
                    .as_ref()
                    .and_then(|sha| lr_library::edit_path(&account.folder, sha));
                let develop = edit_path.as_deref().and_then(read_edit);
                if asset.edit.is_some() {
                    report.edits += 1;
                }
                // Not in the library yet: read once Lightroom has downloaded it.
                let pending = edit_path
                    .filter(|_| develop.is_none())
                    .map(|p| p.to_string_lossy().into_owned());
                if pending.is_some() {
                    report.edits_missing += 1;
                }
                w.execute(
                    sql!("UPDATE photos SET rating=?, flag=?, lightroom_develop=? WHERE id=?"),
                    &[&asset.rating, &asset.flag, &develop, &photo],
                )?;
                let in_cloud = cloud.contains_key(&(i, asset.id.as_str()));
                let preview = in_cloud
                    .then(|| account.previews.get(&asset.id))
                    .flatten()
                    .map(|p| p.to_string_lossy().into_owned());
                w.execute(
                    sql!(
                        "INSERT INTO lightroom_library_photos(photo,asset,size,cloud,preview,edit)
                         VALUES(?,?,?,?,?,?)"
                    ),
                    &[
                        &photo,
                        &asset.id,
                        &(asset.file_size as i64),
                        &in_cloud,
                        &preview,
                        &pending,
                    ],
                )?;
            }
        }
        for (i, account) in read.iter().enumerate() {
            let collections = insert_albums(w, &account.albums)?;
            report.albums += collections.len();
            for (album, asset) in &account.members {
                if let (Some(collection), Some(photo)) =
                    (collections.get(album), photos.get(&(i, asset.as_str())))
                {
                    w.execute(
                        sql!(
                            "INSERT INTO collection_photos(collection,photo) VALUES(?,?)
                             ON CONFLICT DO NOTHING"
                        ),
                        &[collection, photo],
                    )?;
                }
            }
        }
        Ok(())
    })?;
    ensure!(
        catalog.db.is_intact()?,
        "Imported catalog failed integrity check"
    );
    drop(catalog);
    crate::storage::rename_noclobber(&working, destination)
        .context("Publish imported catalog without overwriting")?;
    report.catalog = destination.to_path_buf();
    Ok(report)
}

/// The documents of the account in `folder`, from the copy of its catalog at
/// `snapshot`, which is checked first.
fn read_account(
    catalog: &mut Catalog,
    folder: &Path,
    snapshot: &Path,
    report: &mut LibraryImport,
) -> Result<Account> {
    super::check_intact(
        snapshot,
        "The Lightroom library's catalog failed SQLite's integrity check",
    )?;
    let rows: Vec<DocRow> = catalog.db.with_lightroom(snapshot, |lr| {
        ensure!(
            lr.has_table("docs")? && lr.has_table("revs")?,
            "Not a supported Lightroom library: its catalog has no documents"
        );
        lr.read_sqlite(
            sqlite_sql!(
                "SELECT d.localDocId, d.fullDocId, COALESCE(d.type,''), COALESCE(d.subtype,''), r.content
                 FROM lr.docs d JOIN lr.revs r ON r.sequence=d.winningRevSequence
                 WHERE d.deleted=0 AND r.deleted=0 AND d.type IN('asset','album','album_asset')"
            ),
            &[],
        )
    })?;
    let mut account = Account {
        folder: folder.to_path_buf(),
        assets: Vec::new(),
        albums: Vec::new(),
        members: Vec::new(),
        locals: HashMap::new(),
        previews: HashMap::new(),
    };
    for row in rows {
        let Some(content) = row.content.0 else {
            report.unreadable += 1;
            continue;
        };
        match lr_library::document(&row.kind, &row.subtype, &row.id, &content) {
            Ok(Some(Document::Asset(asset))) => {
                account.locals.insert(row.local, asset.id.clone());
                account.assets.push(asset);
            }
            Ok(Some(Document::Album(album))) => account.albums.push(album),
            Ok(Some(Document::AlbumAsset(m))) => account.members.push((m.album, m.asset)),
            Ok(None) => {}
            Err(_) => report.unreadable += 1,
        }
    }
    report.photos += account.assets.len();
    Ok(account)
}

/// Lightroom's largest preview up to [`PREVIEW_SIDE`] of each asset of
/// `account`, from the account's `previews.db`: JPEGs with the photo's edit,
/// in the account's `previews` folder. A preview named anywhere else, or not
/// there, is passed over; a library without them has none.
fn read_previews(
    catalog: &mut Catalog,
    folder: &Path,
    account: &Account,
) -> Result<HashMap<String, PathBuf>> {
    let index = folder.join("previews.db");
    let Ok(previews_dir) = folder.join("previews").canonicalize() else {
        return Ok(HashMap::new());
    };
    if !index.is_file() {
        return Ok(HashMap::new());
    }
    let snapshot = library_db::snapshot(&index, LIGHTROOM_OPEN)?;
    let rows: Vec<PreviewRow> = catalog.db.with_lightroom(&snapshot.path, |lr| {
        if !lr.has_table("RenditionPath")? {
            return Ok(Vec::new());
        }
        lr.read_sqlite(
            sqlite_sql!("SELECT localAssetId, longSide, path FROM lr.RenditionPath"),
            &[],
        )
    })?;
    let mut best: HashMap<String, (i64, PathBuf)> = HashMap::new();
    for row in rows {
        // Lightroom keeps both numbers as REAL.
        let (local, side) = (row.local as i64, row.side as i64);
        let Some(id) = account.locals.get(&local) else {
            continue;
        };
        if side > PREVIEW_SIDE || best.get(id).is_some_and(|(s, _)| *s >= side) {
            continue;
        }
        let Ok(path) = Path::new(&row.path).canonicalize() else {
            continue;
        };
        if path.starts_with(&previews_dir) && path.is_file() {
            best.insert(id.clone(), (side, path));
        }
    }
    Ok(best.into_iter().map(|(id, (_, path))| (id, path)).collect())
}

/// A photo's original found: the account and asset it is of, the file, its
/// path canonical as the catalog keeps it, and the folder it was found below.
struct Found<'a> {
    account: usize,
    asset: &'a Asset,
    file: PathBuf,
    root: usize,
}

/// What [`find_originals`] found: the originals, and the photos that had
/// files of their name and size here, none provably theirs.
struct Originals<'a> {
    files: Vec<Found<'a>>,
    ambiguous: HashSet<(usize, &'a str)>,
}

/// The folders originals are looked for in, canonical: `folders` in the order
/// given, then each account's `originals`. One inside another is left out, as
/// it is looked in with it.
fn search_roots(accounts: &[Account], folders: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut roots: Vec<PathBuf> = folders
        .iter()
        .map(|f| {
            f.canonicalize()
                .with_context(|| format!("{} isn't there", f.display()))
        })
        .collect::<Result<_>>()?;
    roots.extend(
        accounts
            .iter()
            .filter_map(|a| a.folder.join("originals").canonicalize().ok()),
    );
    let mut kept: Vec<PathBuf> = Vec::new();
    for root in roots {
        if !kept.contains(&root) {
            kept.push(root);
        }
    }
    let all = kept.clone();
    kept.retain(|r| !all.iter().any(|o| o != r && r.starts_with(o)));
    Ok(kept)
}

/// `name` without the `-2` Lightroom adds to a second original of a day and
/// name in its `originals`, lowercased; None when it has none.
fn numbered_as(name: &str) -> Option<String> {
    let (stem, ext) = name.rsplit_once('.')?;
    let (base, n) = stem.rsplit_once('-')?;
    (!base.is_empty() && !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        .then(|| format!("{base}.{ext}").to_lowercase())
}

/// Each photo's original that is found below one of `roots`.
fn find_originals<'a>(accounts: &'a [Account], roots: &[PathBuf]) -> Result<Originals<'a>> {
    let originals: Vec<PathBuf> = accounts
        .iter()
        .filter_map(|a| a.folder.join("originals").canonicalize().ok())
        .collect();
    let mut files: HashMap<(String, u64), Vec<(PathBuf, usize)>> = HashMap::new();
    for (root, dir) in roots.iter().enumerate() {
        for file in walk(dir)? {
            let Ok(len) = file.metadata().map(|m| m.len()) else {
                continue;
            };
            let name = file.file_name().unwrap_or_default().to_string_lossy();
            let mut names = vec![name.to_lowercase()];
            if originals.iter().any(|o| file.starts_with(o)) {
                names.extend(numbered_as(&name));
            }
            for name in names {
                files
                    .entry((name, len))
                    .or_default()
                    .push((file.clone(), root));
            }
        }
    }
    let key = |asset: &Asset| (asset.file_name.to_lowercase(), asset.file_size);
    // Photos of one name and size, as files numbered again after 9999 can be:
    // which file is which takes their SHA-256 too.
    let mut claims: HashMap<(String, u64), usize> = HashMap::new();
    for asset in accounts.iter().flat_map(|a| &a.assets) {
        *claims.entry(key(asset)).or_default() += 1;
    }
    let mut taken = HashSet::new();
    let mut found = Originals {
        files: Vec::new(),
        ambiguous: HashSet::new(),
    };
    for (account, a) in accounts.iter().enumerate() {
        for asset in &a.assets {
            let key = key(asset);
            let candidates = files.get(&key).map(Vec::as_slice).unwrap_or_default();
            let chosen = match candidates {
                [] => continue,
                [only] if claims[&key] == 1 => Some(only.clone()),
                // Several files of the name and size, or several photos:
                // those with its SHA-256 are copies of the original, and the
                // first found is taken.
                many => many
                    .iter()
                    .filter(|(f, _)| {
                        !asset.sha256.is_empty() && sha256(f).is_ok_and(|h| h == asset.sha256)
                    })
                    .min_by_key(|(_, root)| *root)
                    .cloned(),
            };
            let Some((file, root)) = chosen else {
                // Without a SHA-256 none can be told to be its original; with
                // one that names none of them, its original isn't here.
                if asset.sha256.is_empty() {
                    found.ambiguous.insert((account, asset.id.as_str()));
                }
                continue;
            };
            // A file is one photo's original, never two.
            if !taken.insert(file.clone()) {
                found.ambiguous.insert((account, asset.id.as_str()));
                continue;
            }
            found.files.push(Found {
                account,
                asset,
                file,
                root,
            });
        }
    }
    Ok(found)
}

/// Adds the files of `found` as Add Folder adds them, each below the folder
/// of `roots` it was found in, which becomes its root.
fn add_photos(
    catalog: &mut Catalog,
    roots: &[PathBuf],
    found: &[Found<'_>],
    defaults: &crate::catalog::NewPhotoDefaults,
) -> Result<()> {
    for (i, root) in roots.iter().enumerate() {
        let files: Vec<PathBuf> = found
            .iter()
            .filter(|f| f.root == i)
            .map(|f| f.file.clone())
            .collect();
        if files.is_empty() {
            continue;
        }
        let placed = catalog.place(&files, root, &[])?;
        ensure!(
            placed.ambiguous.is_empty() && placed.conflicts.is_empty(),
            "{} can't be added as a folder of the new catalog",
            root.display()
        );
        catalog.insert(root, files, &placed.places, defaults, ImportKind::Add)?;
    }
    Ok(())
}

/// Adds each photo of `accounts` whose original wasn't found where
/// Lightroom will put it, in `w`'s transaction: below the root of `roots`
/// that holds that place, or the account's `originals` as a root of its own.
/// A place another photo has, or a file that isn't the photo's, takes the
/// next number Lightroom would give it (`name-2`). Returns each photo added,
/// by its account and asset, with its file.
fn add_cloud_photos<'a>(
    w: &mut Write<'_>,
    accounts: &'a [Account],
    roots: &[PathBuf],
    found: &Originals<'a>,
    report: &mut LibraryImport,
) -> Result<CloudAdded<'a>> {
    let local: HashSet<(usize, &str)> = found
        .files
        .iter()
        .map(|f| (f.account, f.asset.id.as_str()))
        .collect();
    let mut used: HashSet<PathBuf> = found.files.iter().map(|f| f.file.clone()).collect();
    let mut root_ids: HashMap<PathBuf, RootId> = HashMap::new();
    let mut folders: HashMap<(RootId, String), crate::catalog::FolderId> = HashMap::new();
    let mut added = HashMap::new();
    for (i, account) in accounts.iter().enumerate() {
        let folder = account
            .folder
            .canonicalize()
            .unwrap_or_else(|_| account.folder.clone());
        for asset in &account.assets {
            let key = (i, asset.id.as_str());
            if local.contains(&key) {
                continue;
            }
            let Some(first) = lr_library::original_path(&folder, asset) else {
                report.left_out += 1;
                continue;
            };
            let Some(file) = free_place(&first, &used) else {
                report.left_out += 1;
                continue;
            };
            used.insert(file.clone());
            let originals = folder.join("originals");
            let root = roots
                .iter()
                .find(|r| file.starts_with(r))
                .unwrap_or(&originals);
            let root_id = match root_ids.get(root) {
                Some(id) => *id,
                None => {
                    let id = root_id(w, root)?;
                    root_ids.insert(root.clone(), id);
                    id
                }
            };
            let logical = file
                .parent()
                .and_then(|p| p.strip_prefix(root).ok())
                .map(|p| {
                    p.components()
                        .map(|c| c.as_os_str().to_string_lossy())
                        .collect::<Vec<_>>()
                        .join("/")
                })
                .unwrap_or_default();
            let folder_key = (root_id, logical);
            let folder_id = match folders.get(&folder_key) {
                Some(id) => *id,
                None => {
                    let id = folder_id(w, folder_key.0, &folder_key.1)?;
                    folders.insert(folder_key, id);
                    id
                }
            };
            let name = file
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let format = file
                .extension()
                .unwrap_or_default()
                .to_string_lossy()
                .to_ascii_uppercase();
            // Lightroom's local time, without its zone, as a file's EXIF has it.
            let captured = asset.capture_date.get(..19).unwrap_or_default();
            let id: PhotoId = w.insert_returning_id(
                sql!(
                    "INSERT INTO photos(folder,filename,original_path,format,captured)
                     VALUES(?,?,?,?,?) RETURNING id"
                ),
                &[
                    &folder_id,
                    &name,
                    &file.to_string_lossy(),
                    &format,
                    &captured,
                ],
            )?;
            report.cloud += 1;
            report.cloud_bytes += asset.file_size;
            if found.ambiguous.contains(&key) {
                report.ambiguous += 1;
            }
            added.insert(key, (id, file));
        }
    }
    Ok(added)
}

/// Photos added in the cloud, by their account and asset, with their files.
type CloudAdded<'a> = HashMap<(usize, &'a str), (PhotoId, PathBuf)>;

/// `first`, or the next of `name-2`, `name-3`… that no photo has and no
/// file is at; None after 99.
fn free_place(first: &Path, used: &HashSet<PathBuf>) -> Option<PathBuf> {
    let free = |p: &Path| !used.contains(p) && !p.exists();
    if free(first) {
        return Some(first.to_path_buf());
    }
    let stem = first.file_stem()?.to_string_lossy();
    let ext = first
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    (2..100)
        .map(|n| first.with_file_name(format!("{stem}-{n}{ext}")))
        .find(|p| free(p))
}

/// The catalog's root added at `path`, made if there is none.
fn root_id(w: &mut Write<'_>, path: &Path) -> Result<RootId> {
    let path = path.to_string_lossy();
    let existing: Option<RootId> = w.read_one(
        sql!("SELECT min(id) FROM roots WHERE original_path=?"),
        &[&path],
    )?;
    match existing {
        Some(id) => Ok(id),
        None => w.insert_returning_id(
            sql!("INSERT INTO roots(original_path) VALUES(?) RETURNING id"),
            &[&path],
        ),
    }
}

/// The albums and their folders as collections, parents first; returns the
/// collection of each album. An album whose folder isn't there goes at the
/// top.
fn insert_albums(w: &mut Write<'_>, albums: &[Album]) -> Result<HashMap<String, CollectionId>> {
    let known: HashSet<&str> = albums.iter().map(|a| a.id.as_str()).collect();
    let mut made: HashMap<String, CollectionId> = HashMap::new();
    let mut left: Vec<&Album> = albums.iter().collect();
    while !left.is_empty() {
        let before = left.len();
        let mut waiting = Vec::new();
        for album in left {
            let parent = album.parent.as_deref().filter(|p| known.contains(p));
            let parent_id = match parent {
                Some(p) => match made.get(p) {
                    Some(id) => Some(*id),
                    None => {
                        waiting.push(album);
                        continue;
                    }
                },
                None => None,
            };
            let kind = if album.set {
                "com.adobe.ag.library.group"
            } else {
                "com.adobe.ag.library.collection"
            };
            let id: CollectionId = w.insert_returning_id(
                sql!("INSERT INTO collections(name,parent,kind) VALUES(?,?,?) RETURNING id"),
                &[&album.name, &parent_id, &kind],
            )?;
            made.insert(album.id.clone(), id);
        }
        // Folders that are each other's parents: the rest go at the top.
        if waiting.len() == before {
            for album in waiting {
                let id: CollectionId = w.insert_returning_id(
                    sql!("INSERT INTO collections(name,parent,kind) VALUES(?,NULL,?) RETURNING id"),
                    &[&album.name, &"com.adobe.ag.library.collection"],
                )?;
                made.insert(album.id.clone(), id);
            }
            break;
        }
        left = waiting;
    }
    Ok(made)
}

/// The edit's Camera Raw XMP, if it is there and is text.
pub(super) fn read_edit(path: &Path) -> Option<String> {
    let mut text = String::new();
    std::fs::File::open(path)
        .ok()?
        .take(8_000_000)
        .read_to_string(&mut text)
        .ok()?;
    Some(text)
}

fn sha256(path: &Path) -> Result<String> {
    use sha2::{Digest as _, Sha256};
    let mut file = std::fs::File::open(path)?;
    let mut sha = Sha256::new();
    let mut buffer = vec![0; 1 << 20];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        sha.update(&buffer[..n]);
    }
    Ok(sha.finalize().iter().map(|b| format!("{b:02x}")).collect())
}
