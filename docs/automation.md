# External control

RAWmakase can be controlled locally by scripts and agents while its desktop app
is running. MIDI devices, the control client and the JSON protocol use named
application operations and the same editor workflows for history, treatment,
metadata, navigation, saving and rendering.

Enable **Preferences > Automation > Allow local scripts and applications** first.
External control is **off by default**. MIDI is configured separately and does not
require external control to be enabled.

## Command-line client

The client is included in the app executable:

```sh
rawmakase control capabilities
rawmakase control photos DSCF0042
rawmakase control open --id 17
rawmakase control state
rawmakase control set exposure 0.5
rawmakase control action treatment:bw
rawmakase control save
rawmakase control preview /absolute/path/preview.jpg
rawmakase control export /absolute/path/finished.tif
```

On macOS the executable is inside `RAWmakase.app/Contents/MacOS/rawmakase`.
The optional standalone client, `rawmakase-ctl`, has the same arguments without
`control`; build it from this repository with
`cargo install --locked --path tools/rawmakase-ctl`.

`open` waits for decoding by default. `--no-wait` returns when opening starts;
`--timeout` sets the wait limit. `photo next`, `photo prev` and `develop` initiate
navigation; inspect `loaded` before editing. `photos` returns IDs, filenames,
paths and metadata, with `--offset` and `--limit` pagination. It searches the
catalog's filenames independently of the Library's current filters. `search`
changes the Library search itself.

