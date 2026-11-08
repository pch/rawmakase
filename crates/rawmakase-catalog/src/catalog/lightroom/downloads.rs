//! A catalog's photos still in Adobe's cloud, imported from a Lightroom
//! library (`library`): which they are and Lightroom's preview of each,
//! the library's albums to choose which to download, marking those in
//! Lightroom (`library_db`), and following the originals as Lightroom puts
//! them where the photos wait for them.
use super::library_db;
use crate::catalog::db::{Reads, sql, sqlite_sql};
use crate::catalog::value::{TextOrBlob, row};
use crate::catalog::{Catalog, PhotoId};
use crate::lr_library::{self, Document};
use anyhow::{Result, ensure};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

const LIGHTROOM_OPEN: &str =
    "Lightroom has its library open. Quit Lightroom, then choose the albums again";

/// A photo whose original is in Adobe's cloud.
#[derive(Clone, Debug, PartialEq)]
pub struct CloudPhoto {
    /// Its original's size, which the file Lightroom downloads has.
    pub size: u64,
    /// Lightroom's JPEG of it, with its edit, to show until the original is
    /// here.
    pub preview: Option<PathBuf>,
}

/// An album of the Lightroom library a catalog was imported from, or a folder
/// of albums (`set`), to choose which to download.
#[derive(Clone, Debug, PartialEq)]
pub struct LibraryAlbum {
    /// The account's folder in the library, whose catalog has it.
    pub account: PathBuf,
    pub id: String,
    pub name: String,
    pub parent: Option<String>,
    pub set: bool,
    /// Whether Lightroom keeps it on this computer (Store Album Locally).
    pub marked: bool,
    /// Its photos in this catalog, and of them those still in the cloud
    /// with their originals' bytes.
    pub photos: usize,
    pub in_cloud: usize,
    pub in_cloud_bytes: u64,
    /// The assets of its photos in the cloud with their originals' bytes,
    /// so photos in several albums are counted once.
    pub in_cloud_assets: Vec<(String, u64)>,
    /// Not an album: the account's photos in no album. Downloading them
    /// makes an album of those in the cloud ([`NEW_ALBUM`]), so Lightroom
    /// downloads them as it does an album's.
    pub loose: bool,
}

/// The name of the album made for photos in no album, to download them.
pub const NEW_ALBUM: &str = "Downloaded by RAWmakase";

/// What [`Catalog::settle_lightroom_downloads`] found.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Settled {
    /// Photos whose original is here now.
    pub arrived: Vec<PhotoId>,
    /// Photos whose Lightroom edit was read, now that Lightroom has it.
    pub edits: Vec<PhotoId>,
    /// Photos still in the cloud.
    pub waiting: usize,
}

row! {
    struct CloudRow {
        photo: PhotoId,
        size: i64,
        preview: Option<String>,
    }
}

row! {
    struct PendingRow {
        photo: PhotoId,
        edit: String,
    }
}

row! {
    struct AlbumDocRow {
        id: String,
        subtype: String,
        content: TextOrBlob,
        annotation: TextOrBlob,
    }
}

row! {
    struct AssetRow {
        asset: String,
        size: i64,
        cloud: bool,
    }
}

row! {
    struct MemberRow {
        id: String,
        content: TextOrBlob,
    }
}

impl Catalog {
    /// The photos whose originals are in Adobe's cloud, each master with
    /// its original's size and Lightroom's preview of it. A virtual copy is
    /// its master's.
    pub fn lightroom_cloud_photos(&self) -> Result<HashMap<PhotoId, CloudPhoto>> {
        let rows: Vec<CloudRow> = self.db.read(
            sql!("SELECT photo, size, preview FROM lightroom_library_photos WHERE cloud=1"),
            &[],
        )?;
        Ok(rows
            .into_iter()
            .map(|r| {
                (
                    r.photo,
                    CloudPhoto {
                        size: u64::try_from(r.size).unwrap_or(0),
                        preview: r.preview.map(PathBuf::from),
                    },
                )
            })
            .collect())
    }

