//! The name a control gives the History step it makes. The editor owns one,
//! and hands it to whatever draws or runs a control that edits the photo; the
//! control names the step as it changes the edit (the last name given wins),
//! and the edit frame passes the name on to History as it finishes
//! (`Editor::finish_edit_frame`), after ending any wheel scroll or
//! control-surface turn still going as steps of their own. A name given in a
//! frame that finishes no edit frame waits for the next one that does.
use crate::app::history::Step;

#[derive(Debug, Default)]
pub(in crate::app) struct StepName(Option<Step>);
impl StepName {
    /// Names the step being made `name`, showing `value`, in place of any
    /// name given before in this frame.
    pub(in crate::app) fn name(&mut self, name: String, value: String) {
        self.0 = Some(Step::new(name, value));
    }
    /// The name given, for History; none is left.
    pub(in crate::app) fn take(&mut self) -> Option<Step> {
        self.0.take()
    }
    /// Drops the name given: the edit it named changed nothing, and would
    /// otherwise name the next one.
    pub(in crate::app) fn discard(&mut self) {
        self.0 = None;
    }
    /// The name given so far, and the value it shows.
    #[cfg(test)]
    pub(in crate::app) fn given(&self) -> Option<(String, String)> {
        self.0.as_ref().map(|s| (s.name.clone(), s.value.clone()))
    }
}
