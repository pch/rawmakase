# MIDI devices

MIDI control surfaces are the **MIDI** extension's, which ships with
RAWmakase, enabled, and starts with it. Its page is in **Preferences >
Extensions > MIDI**; **Preferences > Automation** keeps **Scripts and AI** and
links there. MIDI input is currently available on macOS and Windows; on Linux
the page says so and lists no inputs. Scripts and MCP also work on Linux and
do not need a MIDI device. Disabling the extension stops its devices; its
settings stay.

## Add a device

1. Connect your controller and open **Preferences > Extensions > MIDI**.
2. Under **Devices**, choose **Add Loupedeck+** for its supplied layout, or
   **Add Custom MIDI device** for an empty mapping. With several devices,
   **Edit** chooses the one shown under **Device settings**.
3. Give the device a name and choose its **MIDI input**. Use **Refresh inputs**
   after connecting another controller. Inputs are bound by their port identity;
   a disconnected controller is not silently replaced by another with the same name.
4. Expand **Control mappings**. Move a control to discover its CC or note number,
   or enter its number under **Control number** and choose **Add dial / slider**
   or **Add button**. Assign its parameter or action: a button's list holds
   common actions, and its **Action** field takes any other, which RAWmakase
   checks; one it doesn't run stays in the field with the reason under it and
   isn't saved.
5. Choose each dial/slider's **Control format**. Absolute controls map 0–127 to
   the parameter range, with 64 exactly zero for bipolar parameters such as
   Exposure and Contrast. Temperature in kelvin remains linear. Relative formats support 1/127, 65/63, or 1/65 encoders.
   The Loupedeck+ profile uses 1/127; custom controls default to absolute.

Each device has its own mappings, enabled state, connection status and held
modifiers. Add more devices with the same buttons, including several of one type.
One physical input can belong to only one enabled entry. Disable or remove the
old entry before assigning that input elsewhere. Ambiguous name matches are
rejected; choose a specific input to resolve them.

**Advanced connection** provides offline port-name matching, a MIDI-channel
filter (1–16 or all), relative turn speed and photo-dial detents. A photo dial
requires a relative format: one set to absolute does nothing, and the
extension's page says so on that dial. **Reset mappings to profile** affects only the
selected device; a custom profile resets to an empty mapping.

MIDI controls follow the selected mask in Masking where that parameter supports
local adjustments. Other controls affect the global edit. Edits are blocked
while a dialog or blocking operation is active. Photo navigation preserves
Library/Loupe; the photo dial ignores the Library grid. Continuous parameter
controls group into one undo step until a 400 ms pause, another control/source,
or a UI edit. Different devices never share held modifiers or an undo gesture.

## Loupedeck+ profile

| Control | CC | Does |
|---|---|---|
| Exposure, Blacks, Whites | 33, 34, 35 | the slider of that name |
| Saturation, Vibrance | 36, 37 | the slider of that name |
| Temperature, Tint | 38, 39 | Temp (in mireds, clockwise is warmer) and Tint |
| Highlights, Shadows | 40, 44 | the slider of that name |
| Clarity, Contrast | 45, 46 | the slider of that name |
| Faders P1–P8 | 17–24 | Color Mixer bands Red, Orange, Yellow, Green, Aqua, Blue, Purple, Magenta |
| Control Dial | 48 | previous / next photo in Develop and the Loupe |

Dials turn one point (0.01) per tick; Exposure 0.02 EV; Tint one unit.
The faders turn the channel the Color Mixer's Hue / Sat / Lum selector shows
(Hue when it shows "All"), or each band's gray mix in Black & White.
The Control Dial moves at once on the first tick of a turn, then once every
`photo_detent` (default 2) ticks.

