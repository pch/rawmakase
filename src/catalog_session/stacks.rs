//! Folder stacks through the session: each change is written to the catalog
//! in one transaction (see `catalog::stacks`), then the photos' places in
//! their stacks are read again into `photos`. A stack changes no photo's
//! place, name or edit, so nothing else is read again and the structural
//! generation stays.
use super::{CatalogSession, Committed};
use crate::catalog::{StackChange, StackId, StacksChanged};
use anyhow::Result;

impl CatalogSession {
    /// Reads every photo's place in its stack again into `photos`.
    pub(crate) fn refresh_stacks(&mut self) -> Result<()> {
        self.changed();
        let places = self.catalog.stack_places()?;
        for photo in &mut self.photos {
            photo.stack = places.get(&photo.id).copied();
        }
        Ok(())
    }
    /// Makes `change` in the catalog, in one transaction that also reads
    /// the stacks it touches before and after, then reads the photos'
    /// places again. An error means nothing changed; once it returns, the
    /// change is saved, even when reading the places back failed.
    pub(crate) fn change_stacks(
        &mut self,
        change: &StackChange,
    ) -> Result<Committed<StacksChanged>> {
        self.changed();
        let value = self.catalog.change_stacks(change)?;
        Ok(Committed {
            value,
            listed: self.refresh_stacks(),
        })
    }
    /// Stacks the bursts of import `batches` as `stacks` and marks them done,
    /// in one transaction, as [`crate::catalog::Catalog::stack_imported_bursts`]
    /// does; then reads the photos' places again.
    pub(crate) fn stack_imported_bursts(
        &mut self,
        batches: &[i64],
        stacks: &[Vec<crate::catalog::PhotoId>],
    ) -> Result<Committed<Option<StacksChanged>>> {
        self.changed();
        let value = self.catalog.stack_imported_bursts(batches, stacks)?;
        Ok(Committed {
            value,
            listed: self.refresh_stacks(),
        })
    }
    /// Collapses or expands stacks `ids`.
    pub(crate) fn set_stacks_collapsed(
        &mut self,
        ids: &[StackId],
        collapsed: bool,
    ) -> Result<Committed<()>> {
        self.changed();
        self.catalog.set_stacks_collapsed(ids, collapsed)?;
        Ok(Committed {
            value: (),
            listed: self.refresh_stacks(),
        })
    }
}
