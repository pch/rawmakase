//! Drawing primitives with no feature of their own: design tokens, icons, the theme and widgets.
pub(in crate::app) mod design;
pub(in crate::app) mod icons;
pub(in crate::app) mod theme;
#[cfg(test)]
pub(in crate::app) mod ui_snapshot;
pub(in crate::app) mod widgets;