| Button | Note | Does |
|---|---|---|
| Shift, Ctrl, Command, Alt | 66, 67, 68, 69 | held modifiers for the next button |
| Up / Left, Down / Right | 76 / 78, 77 / 79 | previous / next photo |
| P1–P5 | 80–84 | 1–5 stars |
| P6 | 85 | clear the rating (0) |
| P7, P8 | 86, 87 | pick (P), reject (X) |
| Export | 88 | Cmd+Shift+E |
| Copy, Paste | 92, 93 | Cmd+Shift+C / V (copy / paste settings) |
| Undo, Redo | 95, 96 | Cmd+Z, Cmd+Shift+Z |
| Screen Mode | 97 | J (clipping overlay) |
| Hue, Sat, Lum | 98, 99, 100 | show that channel in the Color Mixer |
| Clr/BW | 101 | Black & White on / off |
| Before After | 102 | `\` |
| C1 | 49 | Z (zoom) |
| C3–C6 | 51–54 | colour labels red, yellow, green, blue (6–9) |

Not bound: D1 (CC 41), D2 (CC 42), C2, L1–L3, Col, Fn, Tab, Custom Mode, and the
Texture and Dehaze sliders. [tools/loupedeck/controls.json](../tools/loupedeck/controls.json)
lists every control the device sends.

## How it reaches the editor

The extension's program (`rawmakase-midi`, beside the app) listens to each
enabled device on a thread of its own and translates its messages with the
device's mapping, its held modifiers included. It reports each connection to
the app (`adapter.device`) and sends what the controls mean as named
operations (`adapter.operate`): a parameter turned or set, the photo dial's
ticks, an action. The app applies them with the same rules as before:
adjustments follow the selected mask, the photo dial's detent is counted in
the app, and the Library grid ignores it before it counts. A device that
disconnects, or a program that ends, ends its gesture, and input still on its
way from before is dropped. If the program ends unexpectedly the app starts
it again; after three such ends in a row it stays stopped until **Retry** on
its page. See [the protocol](extensions/protocol.md) for the messages.

## Saved configuration and migration

`midi.json` in the extension's data folder
(`<data>/extension-data/rawmakase.midi/midi.json`) stores version 2 with a
`devices` array.
Each entry has a stable `id`, display `name`, `enabled`, `profile` (`loupedeck` or
`custom`), `port`, optional `port_id`, `exact`, optional `channel`, and `mapping`.
Mappings are stored in full, so a custom controller never inherits Loupedeck controls.
For example, a custom absolute exposure slider on CC 7 and an Undo button on note 40:

```json
{
  "version": 2,
  "devices": [{
    "id": 1,
    "name": "Desk controller",
    "enabled": true,
    "profile": "custom",
    "port": "Desk controller MIDI",
    "port_id": null,
    "exact": true,
    "channel": 1,
    "mapping": {
      "dials": {"7": "exposure"},
      "buttons": {"40": "undo"},
      "photo_dial": null,
      "photo_detent": 1,
      "default_encoder": "absolute",
      "encoders": {},
      "sensitivity": 1
    }
  }]
}
```

`encoders` optionally overrides the default by CC number. Formats are `absolute`,
`twos_complement`, `offset`, and `sign_magnitude`. Parameter names and button
operations are listed by `rawmakase control capabilities`. Buttons also support
legacy shortcut notation and held modifiers such as `hold:shift`.

Earlier releases kept `midi.json` in the data folder itself. On the
extension's first start RAWmakase copies it into the extension's folder,
unless a file is there already, and leaves the original, so an older release
still reads its own. A copy that fails is reported in the top bar and on the
extension's page, and the original stays.

An unversioned `midi.json` becomes one Loupedeck-profile device, retaining
port matching, mapping overrides, unassigned controls and photo-dial
settings. It is written as version 2 when settings are next changed. A fresh
installation starts with no devices. A file the extension can't read, or of a
newer version, is left intact and reported, in the top bar and on its page,
which then edits nothing until the file is fixed. A parameter or action the
app doesn't know is kept as written and refused when its control is used.

`automation.json` stores script/MCP access independently. An unversioned
`midi.json`'s `socket` preference, in the data folder, is honored only when
`automation.json` does not exist. Enabling scripts does not add a device, and
removing devices does not stop MCP.

## Scripts and AI

Enable **Scripts and AI > Allow local scripts and applications** in Preferences.
See [External control](automation.md) for the CLI and [MCP setup](mcp.md) for Codex
and other MCP clients. Semantic `set`, `turn` and named `action` commands are
independent of MIDI mappings. Legacy `dial`/`press` commands retain the bundled
Loupedeck layout in a separate compatibility adapter; they do not borrow any
physical device's mappings or modifier state.

## Mapping another device

[tools/loupedeck](../tools/loupedeck) has the scripts used to map the Loupedeck+:

```bash
python3 -m venv .venv && .venv/bin/pip install -r tools/loupedeck/requirements.txt
.venv/bin/python tools/loupedeck/capture.py     # print every message
.venv/bin/python tools/loupedeck/mapper.py      # operate a control, name it
.venv/bin/python tools/loupedeck/probe.py "Fader P1"   # report one gesture
```

They write `capture.jsonl`, `mapping.json` and `probes.jsonl` to the current
directory.

The built-in profile uses Loupedeck+ names. The extension's page also lists
configured controls and the last received unknown CC/note so other device
numbers can be mapped. The MIDI backend is available on macOS and Windows.
