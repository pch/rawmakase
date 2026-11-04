//! The companion and name rules on their own.
use super::companions::{self, Companions, readers};
use super::conflicts::{Destination, Names, Taken};
use super::disk::Real;
use crate::fsops::{Folding, NameRules};
use std::collections::HashSet;
use std::ffi::OsString;

fn names(list: &[&str]) -> Vec<OsString> {
    list.iter().map(OsString::from).collect()
}

fn carried(photo: &str, listing: &[&str]) -> Companions {
    // A volume that tells every two names apart.
    companions::of(photo.as_ref(), &names(listing), &|_, _| false)
}

#[test]
fn a_raws_sidecars_and_legacy_edits_go_with_it() {
    let found = carried(
        "A.NEF",
        &[
            "A.NEF",
            "A.NEF.xmp",
            "A.xmp",
            "A.NEF.rawmakase.json",
            "A.NEF.rawmakase-local.json",
            "B.NEF",
            "B.xmp",
        ],
    );
    assert_eq!(
        found.carried,
        names(&[
            "A.NEF.rawmakase-local.json",
            "A.NEF.rawmakase.json",
            "A.NEF.xmp",
            "A.xmp"
        ])
    );
    assert!(found.shared.is_empty());
}

#[test]
fn a_sidecar_is_matched_whatever_its_case() {
    // Lookup tries `.xmp` and `.XMP`; a folder may hold `.Xmp`, which is
    // still that photo's on a volume that ignores case.
    let found = carried("A.NEF", &["A.NEF", "a.nef.Xmp", "A.XMP"]);
    assert_eq!(found.carried, names(&["A.XMP", "a.nef.Xmp"]));
}

#[test]
fn a_sidecar_another_file_reads_too_stays() {
    // A.NEF.xmp is A.NEF's and A.NEF.DNG's (its stem sidecar).
    let found = carried("A.NEF", &["A.NEF", "A.NEF.DNG", "A.NEF.xmp"]);
    assert!(found.carried.is_empty());
    assert_eq!(found.shared, names(&["A.NEF.xmp"]));
    // A RAW's stem sidecar is not a JPEG's of the same name: the RAW's.
    let found = carried("A.NEF", &["A.NEF", "A.JPG", "A.xmp"]);
    assert_eq!(found.carried, names(&["A.xmp"]));
    let found = carried("A.JPG", &["A.NEF", "A.JPG", "A.xmp", "A.JPG.xmp"]);
    assert_eq!(found.carried, names(&["A.JPG.xmp"]));
    // Two RAWs of one stem share it.
    let found = carried("A.NEF", &["A.NEF", "A.DNG", "A.xmp"]);
    assert_eq!(found.shared, names(&["A.xmp"]));
}

#[test]
fn ownership_follows_the_rule_lookup_reads_sidecars_by() {
    // For every file of a folder, each sidecar lookup finds has it among
    // the readers ownership counts.
    let dir = tempfile::tempdir().unwrap();
    let list = [
        "A.NEF",
        "A.NEF.DNG",
        "A.NEF.xmp",
        "A.xmp",
        "B.JPG",
        "B.xmp",
        "B.JPG.XMP",
        "C.CR3",
        "C.XMP",
    ];
    for name in list {
        std::fs::write(dir.path().join(name), b"").unwrap();
    }
    let listing = names(&list);
    // A case-insensitive volume finds a name in either case as one file;
    // lookup asks for both cases, so dedupe what it found by name.
    for file in &listing {
        let found = crate::catalog::sidecar::sidecars(&dir.path().join(file));
        for sidecar in found {
            let name = sidecar.file_name().unwrap().to_os_string();
            let name = listing
                .iter()
                .find(|n| {
                    n.to_string_lossy()
                        .eq_ignore_ascii_case(&name.to_string_lossy())
                })
                .unwrap();
            assert!(
                readers(name, &listing).contains(file),
                "{file:?} reads {name:?}"
            );
        }
    }
}

#[test]
fn arriving_companions_and_photos_must_not_read_anything_else() {
    let projected = names(&["A.DNG", "A.NEF", "A.xmp"]);
    assert_eq!(
        companions::arrival_conflict("A.NEF".as_ref(), &names(&["A.xmp"]), &projected),
        Some("A.DNG".into())
    );
    let projected = names(&["B.NEF.xmp", "B.NEF"]);
    assert_eq!(
        companions::arrival_conflict("B.NEF".as_ref(), &[], &projected),
        Some("B.NEF.xmp".into())
    );
    let projected = names(&["C.NEF", "C.NEF.xmp", "D.NEF"]);
    assert_eq!(
        companions::arrival_conflict("C.NEF".as_ref(), &names(&["C.NEF.xmp"]), &projected),
        None
    );
}

