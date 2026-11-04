//! What names files by a template, wherever files are named: the menu of
//! templates (`menu`), the Filename Template Editor (`editor`) with its
//! Preset menu of saved templates (`presets`), and the background read of
//! what names need from the photos' files (`read`).
//! Export and Rename Photos each own an editor and a read for their dialog;
//! none of these knows who opened it.
pub(in crate::app) mod editor;
pub(in crate::app) mod menu;
pub(in crate::app) mod presets;
pub(in crate::app) mod read;
