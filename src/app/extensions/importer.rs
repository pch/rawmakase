//! Extensions' importers (`[[importer]]`, docs/extensions/manifest.md) from
//! the host's side: the importers Catalog › Import lists, and their
//! `import.run` requests. The import itself is a `catalog_import` job the
//! app runs (`app::catalog::extension_import`); the host only carries its
//! request: an inactivity deadline that each `progress` for it moves on
//! ([`Outcome::ImportProgress`](super::Outcome::ImportProgress)), and one
//! outcome however it ends ([`Outcome::Import`](super::Outcome::Import)).
use super::{Host, Pending};
use rawmakase_extensions::protocol::method::{AppRequest, ImportRun};

/// One of an enabled extension's importers, as the import menu lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::app) struct Offer {
    pub(in crate::app) extension: String,
    /// The importer's id within its extension.
    pub(in crate::app) importer: String,
    /// What it imports: the menu item's name.
    pub(in crate::app) name: String,
    /// The file extensions the file chooser offers.
    pub(in crate::app) extensions: Vec<String>,
    /// The extension's name, for what the app says.
    pub(in crate::app) extension_name: String,
}

impl Host {
    /// The enabled extensions' importers. Disabled extensions offer none.
    pub(in crate::app) fn importers(&self) -> Vec<Offer> {
        self.extensions
            .iter()
            .filter(|e| e.enabled())
            .flat_map(|e| {
                e.manifest.importers.iter().map(|i| Offer {
                    extension: e.id().to_string(),
                    importer: i.id.clone(),
                    name: i.name.clone(),
                    extensions: i.extensions.clone(),
                    extension_name: e.name().to_string(),
                })
            })
            .collect()
    }

    /// Whether `offer`'s extension is still enabled and still declares its
    /// importer: an import stops being wanted, until it publishes, the
    /// moment this is false.
    pub(in crate::app) fn importer_enabled(&self, offer: &Offer) -> bool {
        self.get(&offer.extension).is_some_and(|e| {
            e.enabled() && e.manifest.importers.iter().any(|i| i.id == offer.importer)
        })
    }

    /// Sends `import.run` to `offer`'s extension, starting its program
    /// first when it isn't running. Returns the ticket its
    /// [`Outcome::Import`](super::Outcome::Import) comes back with.
    pub(in crate::app) fn run_import(
        &mut self,
        offer: &Offer,
        params: ImportRun,
    ) -> Result<u64, String> {
        if !self.importer_enabled(offer) {
            return Err(format!("{} isn't available", offer.name));
        }
        let ticket = self.next_ticket;
        self.next_ticket += 1;
        self.send(
            &offer.extension,
            AppRequest::ImportRun(params),
            Pending::Import { ticket },
            None,
        )?;
        Ok(ticket)
    }

    /// Cancels import request `ticket` of `extension`, if it is still in
    /// flight: the program is told, and an answer it sends after is
    /// dropped.
    pub(in crate::app) fn cancel_import(&mut self, extension: &str, ticket: u64) {
        let Some(run) = self
            .extensions
            .iter_mut()
            .find(|e| e.id() == extension)
            .and_then(|e| e.run.as_mut())
        else {
            return;
        };
        run.queued
            .retain(|q| q.pending != Pending::Import { ticket });
        let sent = run
            .requests
            .iter()
            .find(|(_, p)| **p == Pending::Import { ticket })
            .map(|(id, _)| *id);
        if let Some(id) = sent {
            run.requests.remove(&id);
            run.session.cancel(id);
        }
    }

    /// Makes `import.run` time out after `inactivity` without progress, for
    /// tests.
    #[cfg(test)]
    pub(in crate::app) fn set_import_inactivity(&mut self, inactivity: std::time::Duration) {
        self.import_inactivity = inactivity;
    }

    /// Ends extension `id`'s program as a crash would, for tests of what an
    /// import does after its program answered.
    #[cfg(test)]
    pub(in crate::app) fn kill_program_for_tests(&mut self, id: &str) {
        if let Some(run) = self.get(id).and_then(|e| e.run.as_ref()) {
            run.session.kill_remaining();
        }
    }

    /// Whether extension `id`'s program runs now.
    #[cfg(test)]
    pub(in crate::app) fn program_running(&self, id: &str) -> bool {
        self.get(id).is_some_and(super::Extension::running)
    }
}
