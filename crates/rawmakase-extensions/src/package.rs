//! `.rmext` packages: a zip named `<id>-<version>-<target>.rmext` holding the
//! manifest, the program for that target, its other files and `SHA256SUMS`,
//! which lists every other file with its SHA-256 in `sha256sum`'s format.
//!
//! A package is refused, before anything of it is used, when an entry's name
//! isn't a plain relative path (absolute, `..`, a backslash, a drive), when it
//! is a link, when two names differ only in case, when a file is missing from
//! `SHA256SUMS` or listed there but missing, when a digest doesn't match, or
//! when it unpacks to more than [`MAX_UNPACKED`]. Sizes are counted as the
//! bytes come out, not taken from the zip's headers.
use crate::{Error, Manifest, Result};
use rawmakase_extension_protocol::manifest::{self, is_package_path};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::path::Path;

/// The checksum file's name, at the package's root.
pub const SUMS: &str = "SHA256SUMS";
/// The most a package may unpack to.
pub const MAX_UNPACKED: u64 = 512 << 20;
/// The most entries a package may hold.
const MAX_ENTRIES: usize = 10_000;
/// The longest `SHA256SUMS` and manifest read.
const MAX_TEXT: u64 = 1 << 20;
/// A package's file name extension.
pub const EXTENSION: &str = "rmext";

/// What a package holds, read without unpacking it.
#[derive(Clone, Debug, PartialEq)]
pub struct Inspected {
    /// Its manifest.
    pub manifest: Manifest,
    /// The target its file name states.
    pub target: String,
}

/// `<id>-<version>-<target>.rmext`.
pub fn file_name(manifest: &Manifest, target: &str) -> String {
    format!(
        "{}-{}-{target}.{EXTENSION}",
        manifest.extension.id, manifest.extension.version
    )
}

/// The target a package's file name states, given its manifest's id and
/// version.
pub fn target_of(name: &str, manifest: &Manifest) -> Option<String> {
    let prefix = format!("{}-{}-", manifest.extension.id, manifest.extension.version);
    let target = name
        .strip_suffix(&format!(".{EXTENSION}"))?
        .strip_prefix(&prefix)?;
    let plain = !target.is_empty()
        && target
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    plain.then(|| target.to_string())
}

/// Reads a package's manifest and checks its file name and entry names,
/// without unpacking or verifying its files.
pub fn inspect(path: &Path) -> Result<Inspected> {
    let mut archive = open(path)?;
    check_names(&mut archive)?;
    let manifest = read_manifest(&mut archive)?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    let target = target_of(name, &manifest).ok_or_else(|| {
        Error::new(format!(
            "A package for {} {} is named {}, not {name}",
            manifest.extension.id,
            manifest.extension.version,
            file_name(&manifest, "<target>")
        ))
    })?;
    Ok(Inspected { manifest, target })
}

/// Unpacks a package into `into`, an empty folder, verifying every file
/// against `SHA256SUMS`. On an error `into` may hold part of the package; the
/// caller removes it.
pub fn unpack(path: &Path, into: &Path) -> Result<Manifest> {
    unpack_within(path, into, MAX_UNPACKED)
}

/// [`unpack`] with the size cap a test can lower.
fn unpack_within(path: &Path, into: &Path, cap: u64) -> Result<Manifest> {
    let mut archive = open(path)?;
    check_names(&mut archive)?;
    let sums = read_sums(&mut archive)?;
    let manifest = read_manifest(&mut archive)?;
    let mut total = 0u64;
    let mut seen = BTreeSet::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(zip_error)?;
        let name = entry
            .name()
            .map_err(zip_error)?
            .trim_end_matches('/')
            .to_string();
        let target = into.join(local_path(&name));
        if entry.is_dir() {
            std::fs::create_dir_all(&target)?;
            continue;
        }
        if name == SUMS {
            continue;
        }
        let Some(expected) = sums.get(&name) else {
            return Err(Error::new(format!("{name} is not listed in {SUMS}")));
        };
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = std::fs::File::create_new(&target)
            .map_err(|e| Error::from(e).context(target.display()))?;
        let mut hash = Sha256::new();
        let mut buffer = vec![0u8; 64 << 10];
        loop {
            let n = entry.read(&mut buffer).map_err(|e| {
                Error::from(e).context(format!("Could not read {name} in the package"))
            })?;
            if n == 0 {
                break;
            }
            total += n as u64;
            if total > cap {
                return Err(Error::new(format!(
                    "The package unpacks to more than {} MiB",
                    cap >> 20
                )));
            }
            hash.update(&buffer[..n]);
            file.write_all(&buffer[..n])?;
        }
        file.sync_all()?;
        if hex(&hash.finalize()) != *expected {
            return Err(Error::new(format!(
                "{name} doesn't match its SHA-256 in {SUMS}: the package is damaged or was altered"
            )));
        }
        seen.insert(name);
    }
    if let Some(missing) = sums.keys().find(|name| !seen.contains(*name)) {
        return Err(Error::new(format!(
            "{missing} is listed in {SUMS} but not in the package"
        )));
    }
    // Only Unix marks the program executable, which needs its target's name.
    #[cfg_attr(not(unix), allow(unused_variables))]
    let stated = check_inventory(path, &manifest, |name| seen.contains(name))?;
    #[cfg(unix)]
    if let Some(program) = &manifest.program {
        use std::os::unix::fs::PermissionsExt;
        let file = into.join(local_path(&program_path(program, &stated)));
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(manifest)
}

