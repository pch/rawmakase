//! What changing a develop setting implies for the rest of the recipe. The
//! panels, the control socket and MIDI all change settings this way, so the side
//! effects of a change live in one place rather than in each of them.
use super::params::ParameterId;
use crate::camera_data::Metadata;
use crate::model::recipe::Recipe;

/// Changes setting `id` with `change`, then brings the rest of the recipe in line
/// as [`setting_changed`] says: the one way to edit a develop setting. Returns
/// what `change` returns.
pub fn change_setting<T>(
    r: &mut Recipe,
    id: ParameterId,
    photo: Option<&Metadata>,
    change: impl FnOnce(&mut Recipe) -> T,
) -> T {
    let previous = *id.value_mut(r);
    let out = change(r);
    setting_changed(r, id, previous, photo);
    out
}

/// Brings the recipe in line after setting `id` changed from `previous`:
/// a new Temp or Tint recomputes the white balance multipliers for `photo`, which
/// then no longer come from Auto.
pub fn setting_changed(r: &mut Recipe, id: ParameterId, previous: f32, photo: Option<&Metadata>) {
    let current = *id.value_mut(r);
    match id {
        ParameterId::Temperature | ParameterId::Tint => {
            if current != previous
                && let Some(m) = photo
            {
                r.update_wb(m);
                r.auto_white_balance = None;
            }
        }
        _ => {}
    }
}

/// Turns a switched-off panel back on when the change from `before` touched only
/// its settings, as Lightroom does, so the change shows: a slider in it, or an edit
/// made another way (B&W Auto, Clear Guides, the fringe picker, a swatch).
pub fn turn_on_edited_panel(before: &Recipe, after: &mut Recipe) {
    use crate::model::panels::{Panel, PanelState};
    let edited = Panel::ALL.into_iter().find(|panel| {
        before.panels.state(*panel) == PanelState::Off
            && after.panels.state(*panel) == PanelState::Off
            && panel.holds_change(before, after)
    });
    if let Some(panel) = edited {
        after.panels.set(panel, PanelState::On);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_change_to_one_switched_off_panel_turns_it_on() {
        use crate::model::panels::{Panel, PanelState};
        let mut before = Recipe::default();
        before.panels.set(Panel::Detail, PanelState::Off);
        let mut after = before.clone();
        after.sharpening = 0.5;
        turn_on_edited_panel(&before, &mut after);
        assert_eq!(after.panels.state(Panel::Detail), PanelState::On);
        // Not when the change reaches beyond it, as a preset's does.
        let mut after = before.clone();
        after.sharpening = 0.5;
        after.exposure = 1.;
        turn_on_edited_panel(&before, &mut after);
        assert_eq!(after.panels.state(Panel::Detail), PanelState::Off);
    }

    #[test]
    fn white_balance_waits_for_a_photo_and_a_change() {
        let mut r = Recipe {
            auto_white_balance: Some([5000., 0.]),
            ..Recipe::default()
        };
        let before = r.temperature;
        setting_changed(&mut r, ParameterId::Temperature, before, None);
        assert!(r.auto_white_balance.is_some());
        r.temperature += 100.;
        setting_changed(&mut r, ParameterId::Temperature, before, None);
        assert!(
            r.auto_white_balance.is_some(),
            "no photo, nothing to recompute"
        );
    }
}