    /// Takes each photo in the cloud whose file is here now with its
    /// original's size for one whose original has arrived. Lightroom gives
    /// the second photo of a day and name `name-2` in the order it downloads
    /// them, so where such photos' files arrived in another order (each has
    /// another's size), they change names first.
    ///
    /// It also reads the Lightroom edits that weren't in the library when
    /// the photos were imported and are now: Lightroom downloads a photo's
    /// edit with its original. An edit made here comes first all the same.
    pub fn settle_lightroom_downloads(&mut self) -> Result<Settled> {
        let edits = self.read_pending_edits()?;
        let cloud = self.lightroom_cloud_photos()?;
        if cloud.is_empty() {
            return Ok(Settled {
                edits,
                ..Settled::default()
            });
        }
        let photos: HashMap<PhotoId, (PathBuf, crate::catalog::FolderId, String)> = self
            .photos()?
            .into_iter()
            .filter(|p| cloud.contains_key(&p.id))
            .map(|p| (p.id, (p.path, p.folder, p.filename)))
            .collect();
        let size_of = |path: &Path| std::fs::metadata(path).ok().map(|m| m.len());
        let mut arrived = Vec::new();
        let mut wrong: Vec<(PhotoId, u64)> = Vec::new();
        for (id, (path, _, _)) in &photos {
            if path.as_os_str().is_empty() {
                continue;
            }
            match size_of(path) {
                Some(size) if size == cloud[id].size => arrived.push(*id),
                Some(size) => wrong.push((*id, size)),
                None => {}
            }
        }
        // Files that arrived under another photo's name: within a folder,
        // each such file goes to the one photo there of its size, when those
        // photos take each other's files and no more (two, or three Lightroom
        // numbered in another order).
        // The photo whose original a file of that size is: the only one of
        // the folder's in the cloud, those whose files aren't there yet
        // included, so two of one size are never told apart by guessing.
        let takes = |a: PhotoId, file: u64| {
            let mut of_size = photos
                .iter()
                .filter(|(b, (_, folder, _))| *folder == photos[&a].1 && cloud[*b].size == file);
            match (of_size.next(), of_size.next()) {
                (Some((b, _)), None) => Some(*b),
                _ => None,
            }
        };
        let mut moves: HashMap<PhotoId, PhotoId> = wrong
            .iter()
            .filter_map(|&(a, file)| takes(a, file).map(|b| (a, b)))
            .collect();
        // Only those whose file goes to one of them, each to its own.
        loop {
            let mut targets: HashMap<PhotoId, usize> = HashMap::new();
            for b in moves.values() {
                *targets.entry(*b).or_default() += 1;
            }
            let before = moves.len();
            moves.retain(|_, b| targets[b] == 1);
            let sources: HashSet<PhotoId> = moves.keys().copied().collect();
            moves.retain(|_, b| sources.contains(b));
            if moves.len() == before {
                break;
            }
        }
        arrived.extend(moves.keys().copied());
        self.db.write(|w| {
            for (&from, &to) in &moves {
                // `to` takes the file at `from`'s name, its virtual copies too.
                let (name, path) = (&photos[&from].2, photos[&from].0.to_string_lossy());
                w.execute(
                    sql!(
                        "UPDATE photos SET filename=?1, original_path=?2
                         WHERE id=?3 OR master_id=?3"
                    ),
                    &[name, &path, &to],
                )?;
            }
            for id in &arrived {
                w.execute(
                    sql!("UPDATE lightroom_library_photos SET cloud=0 WHERE photo=?"),
                    &[id],
                )?;
            }
            Ok(())
        })?;
        Ok(Settled {
            waiting: cloud.len() - arrived.len(),
            arrived,
            edits,
        })
    }

    /// Where the Lightroom edits not read yet will be: photos imported before
    /// their edit's XMP was in the library (see
    /// [`Self::settle_lightroom_downloads`]).
    pub fn lightroom_pending_edits(&self) -> Result<Vec<PathBuf>> {
        let pending: Vec<PendingRow> = self.db.read(
            sql!(
                "SELECT l.photo, l.edit FROM lightroom_library_photos l
                 JOIN photos p ON p.id=l.photo
                 WHERE l.edit IS NOT NULL AND p.lightroom_develop IS NULL"
            ),
            &[],
        )?;
        Ok(pending.into_iter().map(|r| PathBuf::from(r.edit)).collect())
    }

