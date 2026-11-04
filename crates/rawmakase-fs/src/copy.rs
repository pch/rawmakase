//! The steps of copying a file to another volume, each on its own so a
//! caller can journal between them: an empty file made where nothing has
//! its name ([`create_file`]), the bytes copied into it and hashed as they
//! are read ([`copy_into`]), made durable ([`sync_file`]), given the
//! source's extras ([`copy_extras`]) and time, a file's durably
//! ([`copy_times`]), and hashed again ([`hash_file`]), its extras too
//! ([`extras`]). Each step that writes goes through the handle the file
//! was made through rather than its name, which another computer may give
//! another file meanwhile on a share (#675); but for an alternate data
//! stream on Windows, opened by its path. That holds where the system
//! keeps to the handle, as local file systems do; a share's drivers don't
//! all. Linux's CIFS keeps only the bytes
//! and the flush to the handle: it sends an extended attribute by the
//! file's name whatever handle it is given, and times and permissions too
//! once it has looked the name up again (measured through a second mount,
//! Linux 7.2 against Samba 4.22). macOS's smbfs, by its source (SMBClient),
//! opens a name too for an extended attribute (a named stream) and for
//! times (a compound create, set-info and close); not measured. Windows
//! sets times through the handle, on a share too, and opens an alternate
//! data stream by its name.
//! The file is never truncated or replaced, and the source never changed.
//! [`remove_file`] is the one unlink here, for a partial copy.
//!
//! A file's or folder's extras are what it keeps beside its contents, times
//! and permissions: its extended attributes on macOS (Finder tags,
//! FinderInfo, its resource fork, and the rest, but for what the system
//! sets itself), its `user.` ones on Linux, and its alternate data streams
//! on Windows.
use super::FsError;
use sha2::{Digest as _, Sha256};
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{Read as _, Write as _};
use std::path::Path;

/// How many bytes [`copy_into`] reads and writes at a time, between which
/// it can be stopped.
pub const COPY_CHUNK: usize = 1 << 20;

/// A file's contents, as SHA-256.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ContentHash([u8; 32]);

impl ContentHash {
    fn of(sha: Sha256) -> Self {
        Self(sha.finalize().into())
    }
}

/// Hexadecimal, as journals keep it.
impl fmt::Display for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.iter().try_for_each(|b| write!(f, "{b:02x}"))
    }
}

impl fmt::Debug for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ContentHash({self})")
    }
}

impl std::str::FromStr for ContentHash {
    type Err = &'static str;
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let mut out = [0u8; 32];
        if text.len() != 64 || !text.is_ascii() {
            return Err("not a SHA-256");
        }
        for (i, b) in out.iter_mut().enumerate() {
            *b = u8::from_str_radix(&text[2 * i..2 * i + 2], 16).map_err(|_| "not a SHA-256")?;
        }
        Ok(Self(out))
    }
}

/// Why [`copy_into`] did not copy the whole file.
#[derive(Debug)]
pub enum CopyError {
    /// Its caller asked it to stop between chunks.
    Stopped,
    /// The file to copy into isn't the empty file object it was given.
    NotTheFile,
    Fs(FsError),
}

impl From<FsError> for CopyError {
    fn from(e: FsError) -> Self {
        Self::Fs(e)
    }
}

impl From<std::io::Error> for CopyError {
    fn from(e: std::io::Error) -> Self {
        Self::Fs(e.into())
    }
}

impl fmt::Display for CopyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stopped => f.write_str("stopped"),
            Self::NotTheFile => f.write_str("the file being copied into changed"),
            Self::Fs(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for CopyError {}

/// Makes an empty file `path`, only if nothing has that name: a file,
/// folder or link already there is never opened, truncated or replaced.
/// Returns the handle it was made through, open to write, for
/// [`copy_into`]: what is copied goes into the file made, whatever another
/// program or computer puts at its name meanwhile (on an SMB share, where
/// names prove files, one could, #573).
pub fn create_file(path: &Path) -> Result<File, FsError> {
    Ok(OpenOptions::new().write(true).create_new(true).open(path)?)
}

/// Copies every byte of `from` into `to`, the handle [`create_file`] made
/// a file through, which must still be empty: `COPY_CHUNK` bytes at a
/// time, asking `go_on`, with how many bytes are written so far, after
/// each chunk. Returns the SHA-256 of the bytes read from `from`. Neither
/// syncs nor sets times; nothing is made, truncated or replaced here.
pub fn copy_into(
    from: &Path,
    to: &mut File,
    go_on: &mut dyn FnMut(u64) -> bool,
) -> Result<ContentHash, CopyError> {
    let mut source = File::open(from)?;
    if to.metadata()?.len() != 0 {
        return Err(CopyError::NotTheFile);
    }
    let mut sha = Sha256::new();
    let mut buffer = vec![0u8; COPY_CHUNK];
    let mut written = 0u64;
    loop {
        let read = match source.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
        };
        sha.update(&buffer[..read]);
        to.write_all(&buffer[..read])?;
        written += read as u64;
        if !go_on(written) {
            return Err(CopyError::Stopped);
        }
    }
    to.flush()?;
    Ok(ContentHash::of(sha))
}

