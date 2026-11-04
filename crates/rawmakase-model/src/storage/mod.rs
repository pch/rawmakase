//! Durable application state: file identity, shared storage locations and
//! atomic writes.
pub mod bitmaps;
mod checksum;
mod files;
mod formats;
mod identity;
pub mod mask_assets;

pub use checksum::{
    CHUNK as CHECKSUM_CHUNK, FileChecksum, Outcome as ChecksumRead, VERSION as CHECKSUM_VERSION,
};
pub use files::{Replace, data_dir, is_hidden, is_raw, list_raws, local_data_dir};
pub use files::{
    asset_dirs, atomic_json, parent_dir, persist, persist_noclobber, read_json_or_default,
    rename_noclobber, stage, sync_dir, write_atomic,
};
pub use formats::{RasterFormat, is_photo};
pub use identity::Stamp;
pub use identity::{FNV_OFFSET, fnv1a};
pub use identity::{IDENTITY_PREFIX, Identity};