    /// Reads the Lightroom edits of photos imported before their edit's XMP
    /// was in the library, those that are there now.
    fn read_pending_edits(&mut self) -> Result<Vec<PhotoId>> {
        let pending: Vec<PendingRow> = self.db.read(
            sql!(
                "SELECT l.photo, l.edit FROM lightroom_library_photos l
                 JOIN photos p ON p.id=l.photo
                 WHERE l.edit IS NOT NULL AND p.lightroom_develop IS NULL"
            ),
            &[],
        )?;
        let read: Vec<(PhotoId, String)> = pending
            .into_iter()
            .filter_map(|r| super::library::read_edit(Path::new(&r.edit)).map(|t| (r.photo, t)))
            .collect();
        if read.is_empty() {
            return Ok(Vec::new());
        }
        self.db.write(|w| {
            for (photo, text) in &read {
                w.execute(
                    sql!(
                        "UPDATE photos SET lightroom_develop=?
                         WHERE id=? AND lightroom_develop IS NULL"
                    ),
                    &[text, photo],
                )?;
                w.execute(
                    sql!("UPDATE lightroom_library_photos SET edit=NULL WHERE photo=?"),
                    &[photo],
                )?;
            }
            Ok(())
        })?;
        Ok(read.into_iter().map(|(photo, _)| photo).collect())
    }

    /// The Lightroom libraries this catalog's photos were imported from.
    fn lightroom_libraries(&self) -> Result<Vec<PathBuf>> {
        Ok(self
            .lightroom_sources()?
            .into_iter()
            .filter(|p| !lr_library::accounts(p).is_empty())
            .collect())
    }

    /// The albums of the Lightroom libraries this catalog was imported from,
    /// read while Lightroom has them closed, with what of each is still in
    /// the cloud here.
    pub fn lightroom_library_albums(&mut self) -> Result<Vec<LibraryAlbum>> {
        let libraries = self.lightroom_libraries()?;
        ensure!(
            !libraries.is_empty(),
            "This catalog wasn't imported from a Lightroom library"
        );
        let rows: Vec<AssetRow> = self.db.read(
            sql!("SELECT asset, size, cloud FROM lightroom_library_photos"),
            &[],
        )?;
        let by_asset: HashMap<String, (u64, bool)> = rows
            .into_iter()
            .map(|r| (r.asset, (u64::try_from(r.size).unwrap_or(0), r.cloud)))
            .collect();
        let mut albums = Vec::new();
        for account in libraries.iter().flat_map(|l| lr_library::accounts(l)) {
            let snapshot =
                library_db::snapshot(&account.join(lr_library::CATALOG), LIGHTROOM_OPEN)?;
            let (docs, members, assets): (Vec<AlbumDocRow>, Vec<MemberRow>, Vec<String>) =
                self.db.with_lightroom(&snapshot.path, |lr| {
                    let docs = lr.read_sqlite(
                        sqlite_sql!(
                            "SELECT d.fullDocId, COALESCE(d.subtype,''), r.content, d.annotation
                             FROM lr.docs d JOIN lr.revs r ON r.sequence=d.winningRevSequence
                             WHERE d.deleted=0 AND r.deleted=0 AND d.type='album'"
                        ),
                        &[],
                    )?;
                    let members = lr.read_sqlite(
                        sqlite_sql!(
                            "SELECT d.fullDocId, r.content
                             FROM lr.docs d JOIN lr.revs r ON r.sequence=d.winningRevSequence
                             WHERE d.deleted=0 AND r.deleted=0 AND d.type='album_asset'"
                        ),
                        &[],
                    )?;
                    let assets = lr.read_sqlite(
                        sqlite_sql!(
                            "SELECT fullDocId FROM lr.docs
                             WHERE deleted=0 AND type='asset' AND subtype='image'"
                        ),
                        &[],
                    )?;
                    Ok((docs, members, assets))
                })?;
            let mut photos: HashMap<String, Vec<String>> = HashMap::new();
            for m in members {
                if let Some(content) = m.content.0
                    && let Ok(Some(Document::AlbumAsset(m))) =
                        lr_library::document("album_asset", "", &m.id, &content)
                {
                    photos.entry(m.album).or_default().push(m.asset);
                }
            }
            for doc in docs {
                let Some(content) = doc.content.0 else {
                    continue;
                };
                let Ok(Some(Document::Album(album))) =
                    lr_library::document("album", &doc.subtype, &doc.id, &content)
                else {
                    continue;
                };
                let mut entry = LibraryAlbum {
                    account: account.clone(),
                    marked: doc
                        .annotation
                        .0
                        .is_some_and(|a| lr_library::marked_for_download(&a)),
                    id: album.id,
                    name: album.name,
                    parent: album.parent,
                    set: album.set,
                    photos: 0,
                    in_cloud: 0,
                    in_cloud_bytes: 0,
                    in_cloud_assets: Vec::new(),
                    loose: false,
                };
                for asset in photos.get(&entry.id).into_iter().flatten() {
                    if let Some(&(size, cloud)) = by_asset.get(asset) {
                        entry.photos += 1;
                        if cloud {
                            entry.in_cloud += 1;
                            entry.in_cloud_bytes += size;
                            entry.in_cloud_assets.push((asset.clone(), size));
                        }
                    }
                }
                albums.push(entry);
            }
            // The account's photos in no album, as one more to choose: in no
            // album of the account's, as an album's photos are counted above.
            let listed: HashSet<&str> = albums
                .iter()
                .filter(|a| a.account == account && !a.set)
                .map(|a| a.id.as_str())
                .collect();
            let in_albums: HashSet<&String> = photos
                .iter()
                .filter(|(album, _)| listed.contains(album.as_str()))
                .flat_map(|(_, assets)| assets)
                .collect();
            let mut loose = LibraryAlbum {
                account: account.clone(),
                id: String::new(),
                name: "Photos in no album".into(),
                parent: None,
                set: false,
                marked: false,
                photos: 0,
                in_cloud: 0,
                in_cloud_bytes: 0,
                in_cloud_assets: Vec::new(),
                loose: true,
            };
            for asset in assets.iter().filter(|a| !in_albums.contains(a)) {
                if let Some(&(size, cloud)) = by_asset.get(asset) {
                    loose.photos += 1;
                    if cloud {
                        loose.in_cloud += 1;
                        loose.in_cloud_bytes += size;
                        loose.in_cloud_assets.push((asset.clone(), size));
                    }
                }
            }
            if loose.photos > 0 {
                albums.push(loose);
            }
        }
        Ok(albums)
    }
}

