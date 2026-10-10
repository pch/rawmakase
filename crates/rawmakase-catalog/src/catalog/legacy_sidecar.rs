//! Edits saved beside the RAW (`*.rawmakase.json`) by releases before the
//! catalog: identity-checked, with a fallback store for read-only folders and
//! conflict protection. Adding a folder imports them; RAWmakase no longer
//! writes them; the writer stays for the persistence tests. Not to be confused
//! with [`sidecars`](super::sidecars), the XMP metadata files beside a photo.
use crate::export_settings::ExportOptions;
use crate::model::recipe::{LocalEdits, Recipe};
use crate::model::saved_format::{SCHEMA, migrate_recipe};
use crate::storage::{Identity, atomic_json, bitmaps, data_dir};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File},
    path::{Path, PathBuf},
};

/// A photo's saved edit. Unknown fields (from a newer release) are kept.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Sidecar {
    pub schema: u32,
    pub pipeline: u32,
    pub source: Identity,
    /// The recipe with its spots and masks, which are saved in the companion file.
    pub recipe: Recipe,
    pub export: ExportOptions,
    #[serde(flatten)]
    pub unknown: std::collections::BTreeMap<String, serde_json::Value>,
}
/// Spot removal, masks and their bitmaps (experimental), saved next to the sidecar in
/// `photo.ARW.rawmakase-local.json`, so releases before them keep reading the sidecar.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Companion {
    schema: u32,
    source: Identity,
    #[serde(flatten)]
    local: LocalEdits,
    /// Compressed bitmaps the edits refer to by hash, base64-encoded (see
    /// `storage::bitmaps`).
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    bitmaps: std::collections::BTreeMap<String, String>,
}
const COMPANION_SCHEMA: u32 = 1;
pub fn sidecar_path(raw: &Path) -> PathBuf {
    let mut p = raw.as_os_str().to_os_string();
    p.push(".rawmakase.json");
    p.into()
}
/// Where the photo's spots and masks are saved: next to the sidecar.
fn local_path(raw: &Path) -> PathBuf {
    let mut p = raw.as_os_str().to_os_string();
    p.push(".rawmakase-local.json");
    p.into()
}
fn fallback_at(identity: &Identity, store: &Path, extension: &str) -> PathBuf {
    store
        .to_path_buf()
        .join("sidecars")
        .join(format!("{}.{extension}", identity.key()))
}
fn parse_companion(path: &Path, id: &Identity) -> Result<Companion> {
    ensure!(
        fs::metadata(path)?.len() < 16_000_000,
        "Spots and masks file too large"
    );
    let c: Companion = serde_json::from_reader(File::open(path)?)?;
    ensure!(
        c.schema <= COMPANION_SCHEMA,
        "Spots and masks saved by a newer release: preserved without changes"
    );
    ensure!(
        &c.source == id,
        "RAW identity differs from saved spots and masks: file preserved"
    );
    c.local.validate()?;
    Ok(c)
}
fn parse_sidecar(path: &Path, id: &Identity) -> Result<Sidecar> {
    ensure!(fs::metadata(path)?.len() < 16_000_000, "Sidecar too large");
    let mut v: serde_json::Value = serde_json::from_reader(File::open(path)?)?;
    migrate_recipe(&mut v)?;
    let s: Sidecar = serde_json::from_value(v)?;
    ensure!(
        &s.source == id,
        "RAW identity differs from saved edits: sidecar preserved"
    );
    s.recipe.validate()?;
    s.export.validate()?;
    Ok(s)
}
pub fn load(raw: &Path) -> Result<Option<Sidecar>> {
    load_at(raw, &data_dir())
}
fn load_at(raw: &Path, store: &Path) -> Result<Option<Sidecar>> {
    Ok(chosen(raw, store)?.map(|(mut sidecar, companion)| {
        if let Some(companion) = companion {
            sidecar.recipe = sidecar.recipe.with_local(companion.local);
        }
        sidecar
    }))
}
/// A photo's sidecar edit and the bitmaps its spots and masks refer to, both from
/// the store `load` chooses, for importing into the catalog.
pub(crate) fn import(raw: &Path) -> Result<Option<(Sidecar, Vec<bitmaps::Bitmap>)>> {
    import_at(raw, &data_dir())
}
fn import_at(raw: &Path, store: &Path) -> Result<Option<(Sidecar, Vec<bitmaps::Bitmap>)>> {
    let Some((mut sidecar, companion)) = chosen(raw, store)? else {
        return Ok(None);
    };
    let mut bitmaps = Vec::new();
    if let Some(companion) = companion {
        bitmaps = companion
            .bitmaps
            .values()
            .map(|text| bitmaps::Bitmap::decompress(&bitmaps::from_base64(text)?))
            .collect::<Result<_>>()?;
        sidecar.recipe = sidecar.recipe.with_local(companion.local);
    }
    Ok(Some((sidecar, bitmaps)))
}
/// The newer of the photo's two sidecar stores, and the spots and masks saved there.
fn chosen(raw: &Path, store: &Path) -> Result<Option<(Sidecar, Option<Companion>)>> {
    let id = Identity::read(raw)?;
    let primary = sidecar_path(raw);
    let backup = fallback_at(&id, store, "json");
    // Validate both stores before choosing newest; never hide a conflicting primary.
    let a = if primary.exists() {
        Some(parse_sidecar(&primary, &id)?)
    } else {
        None
    };
    let b = if backup.exists() {
        Some(parse_sidecar(&backup, &id)?)
    } else {
        None
    };
    let use_backup = b.is_some()
        && (a.is_none()
            || fs::metadata(&backup)?.modified()? > fs::metadata(&primary)?.modified()?);
    let (chosen, local) = if use_backup {
        (b, fallback_at(&id, store, "local.json"))
    } else {
        (a, local_path(raw))
    };
    let Some(sidecar) = chosen else {
        return Ok(None);
    };
    let companion = if local.exists() {
        Some(parse_companion(&local, &id)?)
    } else {
        None
    };
    Ok(Some((sidecar, companion)))
}
pub fn save(raw: &Path, recipe: &Recipe, export: &ExportOptions) -> Result<PathBuf> {
    save_at(raw, recipe, export, &data_dir())
}
fn save_at(raw: &Path, recipe: &Recipe, export: &ExportOptions, store: &Path) -> Result<PathBuf> {
    recipe.validate()?;
    export.validate()?;
    let source = Identity::read(raw)?;
    let primary = sidecar_path(raw);
    let mut unknown = Default::default();
    if primary.exists() {
        unknown = parse_sidecar(&primary, &source)?.unknown;
    }
    let backup = fallback_at(&source, store, "json");
    if backup.exists() {
        parse_sidecar(&backup, &source)?;
    }
    let (saved, local) = recipe.split_local();
    let s = Sidecar {
        schema: SCHEMA,
        pipeline: SCHEMA,
        source: source.clone(),
        recipe: saved,
        export: export.clone(),
        unknown,
    };
    let written = match atomic_json(&primary, &s) {
        Ok(()) => primary,
        Err(e) => {
            let permission = e.downcast_ref::<std::io::Error>().is_some_and(|e| {
                e.kind() == std::io::ErrorKind::PermissionDenied || e.raw_os_error() == Some(30)
            });
            if !permission {
                return Err(e);
            }
            atomic_json(&backup, &s)?;
            backup
        }
    };
    let companion = if written == sidecar_path(raw) {
        local_path(raw)
    } else {
        fallback_at(&source, store, "local.json")
    };
    save_companion(&companion, source, local)?;
    Ok(written)
}
/// Writes the spots and masks, keeping bitmaps already saved; removes the file when
/// there are none.
fn save_companion(path: &Path, source: Identity, local: LocalEdits) -> Result<()> {
    let bitmaps = if path.exists() {
        parse_companion(path, &source)?.bitmaps
    } else {
        Default::default()
    };
    if local.is_empty() && bitmaps.is_empty() {
        if path.exists() {
            fs::remove_file(path)?;
        }
        return Ok(());
    }
    atomic_json(
        path,
        &Companion {
            schema: COMPANION_SCHEMA,
            source,
            local,
            bitmaps,
        },
    )
}

#[cfg(test)]
mod tests;