/// Whether `dir` holds exactly the files of the package at `path`, as
/// [`unpack`] leaves them: every file `SHA256SUMS` lists, with its digest,
/// and no other file or link. Folders aren't compared. Anything that can't
/// be read counts as a difference, and so does a package [`unpack`] would
/// refuse for lacking a file its manifest names ([`check_inventory`]).
pub fn is_unpacked_in(path: &Path, dir: &Path) -> bool {
    let Ok(sums) = open(path).and_then(|mut archive| {
        let sums = read_sums(&mut archive)?;
        let manifest = read_manifest(&mut archive)?;
        check_inventory(path, &manifest, |name| sums.contains_key(name))?;
        Ok(sums)
    }) else {
        return false;
    };
    let mut found = BTreeSet::new();
    if !list_files(dir, "", &mut found) || found.len() != sums.len() {
        return false;
    }
    sums.iter().all(|(name, expected)| {
        found.contains(name)
            && std::fs::File::open(dir.join(local_path(name)))
                .and_then(|mut file| {
                    let mut hash = Sha256::new();
                    let mut buffer = vec![0u8; 64 << 10];
                    loop {
                        match file.read(&mut buffer)? {
                            0 => break,
                            n => hash.update(&buffer[..n]),
                        }
                    }
                    Ok(hex(&hash.finalize()) == *expected)
                })
                .unwrap_or(false)
    })
}

/// Adds the files under `dir` to `found` as package paths, after `prefix`.
/// False when it holds a link or a name that isn't UTF-8, or can't be read.
fn list_files(dir: &Path, prefix: &str, found: &mut BTreeSet<String>) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    for entry in entries {
        let Ok(entry) = entry else { return false };
        let name = entry.file_name();
        let (Ok(kind), Some(name)) = (entry.file_type(), name.to_str()) else {
            return false;
        };
        let name = format!("{prefix}{name}");
        if kind.is_dir() {
            if !list_files(&entry.path(), &format!("{name}/"), found) {
                return false;
            }
        } else if kind.is_file() {
            found.insert(name);
        } else {
            return false;
        }
    }
    true
}

/// A package path as a relative path on this computer.
fn local_path(name: &str) -> std::path::PathBuf {
    name.split('/').collect()
}

/// The target the package's file name states, which decides its program's
/// name: this computer's for a package named otherwise or for `any`. The
/// host installs only packages for its own target, so there the two agree;
/// `cargo rawmakase package` checks packages for other targets with it.
fn stated_target(path: &Path, manifest: &Manifest) -> String {
    path.file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| target_of(n, manifest))
        .filter(|t| t != crate::ANY_TARGET)
        .unwrap_or_else(crate::current_target)
}

/// The program's path in a package for `target`: its manifest path, with
/// `.exe` added for a Windows target when the file name has no extension.
pub fn program_path(program: &manifest::Program, target: &str) -> String {
    let mut path = program.path.clone();
    let name = path.rsplit('/').next().unwrap_or_default();
    if target.starts_with("windows-") && !name.contains('.') {
        path.push_str(".exe");
    }
    path
}

/// Checks that a package's files, those `holds` says it has, include every
/// file its manifest names for the target its file name states, and returns
/// that target. [`unpack`] and [`is_unpacked_in`] both apply it, so a folder
/// is reused only for a package that could be unpacked.
fn check_inventory(
    path: &Path,
    manifest: &Manifest,
    holds: impl Fn(&str) -> bool,
) -> Result<String> {
    let stated = stated_target(path, manifest);
    for (key, path) in package_paths(manifest, &stated) {
        if !holds(&path) {
            return Err(Error::new(format!("{key} {path} is not in the package")));
        }
    }
    Ok(stated)
}

