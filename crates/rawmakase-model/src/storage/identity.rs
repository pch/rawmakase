//! What identifies a photo's file: its size, modification time and first bytes.
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::{fs, fs::File, io::Read, path::Path, time::UNIX_EPOCH};

/// FNV-1a's standard 64-bit offset basis, the usual `seed` for [`fnv1a`].
pub const FNV_OFFSET: u64 = 0xcbf29ce484222325;
/// 64-bit FNV-1a of `bytes`, starting from `seed`. Hashing a second slice
/// from the first's result hashes the two as one.
pub fn fnv1a(seed: u64, bytes: &[u8]) -> u64 {
    bytes
        .iter()
        .fold(seed, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x100000001b3))
}

/// What tells a cached preview that its source changed: the file's size and
/// modification time, from one stat. Unlike `Identity` it reads none of the
/// file, which on a network share costs a round trip for every photo shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stamp {
    pub size: u64,
    pub modified_ns: u128,
}
impl Stamp {
    pub fn read(path: &Path) -> Result<Self> {
        let m = fs::metadata(path)?;
        Ok(Self {
            size: m.len(),
            modified_ns: m.modified()?.duration_since(UNIX_EPOCH)?.as_nanos(),
        })
    }
}

/// How many of a file's first bytes its [`Identity`] hashes.
pub const IDENTITY_PREFIX: u64 = 65536;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Identity {
    pub size: u64,
    pub modified_ns: u128,
    pub prefix_hash: String,
}
impl Identity {
    pub fn read(path: &Path) -> Result<Self> {
        let Stamp { size, modified_ns } = Stamp::read(path)?;
        let mut bytes = Vec::new();
        File::open(path)?
            .take(IDENTITY_PREFIX)
            .read_to_end(&mut bytes)?;
        let hash = fnv1a(FNV_OFFSET, &bytes);
        Ok(Self {
            size,
            modified_ns,
            prefix_hash: format!("{hash:016x}"),
        })
    }
    /// Whether `other` names the same photo, as a saved edit is bound to it:
    /// the same size and first bytes, whatever the modification time.
    ///
    /// A different photo at the same path differs in its first
    /// [`IDENTITY_PREFIX`] bytes, which hold every camera format's header and
    /// capture metadata, or in its size. The modification time alone changes
    /// without the bytes changing: a cloud mount reporting it rounded, a copy
    /// that doesn't keep it, `touch` (issue #870). Caches still key on the whole
    /// identity, so they read such a file again.
    pub fn same_photo(&self, other: &Self) -> bool {
        self.size == other.size && self.prefix_hash == other.prefix_hash
    }
    /// The size and modification time this identity names, as a preview's
    /// [`Stamp`] keeps them.
    pub fn stamp(&self) -> Stamp {
        Stamp {
            size: self.size,
            modified_ns: self.modified_ns,
        }
    }
    /// A file name for state kept per file, unique to this identity.
    pub fn key(&self) -> String {
        format!("{}-{}-{}", self.prefix_hash, self.size, self.modified_ns)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fnv1a_matches_the_reference_and_streams() {
        // Cache and file names depend on these values.
        assert_eq!(fnv1a(FNV_OFFSET, b""), 0xcbf29ce484222325);
        assert_eq!(fnv1a(FNV_OFFSET, b"a"), 0xaf63dc4c8601ec8c);
        assert_eq!(fnv1a(FNV_OFFSET, b"foobar"), 0x85944171f73967e8);
        assert_eq!(fnv1a(fnv1a(FNV_OFFSET, b"foo"), b"bar"), 0x85944171f73967e8);
    }
}