`set` uses displayed units: Exposure in EV, global Temperature in kelvin, global
Tint in tint units, `straighten` (alias `angle`, the Crop panel's Angle) in
degrees from −45 to 45, other basic and mixer parameters in percent. `turn`
moves Straighten 0.1° per tick and has no mask scope. Values are
clamped to their supported ranges. `capabilities` lists these ranges and whether
a parameter supports masks. Global Exposure is the exception: `set` accepts
±8 EV, the most a saved edit may hold, while its listed range is the slider's
±5. A `turn` never moves a value further outside the listed range, and moves one
already outside it (an imported +6 EV, say) one step at a time toward it. Mixer channels are explicit: `band3.sat`,
`band3.hue`, `band3.lum`, `band3.gray`. Bare `band3` is a device mapping only.

Develop presets, built-in and imported, are listed by `presets` and applied by
`preset`, as a click in the Presets panel applies them: one History step, the
settings that don't fit the photo skipped and reported in `applied.skipped`.
`issue` in the list says in advance why a preset doesn't fully fit the open
photo. A name matches the panel's in any case, or part of one when only one
preset has it; an ambiguous name fails and lists the matches, so add a `group`
or use the `id`. Ids name presets in the loaded list only, and both commands
return `not_ready` while the list is still loading at startup. `state.preset` is
the listed preset last applied to the open photo.

```bash
rawmakase control presets --group "My Presets"
rawmakase control preset "Ett B&W New"
```

Point curves are available through the `curve` protocol command and MCP tools.
`state.tone_curve` includes RGB and individual color-channel points. Inputs and
outputs range from 0 to 1; inputs must increase by at least 0.00049. Invalid curves
fail without changing the recipe. Built-in presets are `curve:linear`,
`curve:medium_contrast` and `curve:strong_contrast`.

Actions include `undo`, `redo`, `rating:3`, `pick`, `reject`, `label:red`,
`treatment:bw`, `treatment:color`, `copy`, `paste`, `auto_tone`, `auto_white_balance`, `export_dialog`
and `export_previous`; see `capabilities` for the complete list. Actions do not
simulate typing and do not depend on text focus or physical modifier keys.
Device rating/flag buttons retain Shift and Photo > Auto Advance behavior; label
buttons toggle their labels. Named `label:red` sets a label idempotently, while
`toggle_label:red` toggles it explicitly.

Panel actions hide or show the open module's panels, as the keyboard does:
`panels:sides` (Tab), `panels:all` (Shift+Tab, with the filmstrip and status
bar), and `panels:left`, `panels:right` and `panels:filmstrip` (F7, F8, F6).
`panels:presets` folds or opens Develop's Presets and History panel, as its rail
does. They need no photo. `state.panels` reports each panel of the open module as
`shown` or `hidden`. A device button can also be set to `tab`, `shift+tab` or
`f6`–`f8`.

`chrome` hides or shows everything but the photo, as F does: the workspace bar,
the panels, the filmstrip and the views' toolbars. It needs no photo. In the
Library the grid takes the window first and, applied again, the photo (the
Loupe) does; in Develop the photo has it from the first application. Applied
once more, or after a module switch, the chrome and the panels are back. The
window itself is not changed.

Dialog actions report that the action was applied, but require user interaction.
`auto_running` and `treatment_pending` describe asynchronous editor work.
Prefer the explicit `export` command for unattended output.

## Guard the target

By default a semantic parameter command edits the **global controls of the open
photo**, regardless of which panel or mask is selected. Device-level commands
follow the hardware mapping, including the active mask.

Scripts sharing the app with a person should read `state` and pass its identity:

```sh
rawmakase control --photo-id 17 --generation 8 --revision 23 set exposure 0.5
rawmakase control --photo-id 17 --generation 8 --revision 24 --mask 0 set temperature 35
```

These numbers are examples: use values returned by your running app. A mismatch
returns `stale_target` without applying the edit. A mask is an index into `masks`
in the state response. Since masks have no persistent IDs, **generation and
revision are mandatory for mask edits**. Each edit may advance the revision;
use `--state` to obtain the next target guards from the same reply.

Mask Temperature and Tint use local −100…100 units, rather than global kelvin
and tint units. Local Exposure is −4…4 EV. Unsupported local parameters fail;
they never fall back to changing the global recipe.

Mutations fail while a dialog or blocking activity is active. Parameter edits
also fail in Library or while the photo is loading. Read-only state, discovery,
catalog queries and output-job polling remain available.

## Preview and export completion

`preview` and `export` capture the decoded photo, recipe and its generation and
revision before starting a background job. Later UI edits do not change that
output. Preview defaults to a 1600-pixel long edge; use `--max-edge` to choose a
size. Export defaults to full size. Both accept JPEG, TIFF and PNG paths and use the
existing export pipeline. They never overwrite an existing file or change saved
Export dialog settings.

The client waits for file publication by default. For asynchronous operation:

```sh
rawmakase control preview /absolute/path/preview.jpg --no-wait
rawmakase control job 1
```

The job result contains `job_id`, `status`, `path`, `generation`, `revision` and
`progress`. Only `completed` confirms a published file; `failed` includes an
error. Two output jobs may run concurrently. The latest 32 jobs are retained;
IDs are scoped to the running app session. If a client stops waiting, the job
continues and can be read with `job`. `wait` with `until: "job"` answers as soon as
a job finishes, so a client need not poll. Closing the app cancels its remaining
jobs.

`save` waits for the current Develop edit to be saved to its catalog, or returns
an error. Protected and uncataloged edits cannot report save success. Ordinary edit
replies confirm the application transaction, including history and scheduling
rendering; they do **not** imply a finished render or durable save. `save_state`
is `saved`, `pending`, `saving`, `failed` or `protected`.

## JSON protocol (version 1)

When enabled, the app binds an ephemeral loopback TCP port and writes
`control.json` in its data directory with `protocol`, `port`, `token` and `pid`.
The token uses OS randomness. The connection file is owner-readable/writable on
Unix. Treat it as a credential. Any local program able to read it can control
the app; there is no remote network service.

One connection carries one newline-terminated JSON request and reply. For example:

```json
{"protocol":1,"request_id":"edit-42","token":"TOKEN","cmd":"set","param":"exposure","value":0.5,"target":{"photo_id":17,"generation":8,"revision":23}}
```

A successful reply includes `protocol`, the echoed `request_id`, `ok: true`,
`status: "applied"`, the state after this request, and a command-specific `result`.
For `export`/`preview`, `result.status` tracks the output job separately.
A failure uses `ok: false`, a stable `code` and a human-readable `error`.
Every request has its own reply; concurrent clients do not share a result slot.
Commands execute in queue order. The UI may still change between requests:
use target guards for a sequence that must refer to the same edit.

| Command | Fields | Result |
| --- | --- | --- |
| `state` | — | State snapshot |
| `capabilities` | — | Protocol, actions, parameters, ranges and scope support |
| `photos` | optional `query`, `offset`, `limit` (maximum 500) | Catalog entries and total |
| `presets` | optional `group` | Develop presets: `id`, `name`, `group`, `builtin`, `issue` |
| `preset` | `id`, or `name` with optional `group`; optional `target` | `applied` preset and skipped settings |
| `set` | `param`, finite `value`, optional `target` | Post-edit state |
| `curve` | `channel`: `rgb`, `red`, `green` or `blue`; `points`: 2–32 normalized input/output pairs; optional `target` | Post-edit state |
| `turn` | `param`, integer `ticks` (−1000…1000), optional `target` | Post-edit state; grouped gesture |
| `action` | named `action`, optional `target` | Post-action state |
| `open` | catalog `id` or `name` | `opened` identity; `wait` for `loaded` |
| `search` | `text` | Applied Library query |
| `module` | `module`: `develop` or `library` | Post-action state |
| `photo` | `step`: −1 or 1 | Selection change in Library; opened identity in Develop |
| `save` | optional `target` | `saved: true` |
| `preview`, `export` | absolute `path`, optional `max_edge`, optional `target` | Captured output job |
| `job` | `job_id` | Output status |
| `build_previews` | optional `kind`: `standard` (the default) or `one_to_one` | `queued` and `skipped` (offline or not RAW) counts for the selection, as the thumbnail menu's Build Standard-Sized Previews; state's `building_previews` (`done`, `total`) shows progress while they build |
| `wait` | `until`: `loaded` with `photo_id`, or `job` with `job_id`; optional `timeout_ms` (1…5000, default 1000) | State once the photo is loaded (or replaced by another) or the job finished, status `applied`; or at `timeout_ms`, status `timed_out` |
| `cc` | integer `cc`, `value` (0…127) | Legacy device mapping |
| `note` | integer `note` (0…127), `press`: `click`, `down`, `up` | Legacy device mapping |

Raw socket `cc` and `note` requests use the fixed Loupedeck+ default mapping,
independent of configured MIDI devices and their custom mappings. Their held
modifiers are separate from physical devices. Use semantic `set`, `turn` and
`action` commands for device-independent scripting.

The legacy `action` shortcut notation (`cmd+shift+z`, etc.) resolves to supported
named actions. Unknown shortcuts fail explicitly. Legacy requests may omit
`protocol`; explicit unsupported versions are rejected. `target` accepts only
`photo_id`, `generation`, `revision`, and `mask`. In Library, semantic rating,
flag and label actions require an explicit `target.photo_id` from `photos`;
they never act on an unguarded selection. Physical device controls retain their
selection-based behavior. `photo` and named `next`/`previous` preserve the current
Library grid or Loupe view, or navigate the open photo in Develop.

Consecutive `turn` commands group into one undo step only for the same transport,
parameter, mask, photo generation and mixer channel within 400 ms. A UI edit or a
different command ends that group. Auto tone and white-balance commands return
`busy` if an automatic adjustment is already running.

Common error codes include `invalid_request`, `unsupported_protocol`,
`unauthorized`, `busy`, `stale_target`, `target_required`, `no_document`,
`not_ready`, `unsupported_parameter`, `not_editable`, `protected`, `save_failed`,
`already_exists` and `unknown_job`. `open` returns `not_editable` for a photo whose
file is offline, or a JPEG, PNG or TIFF Develop can't edit (a CMYK JPEG, say);
any other JPEG, PNG or TIFF opens as a RAW does. On a JPEG, PNG or TIFF,
`temperature` and `tint` move its relative Temp and Tint (−100 to 100).

The request limit is 64 KiB, the input queue is bounded and at most 16 socket
clients are served concurrently. If a request has not started within three
seconds, it is cancelled and returns `cancelled`: it will not execute later.
If execution already started but its reply times out, `outcome_unknown` means
inspect state before retrying, especially for relative or toggle operations.
There are no automatic mutation retries.

Commands currently execute during desktop frames. A minimized/suspended window
may not process them; queued requests then expire safely. This is live desktop
control, not a headless daemon. For unattended rendering without a GUI, use
`rawmakase render` and its recipe options.

## Configuration and compatibility

`automation.json` in the data directory stores `{"protocol":1,"socket":true}`.
MIDI device instances and their independent mappings stay in version 2
`midi.json`. Legacy single-device files migrate to one device entry. An explicit legacy `socket` choice in
`midi.json` is read when no `automation.json` exists; saving Automation preferences
moves that setting to `automation.json`. A fresh installation never enables the
socket implicitly. Turning it off removes the connection file and rejects queued
requests from that listener.

The `rawmakase control` and standalone client share their implementation and the
app's data-directory policy (`RAWMAKASE_DATA_DIR` or the platform default).
`--data-dir` can point the client to an app using an isolated data directory.

An agent can invoke the CLI directly or implement an adapter over this protocol.
The bundled `rawmakase mcp` adapter exposes editing tools and preview images over
stdio without knowing MIDI numbers or keyboard shortcuts. See [MCP setup and
editing workflow](mcp.md).