#[test]
fn names_are_compared_as_the_folder_compares_them() {
    let exact = Names(NameRules {
        case: Folding::Exact,
        normalization: Folding::Exact,
    });
    let loose = Names(NameRules {
        case: Folding::Unknown,
        normalization: Folding::Insensitive,
    });
    let (nfc, nfd) = ("caf\u{e9}.NEF", "cafe\u{301}.NEF");
    assert_ne!(exact.key("A.NEF".as_ref()), exact.key("a.nef".as_ref()));
    assert_ne!(exact.key(nfc.as_ref()), exact.key(nfd.as_ref()));
    assert_eq!(loose.key("A.NEF".as_ref()), loose.key("a.nef".as_ref()));
    assert_eq!(loose.key(nfc.as_ref()), loose.key(nfd.as_ref()));
}

#[test]
fn a_name_is_taken_on_disk_or_in_the_catalog() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("A.NEF"), b"").unwrap();
    let listing = names(&["A.NEF"]);
    let catalog: HashSet<String> = ["b.nef".to_string()].into();
    let exact = Names(NameRules {
        case: Folding::Exact,
        normalization: Folding::Exact,
    });
    let d = Destination {
        dir: dir.path(),
        names: &listing,
        rules: exact,
        catalog: &catalog,
    };
    assert_eq!(d.taken(&Real, "A.NEF".as_ref()), Some(Taken::OnDisk));
    assert_eq!(d.taken(&Real, "B.NEF".as_ref()), Some(Taken::InCatalog));
    assert_eq!(d.taken(&Real, "C.NEF".as_ref()), None);
    // Where the volume ignores case, looking the name up finds it even
    // when the rules said exact.
    let lower = d.taken(&Real, "a.NEF".as_ref());
    if dir.path().join("a.NEF").exists() {
        assert_eq!(lower, Some(Taken::OnDisk));
    } else {
        assert_eq!(lower, None);
    }
}

/// The companion rules as they were asked of every name, one name at a
/// time, before a folder's names were indexed: what the index must answer
/// exactly.
mod scanned {
    use crate::catalog::folder_sync::folded;
    use crate::catalog::legacy_sidecar::companion_paths;
    use crate::catalog::sidecar::{caseless, sidecar_names, sidecar_names_caseless};
    use std::ffi::{OsStr, OsString};
    use std::path::Path;

    fn names_of(photo: &OsStr) -> Vec<String> {
        let mut names = sidecar_names_caseless(Path::new(photo));
        names.extend(
            companion_paths(Path::new(photo))
                .iter()
                .map(|p| caseless(p.as_os_str())),
        );
        names
    }

    fn reads(photo: &OsStr, name: &OsStr) -> bool {
        photo != name && name.to_str().is_some() && names_of(photo).contains(&caseless(name))
    }

    pub(super) fn readers<'a>(companion: &OsStr, names: &'a [OsString]) -> Vec<&'a OsString> {
        let Some(wanted) = companion.to_str().map(|s| caseless(OsStr::new(s))) else {
            return Vec::new();
        };
        let mut found: Vec<&OsString> = names
            .iter()
            .filter(|n| n.as_os_str() != companion && n.to_str().is_some())
            .filter(|n| sidecar_names_caseless(Path::new(n)).contains(&wanted))
            .collect();
        for name in names {
            let legacy = companion_paths(Path::new(name))
                .iter()
                .any(|p| caseless(p.as_os_str()) == caseless(companion));
            if legacy && companion.to_str().is_some() && !found.contains(&name) {
                found.push(name);
            }
        }
        found
    }

    fn is_companion(
        photo: &OsStr,
        name: &OsStr,
        same_file: &dyn Fn(&OsStr, &OsStr) -> bool,
    ) -> bool {
        if photo == name || name.to_str().is_none() {
            return false;
        }
        let mut candidates = sidecar_names(Path::new(photo));
        candidates.extend(
            companion_paths(Path::new(photo))
                .into_iter()
                .map(|p| p.into_os_string()),
        );
        candidates.iter().any(|c| {
            folded(name) == folded(c) || (caseless(name) == caseless(c) && same_file(name, c))
        })
    }

    pub(super) fn of(
        photo: &OsStr,
        names: &[OsString],
        same_file: &dyn Fn(&OsStr, &OsStr) -> bool,
    ) -> (Vec<OsString>, Vec<OsString>) {
        let (mut carried, mut shared) = (Vec::new(), Vec::new());
        for name in names.iter().filter(|n| is_companion(photo, n, same_file)) {
            if readers(name, names).iter().all(|r| r.as_os_str() == photo) {
                carried.push(name.clone());
            } else {
                shared.push(name.clone());
            }
        }
        carried.sort();
        shared.sort();
        (carried, shared)
    }

    pub(super) fn arrival_conflict(
        photo: &OsStr,
        carried: &[OsString],
        projected: &[OsString],
    ) -> Option<OsString> {
        let theirs = projected
            .iter()
            .filter(|n| !carried.contains(n) && n.as_os_str() != photo);
        for name in theirs {
            if reads(photo, name) {
                return Some(name.clone());
            }
        }
        for companion in carried {
            let others = readers(companion, projected)
                .into_iter()
                .find(|r| r.as_os_str() != photo);
            if let Some(other) = others {
                return Some(other.clone());
            }
        }
        None
    }
}