/// The files the manifest names, which a package for `target` must hold.
fn package_paths(manifest: &Manifest, target: &str) -> Vec<(&'static str, String)> {
    let mut paths = vec![("The manifest", manifest::FILE.to_string())];
    if let Some(program) = &manifest.program {
        paths.push(("program.path", program_path(program, target)));
    }
    if let Some(icon) = &manifest.extension.icon {
        paths.push(("extension.icon", icon.clone()));
    }
    paths
}

fn open(path: &Path) -> Result<zip::ZipArchive<std::fs::File>> {
    let file = std::fs::File::open(path).map_err(|e| Error::from(e).context(path.display()))?;
    zip::ZipArchive::new(file)
        .map_err(|e| Error::new(format!("{} is not a package: {e}", path.display())))
}

fn zip_error(e: zip::result::ZipError) -> Error {
    Error::new(format!("The package can't be read: {e}"))
}

/// Checks every entry's name and kind.
fn check_names(archive: &mut zip::ZipArchive<std::fs::File>) -> Result<()> {
    if archive.len() > MAX_ENTRIES {
        return Err(Error::new(format!(
            "The package holds more than {MAX_ENTRIES} files"
        )));
    }
    let mut folded = BTreeSet::new();
    for index in 0..archive.len() {
        let entry = archive.by_index_raw(index).map_err(zip_error)?;
        let raw = entry.name().map_err(zip_error)?.into_owned();
        let name = raw.strip_suffix('/').unwrap_or(&raw);
        if !is_package_path(name) {
            return Err(Error::new(format!(
                "The package holds {raw:?}, which is not a plain relative path"
            )));
        }
        if entry.is_symlink() {
            return Err(Error::new(format!("The package holds a link, {name}")));
        }
        // Two names a case-insensitive file system would make one.
        if !folded.insert(name.to_lowercase()) {
            return Err(Error::new(format!("The package holds {name} twice")));
        }
    }
    Ok(())
}

fn read_text(archive: &mut zip::ZipArchive<std::fs::File>, name: &str) -> Result<String> {
    let entry = archive
        .by_name(name)
        .map_err(|_| Error::new(format!("The package has no {name}")))?;
    let mut text = String::new();
    entry
        .take(MAX_TEXT + 1)
        .read_to_string(&mut text)
        .map_err(|e| Error::from(e).context(name))?;
    if text.len() as u64 > MAX_TEXT {
        return Err(Error::new(format!("{name} is too large")));
    }
    Ok(text)
}

fn read_manifest(archive: &mut zip::ZipArchive<std::fs::File>) -> Result<Manifest> {
    let text = read_text(archive, manifest::FILE)?;
    Manifest::parse(&text).map_err(|e| Error::new(e.0).context(manifest::FILE))
}

/// `SHA256SUMS` by name: `<64 hex digits>  <path>` per line (a `*` before the
/// path, binary mode, is accepted too).
fn read_sums(archive: &mut zip::ZipArchive<std::fs::File>) -> Result<BTreeMap<String, String>> {
    parse_sums(&read_text(archive, SUMS)?)
}

/// See [`read_sums`].
pub fn parse_sums(text: &str) -> Result<BTreeMap<String, String>> {
    let mut sums = BTreeMap::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let bad = || Error::new(format!("{SUMS} has a line it can't read: {line:?}"));
        let (digest, rest) = line.split_once(' ').ok_or_else(bad)?;
        let name = rest
            .strip_prefix(' ')
            .or_else(|| rest.strip_prefix('*'))
            .ok_or_else(bad)?;
        let digest = digest.to_ascii_lowercase();
        if digest.len() != 64 || !digest.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(bad());
        }
        if name == SUMS || !is_package_path(name) {
            return Err(bad());
        }
        if sums.insert(name.to_string(), digest).is_some() {
            return Err(Error::new(format!("{SUMS} lists {name} twice")));
        }
    }
    Ok(sums)
}