/// Marks `albums` (of [`Catalog::lightroom_library_albums`]) Store Album
/// Locally in their libraries, so Lightroom downloads their originals when
/// it next runs; the photos of a `loose` one, in no album, are put in album
/// [`NEW_ALBUM`] (made the first time) marked the same way; a copy of each account's catalog as it was is written to
/// `backups` first. Refused while Lightroom has a library open. Albums of one
/// account are marked together or not at all.
pub fn mark_albums_for_download(
    albums: &[LibraryAlbum],
    backups: &Path,
) -> Result<library_db::Marked> {
    // By account: the albums to mark, and the photos in no album to make an
    // album of.
    let mut by_account: HashMap<&Path, (Vec<String>, Vec<String>)> = HashMap::new();
    for album in albums.iter().filter(|a| !a.set) {
        let (ids, loose) = by_account.entry(&album.account).or_default();
        if album.loose {
            loose.extend(album.in_cloud_assets.iter().map(|(asset, _)| asset.clone()));
        } else {
            ids.push(album.id.clone());
        }
    }
    std::fs::create_dir_all(backups)?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let mut total = library_db::Marked::default();
    for (account, (ids, loose)) in by_account {
        let name = account
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        // A name of its own, also for a second download within the second.
        let backup = (1..)
            .map(|n| match n {
                1 => backups.join(format!("Managed Catalog {name} {stamp}.mcat")),
                n => backups.join(format!("Managed Catalog {name} {stamp}-{n}.mcat")),
            })
            .find(|p| !p.exists())
            .expect("a free name");
        let new_album = (!loose.is_empty()).then(|| library_db::NewAlbum {
            name: NEW_ALBUM,
            assets: &loose,
        });
        let marked = library_db::mark_for_download(
            &account.join(lr_library::CATALOG),
            &ids,
            new_album,
            &backup,
            "Lightroom has its library open. Quit Lightroom, then download again",
        )?;
        total.albums += marked.albums;
        total.photos += marked.photos;
        total.unsettled += marked.unsettled;
        // Of several accounts, the photos of all; the first album's id.
        total.new_album = match (total.new_album, marked.new_album) {
            (Some((album, a)), Some((_, b))) => Some((album, a + b)),
            (old, new) => old.or(new),
        };
    }
    Ok(total)
}