/// Makes the contents of the file [`create_file`] made through `file`
/// durable: through that handle, opened to write, as an SMB server flushes
/// no other (MS-SMB2 3.3.5.11; macOS's smbfs answers EACCES), and which
/// still writes once it is given a read-only source's permissions.
pub fn sync_file(file: &File) -> Result<(), FsError> {
    Ok(file.sync_all()?)
}

/// Gives file or folder `to` the modification and access times of `from`
/// (to the nanosecond where `to`'s volume keeps them; to what it keeps
/// otherwise), and on Unix its permission bits; a file's durably, its
/// contents made durable first ([`sync_file`]). A file's through `into`,
/// the handle [`create_file`] made it through, where given.
pub fn copy_times(from: &Path, to: &Path, into: Option<&File>) -> Result<(), FsError> {
    let source = std::fs::metadata(from)?;
    let opened;
    let file = match (into, source.is_dir()) {
        (Some(into), _) => into,
        (None, true) => {
            opened = super::os::open_folder(to)?;
            &opened
        }
        (None, false) => {
            opened = OpenOptions::new().write(true).open(to)?;
            &opened
        }
    };
    let times = std::fs::FileTimes::new()
        .set_accessed(source.accessed()?)
        .set_modified(source.modified()?);
    file.set_times(times)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = source.permissions().mode() & 0o777;
        file.set_permissions(std::fs::Permissions::from_mode(mode))?;
        // An SMB server given a flush after the times sets the write time
        // to when its handle closes (macOS 15 against a Synology share,
        // and Linux 7.2's CIFS against Samba 4.22, every time). There
        // macOS's `fsync(2)` kept them every time, and Linux's, which is
        // that flush, is left out; the server applies the times as it is
        // given them, before the call returns.
        if !source.is_dir() && super::os::kind(to)? == super::VolumeKind::Smb {
            #[cfg(target_vendor = "apple")]
            return rustix::fs::fsync(file).map_err(|e| std::io::Error::from(e).into());
            #[cfg(not(target_vendor = "apple"))]
            return Ok(());
        }
    }
    // A folder's are left to the system, as before (a Windows folder's
    // handle here may only write attributes, and can't flush).
    match source.is_dir() {
        true => Ok(()),
        false => Ok(file.sync_all()?),
    }
}

/// Gives file or folder `to` each extra of `from`, as it is there; what
/// `to` has already stays. A file's through `into`, the handle
/// [`create_file`] made it through, where given; Windows opens an
/// alternate data stream by its path. [`FsError::Unsupported`] where
/// `to`'s volume can't keep one (FAT and exFAT on Linux and Windows; macOS
/// keeps them there in AppleDouble files, `._name`). Neither syncs nor
/// compares: see [`extras`].
pub fn copy_extras(from: &Path, to: &Path, into: Option<&File>) -> Result<(), FsError> {
    for name in super::os::extra_names(from)? {
        let value = super::os::read_extra(from, &name)?;
        super::os::write_extra(to, into, &name, &value)?;
    }
    Ok(())
}

/// The SHA-256 of every extra of file or folder `path`, by name: two
/// files or folders whose extras are the same have the same.
pub fn extras(path: &Path) -> Result<ContentHash, FsError> {
    let mut names = super::os::extra_names(path)?;
    names.sort();
    let mut sha = Sha256::new();
    for name in names {
        let value = super::os::read_extra(path, &name)?;
        for part in [name.as_encoded_bytes(), &value] {
            sha.update((part.len() as u64).to_le_bytes());
            sha.update(part);
        }
    }
    Ok(ContentHash::of(sha))
}

/// Gives file or folder `path` the extra `name` (on Linux `user.name`),
/// for tests that need one.
#[cfg(any(test, feature = "test-support"))]
pub fn set_extra(path: &Path, name: &str, value: &[u8]) -> Result<(), FsError> {
    let name = match cfg!(any(target_os = "linux", target_os = "android")) {
        true => format!("user.{name}"),
        false => name.to_string(),
    };
    super::os::write_extra(path, None, std::ffi::OsStr::new(&name), value)
}

/// The SHA-256 of `path`'s contents, read whole.
pub fn hash_file(path: &Path) -> Result<ContentHash, FsError> {
    hash_prefix(path, u64::MAX)
}

/// The SHA-256 of `path`'s first `len` bytes, or of all it has when it has
/// fewer: what a partial copy of it, `len` bytes long, holds.
pub fn hash_prefix(path: &Path, len: u64) -> Result<ContentHash, FsError> {
    let mut file = File::open(path)?.take(len);
    let mut sha = Sha256::new();
    let mut buffer = vec![0u8; COPY_CHUNK];
    loop {
        match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => sha.update(&buffer[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(ContentHash::of(sha))
}

/// Removes file `path`. Not for a user's file: its caller proves `path` is
/// a partial copy it made, which nothing else has.
pub fn remove_file(path: &Path) -> Result<(), FsError> {
    Ok(std::fs::remove_file(path)?)
}