/// `SHA256SUMS`' text for these files, sorted by name.
pub fn write_sums<'a>(files: impl IntoIterator<Item = (&'a str, &'a [u8])>) -> String {
    let sorted: BTreeMap<_, _> = files
        .into_iter()
        .map(|(name, bytes)| (name, hex(&Sha256::digest(bytes))))
        .collect();
    sorted
        .into_iter()
        .map(|(name, digest)| format!("{digest}  {name}\n"))
        .collect()
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Writes a package: `files` (path in the package, bytes) and their
/// `SHA256SUMS`, deflated, with the program marked executable.
pub fn write(path: &Path, files: &[(String, Vec<u8>)], program: Option<&str>) -> Result<()> {
    let out = std::fs::File::create(path).map_err(|e| Error::from(e).context(path.display()))?;
    let mut zip = zip::ZipWriter::new(out);
    let sums = write_sums(files.iter().map(|(n, b)| (n.as_str(), b.as_slice())));
    let entries = files
        .iter()
        .map(|(n, b)| (n.as_str(), b.as_slice()))
        .chain([(SUMS, sums.as_bytes())]);
    for (name, bytes) in entries {
        let mode = if Some(name) == program { 0o755 } else { 0o644 };
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .unix_permissions(mode);
        zip.start_file(name, options).map_err(zip_error)?;
        zip.write_all(bytes)?;
    }
    zip.finish().map_err(zip_error)?;
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) const MANIFEST: &str = "[extension]\nid = \"dev.test.tool\"\nname = \"Tool\"\n\
        version = \"VERSION\"\nrawmakase = \">=0.1\"\napi = 1\n\
        [program]\npath = \"bin/tool\"\n[[command]]\nid = \"go\"\ntitle = \"Go\"\n";

    /// A package's files, for `version`, with the program for this platform.
    pub(crate) fn files(version: &str) -> Vec<(String, Vec<u8>)> {
        let program = if cfg!(windows) {
            "bin/tool.exe"
        } else {
            "bin/tool"
        };
        vec![
            (
                manifest::FILE.into(),
                MANIFEST.replace("VERSION", version).into_bytes(),
            ),
            (program.into(), format!("program {version}").into_bytes()),
        ]
    }

    pub(crate) fn package(dir: &Path, version: &str) -> std::path::PathBuf {
        let path = dir.join(format!(
            "dev.test.tool-{version}-{}.rmext",
            crate::current_target()
        ));
        write(&path, &files(version), None).unwrap();
        path
    }

    /// A zip written by hand, for packages `write` would never make.
    pub(crate) fn raw_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        for (name, bytes) in entries {
            zip.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn a_package_unpacks_verified() {
        let dir = tempfile::tempdir().unwrap();
        let path = package(dir.path(), "1.0.0");
        let inspected = inspect(&path).unwrap();
        assert_eq!(inspected.target, crate::current_target());
        let into = dir.path().join("out");
        std::fs::create_dir(&into).unwrap();
        let manifest = unpack(&path, &into).unwrap();
        assert_eq!(manifest, inspected.manifest);
        let program = crate::program_file(&into, manifest.program.as_ref().unwrap());
        assert_eq!(std::fs::read(&program).unwrap(), b"program 1.0.0");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&program).unwrap().permissions().mode();
            assert_eq!(mode & 0o111, 0o111);
        }
    }

    #[test]
    fn a_package_is_checked_by_the_program_name_of_its_own_target() {
        // The other platform family's naming: `.exe` or none.
        let (target, theirs, ours) = if cfg!(windows) {
            ("linux-x86_64", "bin/tool", "bin/tool.exe")
        } else {
            ("windows-x86_64", "bin/tool.exe", "bin/tool")
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir
            .path()
            .join(format!("dev.test.tool-1.0.0-{target}.rmext"));
        let manifest = files("1.0.0").swap_remove(0);
        for (program, accepted) in [(theirs, true), (ours, false)] {
            let files = [manifest.clone(), (program.into(), b"program".to_vec())];
            write(&path, &files, Some(program)).unwrap();
            let into = dir.path().join(format!("out-{accepted}"));
            std::fs::create_dir(&into).unwrap();
            let unpacked = unpack(&path, &into);
            assert_eq!(unpacked.is_ok(), accepted, "{program}: {unpacked:?}");
            if !accepted {
                assert!(unpacked.unwrap_err().0.contains("program.path"));
            }
        }
    }

    fn refused(entries: &[(&str, &[u8])]) -> String {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(format!(
            "dev.test.tool-1.0.0-{}.rmext",
            crate::current_target()
        ));
        raw_zip(&path, entries);
        let into = dir.path().join("out");
        std::fs::create_dir(&into).unwrap();
        let error = inspect(&path)
            .and_then(|_| unpack(&path, &into))
            .unwrap_err();
        error.0
    }

    #[test]
    fn files_missing_extra_or_altered_are_refused() {
        let files = files("1.0.0");
        let (manifest, program) = (&files[0], &files[1]);
        let sums = write_sums([
            (manifest.0.as_str(), manifest.1.as_slice()),
            (program.0.as_str(), program.1.as_slice()),
        ]);
        let m = (manifest.0.as_str(), manifest.1.as_slice());
        let p = (program.0.as_str(), program.1.as_slice());
        let s = (SUMS, sums.as_bytes());
        // Accepted as a whole, so each refusal below is the change alone.
        let dir = tempfile::tempdir().unwrap();
        let good = dir.path().join(format!(
            "dev.test.tool-1.0.0-{}.rmext",
            crate::current_target()
        ));
        raw_zip(&good, &[m, p, s]);
        std::fs::create_dir(dir.path().join("out")).unwrap();
        unpack(&good, &dir.path().join("out")).unwrap();

        assert!(refused(&[m, p, s, ("extra.txt", b"x")]).contains("not listed"));
        assert!(refused(&[m, s]).contains("not in the package"));
        assert!(refused(&[m, (p.0, b"altered"), s]).contains("SHA-256"));
        assert!(refused(&[m, p]).contains("no SHA256SUMS"));
        let only_manifest = write_sums([(m.0, m.1)]);
        assert!(refused(&[m, (SUMS, only_manifest.as_bytes())]).contains("program.path"));
    }

    #[test]
    fn paths_leaving_the_package_and_twins_are_refused() {
        for name in [
            "../evil",
            "/etc/evil",
            "a/../../evil",
            "C:/evil",
            "a\\b",
            "./a",
            "readme.",
            "bin/tool ",
            ".. /evil",
            "...",
            "NUL",
            "nul.txt",
            "a/COM1.blah",
            "LPT9",
            "COM¹",
            "con .txt",
        ] {
            let error = refused(&[(name, b"x")]);
            assert!(error.contains("plain relative path"), "{name}: {error}");
        }
        assert!(refused(&[("Read.me", b"x"), ("read.me", b"y")]).contains("twice"));
    }

    #[test]
    fn a_link_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(format!(
            "dev.test.tool-1.0.0-{}.rmext",
            crate::current_target()
        ));
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        zip.add_symlink(
            "bin/tool",
            "/bin/sh",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        zip.finish().unwrap();
        assert!(inspect(&path).unwrap_err().0.contains("link"));
    }

    #[test]
    fn a_package_past_the_size_cap_stops_unpacking() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(format!(
            "dev.test.tool-1.0.0-{}.rmext",
            crate::current_target()
        ));
        // Zeros deflate to almost nothing: the cap counts what comes out.
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        let files = files("1.0.0");
        let big_sum = format!("{}  big.bin\n", "0".repeat(64));
        let sums = write_sums(files.iter().map(|(n, b)| (n.as_str(), b.as_slice()))) + &big_sum;
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, bytes) in &files {
            zip.start_file(name.as_str(), options).unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.start_file(SUMS, options).unwrap();
        zip.write_all(sums.as_bytes()).unwrap();
        zip.start_file("big.bin", options).unwrap();
        zip.write_all(&vec![0u8; 3 << 20]).unwrap();
        zip.finish().unwrap();
        assert!(std::fs::metadata(&path).unwrap().len() < 1 << 20);
        let into = dir.path().join("out");
        std::fs::create_dir(&into).unwrap();
        let error = unpack_within(&path, &into, 2 << 20).unwrap_err();
        assert!(error.0.contains("more than 2 MiB"), "{error}");
    }

    #[test]
    fn the_file_name_states_the_target() {
        let manifest = Manifest::parse(&MANIFEST.replace("VERSION", "2.1.0")).unwrap();
        assert_eq!(
            target_of("dev.test.tool-2.1.0-linux-x86_64.rmext", &manifest).as_deref(),
            Some("linux-x86_64")
        );
        assert_eq!(
            target_of("dev.test.tool-2.0.0-linux-x86_64.rmext", &manifest),
            None
        );
        assert_eq!(target_of("dev.test.tool-2.1.0-.rmext", &manifest), None);
        assert_eq!(target_of("dev.test.tool-2.1.0-linux.zip", &manifest), None);
        let dir = tempfile::tempdir().unwrap();
        let renamed = dir.path().join("download (1).rmext");
        write(&renamed, &files("2.1.0"), None).unwrap();
        assert!(inspect(&renamed).unwrap_err().0.contains("is named"));
    }

    #[test]
    fn sums_read_in_sha256sum_format() {
        let digest = "a".repeat(64);
        let sums = parse_sums(&format!("{digest}  bin/tool\n{digest} *icon.png\n")).unwrap();
        assert_eq!(sums.len(), 2);
        for bad in [
            format!("{digest} bin/tool"),
            format!("{}  bin/tool", "a".repeat(63)),
            format!("{digest}  ../tool"),
            format!("{digest}  a\n{digest}  a"),
        ] {
            assert!(parse_sums(&bad).is_err(), "{bad}");
        }
    }
}
