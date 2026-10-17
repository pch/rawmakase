//! The one whole-window mode: F hides the workspace bar, the panels, the
//! filmstrip and the views' toolbars, so the photo, or the grid, fills the
//! window on its own backdrop. In the Library the window goes to the grid
//! first, and the next F gives it to the photo. F again, or Escape, brings the
//! chrome back. The window itself is left alone: no display is filled and
//! nothing about it changes, as Lightroom's screen modes do.
use super::Module;

/// What the mode did to the Library's view, for the caller to apply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum View {
    /// The view is as it was.
    Unchanged,
    /// The photo has the window: open the Loupe.
    OpenLoupe,
    /// The window is the grid's again: close the Loupe, which the mode opened.
    CloseLoupe,
}

/// The module whose chrome F hid, and what it gave the window to; the normal
/// window in `None`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Chrome {
    hidden: Option<Module>,
    /// In the Library, whether the photo, rather than the grid, has the window.
    photo: bool,
    /// Whether the Loupe was opened for the mode, so that leaving the mode
    /// closes it again and the grid is what the Library shows.
    opened: bool,
}

impl Chrome {
    /// Whether `module`'s chrome is hidden, leaving only the photo or the grid.
    pub(super) fn hidden(self, module: Module) -> bool {
        self.hidden == Some(module)
    }
    /// Whether `module`'s chrome is drawn.
    pub(super) fn shown(self, module: Module) -> bool {
        !self.hidden(module)
    }
    /// F: hide `module`'s chrome, or bring it back. The Library gives the window
    /// to the grid first and to the photo on the next press, which opens the
    /// Loupe, unless the photo is already the one shown. Only what is drawn
    /// changes: the panels keep the layout they are saved with.
    pub(super) fn toggle(&mut self, module: Module, loupe: bool) -> View {
        if self.hidden != Some(module) {
            self.hidden = Some(module);
            self.photo = module == Module::Develop || loupe;
            self.opened = false;
            return View::Unchanged;
        }
        if self.photo {
            return self.show();
        }
        self.photo = true;
        self.opened = true;
        View::OpenLoupe
    }
    /// Shows the chrome again, closing a Loupe this mode opened; Escape, or the
    /// mode being left behind on a module switch.
    pub(super) fn show(&mut self) -> View {
        self.hidden = None;
        self.photo = false;
        if std::mem::take(&mut self.opened) {
            View::CloseLoupe
        } else {
            View::Unchanged
        }
    }
    /// Enters `module`, whose chrome is drawn again: the mode belongs to the
    /// module it was turned on in, so G and D always land on a window with
    /// its bars, not on a hidden one.
    pub(super) fn entered(&mut self, module: Module) -> View {
        if self.hidden.is_some_and(|hidden| hidden != module) {
            return self.show();
        }
        View::Unchanged
    }
}
