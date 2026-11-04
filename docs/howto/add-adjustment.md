# Adding a develop adjustment

Status: Current. A walk through every place a new develop setting touches, in the
order to make the change. Lightroom's Refine Saturation (`Recipe::curve_saturation`)
is the example throughout: a single value with a Lightroom key, rendered on the CPU
and the GPU. Read [architecture](../architecture.md) for the rules the change must
keep and the [glossary](../glossary.md) for the terms.

Many steps are enforced by the compiler: `Recipe` and `Effects` are taken apart
without `..` where a field must be placed (Copy Settings' groups, the stage cache's
keys, preset Amount's blend), so a new field fails to build until each one says
where it belongs. Treat each such error as a decision, not a formality.

## 1. The model parameter

In `crates/rawmakase-model/src/model/recipe.rs`, add the field to `Recipe`, with:

- a doc comment saying what it is in Lightroom's terms, its key and its scale
  (`crs:CurveRefineSaturation` / 100);
- its default in `Recipe::default`, equal to what Lightroom renders when the setting
  is absent;
- its range in `Recipe::validate`, the values a recipe may hold (which can be wider
  than the slider's: Refine Saturation keeps imported values up to 2).

If it is a slider, give it a `ParameterId` and a `Descriptor` in
`crates/rawmakase-model/src/model/params.rs`: label, slider range, valid range, step
and formatting. Sliders, the control socket, MIDI and History all read it there. A
control the socket and MIDI address by name is listed in `NAMED` in
`src/app/shell/commands/parameter.rs`.

If a panel switch covers it, reset it in that panel's `bypass` in
`crates/rawmakase-model/src/model/panels.rs`.

## 2. The saved format

Saved recipes are read by older releases too ([architecture](../architecture.md),
`model::saved_format`). Choose one, deliberately:

- **Omitted at its default.** `#[serde(default = …, skip_serializing_if = …)]`, so a
  recipe that doesn't use it is written as before and every older release reads it;
  one that does use it keeps the field through older releases, which keep unknown
  `Recipe` fields. Refine Saturation does this. Put the field on `Recipe`, not on
  `Effects`: `Effects` denies unknown fields, so older readers would reject it.
- **A new schema version.** When an older release would render the recipe wrongly
  without the setting (it changes the picture, and dropping it silently is worse
  than refusing), raise the version in `saved_format::schema_for` for recipes that
  use it, as AI Denoise and Lensfun lenses do: older releases then refuse the recipe
  and keep it unchanged.

Add the field to `SAVED_FIELDS` in `saved_format.rs`' tests only if it is written for
every recipe; that list changing is a compatibility decision. Saved slider values
must keep meaning what they meant: never reinterpret an existing field's scale.

## 3. XMP and Lightroom settings

Name the setting's Camera Raw key once, in
`crates/rawmakase-model/src/model/camera_raw.rs`: its key, Lightroom's units per
recipe unit (100 for most sliders) and the preset group Lightroom files it under
(`CURVE_REFINE_SATURATION`). Then, in `crates/rawmakase-interop/src/xmp/`:

- `apply.rs` reads it into the recipe with the range it accepts
  (`settings.assign(crs::CURVE_REFINE_SATURATION, …, 0., 2.)`). Lightroom catalogs'
  develop settings go through the same code (`lr_develop`), so imported catalogs get
  the setting too.
- `write.rs` writes it back for exports' embedded XMP, with its decimals and sign
  (`s.put(crs::CURVE_REFINE_SATURATION, …, 0, false)`), so reading the packet back
  reproduces the edit.
- `preset_write.rs`'s `group_of_key` finds its group in the table.

`xmp/key_table_tests.rs` freezes what every key reads and writes; a new key shows up
in its golden file, which you re-record with `RAWMAKASE_BLESS=1` and review.

Check the key, the scale and the default against Lightroom or Camera Raw, not
against another program. If the setting asks for Auto, it is measured through
`model::measures::PhotoMeasures`, which the engine implements.

## 4. Copy Settings, Sync and presets

In `crates/rawmakase-model/src/model/settings_groups.rs`, place the field in
`every_setting`: in a `SettingGroup` (Refine Saturation is `ToneCurve`), as derived
for the photo it is applied to, or as the photo's own. Copy it in the group's
transfer (`to.curve_saturation = from.curve_saturation`). Preset Amount blends it in
`crates/rawmakase-interop/src/presets/amount.rs`; say how it scales.

## 5. Rendering on the CPU

The CPU renderer is the reference (`crates/rawmakase-engine/src/develop/`). Find the
stage the setting belongs to as Camera Raw orders it ([scene tone
stage](../scene-tone-stage.md), [rendering quality](../rendering-quality.md)) and
apply it there: Refine Saturation applies with the point curve in
`pipeline/tone.rs`, through `color::curve::refine_saturation` in the model crate so
the CPU and the GPU port share one description.

Place the field in `stage_cache::stage_recipes`: a stage that reads a field it does
not key on would reuse stale results. Measure the setting against Camera Raw renders
([tests/corpus/README.md](../../tests/corpus/README.md)) and record the measurement
in the page for that stage.

## 6. Rendering on the GPU

The per-pixel stage has a WGSL port. In
`crates/rawmakase-engine/src/develop/pipeline/pixel_params.rs`, add a named slot and
its length to the `slot` list (`REFINE_SATURATION: 1`) and set it from the recipe
in the same function as its neighbours
(`p.set(slot::REFINE_SATURATION, …)`); `wgsl_prelude` turns each slot into a
`P_*` constant. Read it in `gpu/develop.wgsl` (`p(P_REFINE_SATURATION)`) at the same
point of the stage as the CPU. A recipe the port can't render returns `None` there
and renders on the CPU.

Add the setting to the recipes `gpu::tests::gpu_develop_matches_cpu_pixel_stage`
compares. GPU tests need a compute adapter and are ignored by default; run them
before sending the change:

```sh
cargo test --locked -p rawmakase-engine --lib gpu -- --ignored
```

## 7. The inspector

Each Develop panel has its own file in `src/app/develop/panels/inspector/panels/` (Refine
Saturation is in `tone_curve.rs`); `src/app/develop/panels/inspector.rs` draws them in order and
carries out the actions they return. Draw the slider with
`setting_slider(ui, ParameterId::…, &mut value, default)` in its panel, at
Lightroom's place, and reset it with the panel's reset. A panel edits the recipe it
is given; anything it can't do itself (Auto, an import, a dialog) it returns as an
action. Change the recipe
only through the editor's frame: the frame records one History step, marks the edit
for saving and turns on a switched-off panel.

## 8. History

`describe` in `src/edit_session/history.rs` names a step from what changed. Add the
field to the comparison for its panel, so a change to it reads as Lightroom names it
("Tone Curve").

## 9. Tests

Test observable behaviour at each boundary the setting crosses:

- **Saved format**: omitted at the default and round-tripped
  (`pipeline/tests.rs`' `refine_saturation_is_omitted_at_its_default_and_round_trips`),
  or, with a new schema version, refused by the version before.
- **XMP**: imports, writes back, and out-of-range values rejected
  (`xmp/tests.rs`' `refine_saturation_imports_and_out_of_range_values_are_rejected`).
- **Copy Settings**: the coverage test in `settings_groups/tests.rs` fails until the
  field has a kind; test the group's transfer.
- **Rendering**: what the setting does to pixels (`pipeline/tests.rs`), the GPU
  against the CPU (above), and an export carrying it
  (`crates/rawmakase-export/src/export/tests.rs`).
- **Colour corpus**: `cargo test --test color` must pass unchanged, or the change of
  snapshots must be intended and explained. Never re-bless references to make a
  change pass.

Then document the setting in its page under `docs/` and, if it differs from
Lightroom, in [parity gaps](../records/parity-gaps.md).