/// A folder's names asking the hard questions of an index.
fn hard_names() -> Vec<OsString> {
    let listing = names(&[
        "A.NEF",
        "a.nef",
        "A.NEF.xmp",
        "a.NEF.XMP",
        "A.xmp",
        "a.XMP",
        "A.NEF.rawmakase.json",
        "a.nef.RAWMAKASE.JSON",
        "A.NEF.rawmakase-local.json",
        "A.JPG",
        "A.JPG.xmp",
        "Straße.NEF",
        "STRASSE.xmp",
        "strasse.nef.xmp",
        "Straẞe.NEF.xmp",
        "Caf\u{e9}.ARW",
        "Cafe\u{301}.xmp",
        "CAFE\u{301}.ARW.XMP",
        "\u{fb01}le.DNG",
        "file.xmp",
        "FILE.DNG.xmp",
        "B.CR3",
        "B.xmp",
        "B.xmp.xmp",
        "C.tif",
        "C.tif.xmp",
        "C.xmp",
        "D",
        "D.xmp",
        ".xmp",
        "E.NEF.DNG",
        "E.NEF.DNG.xmp",
        "E.NEF.xmp",
    ]);
    // Names that aren't text, where the system has them.
    #[cfg(unix)]
    let listing = {
        use std::os::unix::ffi::OsStringExt;
        let mut listing = listing;
        listing.push(OsString::from_vec(b"A\xff.NEF".to_vec()));
        listing.push(OsString::from_vec(b"A\xff.NEF.xmp".to_vec()));
        listing
    };
    listing
}

#[test]
fn an_indexed_folder_answers_as_asking_every_name_did() {
    use crate::catalog::sidecar::caseless;
    use std::ffi::OsStr;
    let listing = hard_names();
    // A volume that takes some names alike for one file and others not.
    let same_file =
        |a: &OsStr, b: &OsStr| caseless(a) == caseless(b) && !(a.len() + b.len()).is_multiple_of(3);
    let index = companions::Listing::new(listing.clone());
    let (mut shared, mut clashes) = (0, 0);
    for photo in &listing {
        let expected = scanned::of(photo, &listing, &same_file);
        shared += expected.1.len();
        let found = index.companions(photo, &same_file);
        assert_eq!((found.carried, found.shared), expected, "{photo:?}");
        assert_eq!(
            readers(photo, &listing),
            scanned::readers(photo, &listing)
                .into_iter()
                .cloned()
                .collect::<Vec<_>>(),
            "readers of {photo:?}"
        );
        // Carrying each name in turn, with each other name gone.
        for (i, carried) in listing.iter().enumerate() {
            let carried = vec![carried.clone()];
            for gone in [None, Some(i), Some((i + 1) % listing.len())] {
                let projected: Vec<OsString> = listing
                    .iter()
                    .enumerate()
                    .filter(|(j, _)| Some(*j) != gone)
                    .map(|(_, n)| n.clone())
                    .collect();
                let expected = scanned::arrival_conflict(photo, &carried, &projected);
                clashes += usize::from(expected.is_some());
                assert_eq!(
                    index.arrival_conflict(photo, &carried, &|j| Some(j) == gone),
                    expected,
                    "{photo:?} carrying {carried:?} without {gone:?}"
                );
            }
        }
    }
    // The listing asks the hard questions: companions read by two files,
    // and clashes found as well as not.
    assert!(shared > 5, "{shared}");
    assert!(clashes > 100, "{clashes}");
}

#[test]
fn an_index_changed_name_by_name_answers_as_one_read_whole() {
    use crate::catalog::sidecar::caseless;
    use std::ffi::OsStr;
    let same_file = |a: &OsStr, b: &OsStr| caseless(a) == caseless(b);
    let answers = |index: &companions::Listing, names: &[OsString]| {
        let mut listed = index.names().to_vec();
        listed.sort();
        let mut asked = Vec::new();
        for photo in names {
            let found = index.companions(photo, &same_file);
            let mut alike: Vec<OsString> = index.alike(photo).cloned().collect();
            alike.sort();
            let clash = index.arrival_conflict(photo, std::slice::from_ref(photo), &|_| false);
            asked.push((found.carried, found.shared, alike, clash.is_some()));
        }
        (listed, asked)
    };
    let all = hard_names();
    let mut index = companions::Listing::new(all.clone());
    let mut now = all.clone();
    // Each name renamed away and back in turn, some of them twice over,
    // as a batch's renames change a folder's listing.
    for (i, name) in all.iter().enumerate() {
        index.remove(name);
        now.retain(|n| n != name);
        let mut renamed = name.clone();
        renamed.push(".moved");
        index.push(renamed.clone());
        now.push(renamed.clone());
        if i % 3 == 0 {
            // Another arriving and leaving again, as a check projects.
            let len = index.names().len();
            index.push(name.clone());
            index.push(all[(i + 1) % all.len()].clone());
            index.truncate(len);
        }
        if i % 2 == 0 {
            index.remove(&renamed);
            now.retain(|n| *n != renamed);
            index.push(name.clone());
            now.push(name.clone());
        }
        let whole = companions::Listing::new(now.clone());
        assert_eq!(
            answers(&index, &all),
            answers(&whole, &all),
            "after {name:?}"
        );
    }
}
