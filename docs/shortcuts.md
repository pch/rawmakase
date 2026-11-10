# Keyboard shortcuts

Status: Current.

RAWmakase's shortcuts follow Lightroom's. Press **Cmd+/** (Ctrl+/ on Windows and
Linux) for the reference card of every shortcut, or open **Preferences >
Shortcuts** to change them. The card shows your keys too; a key you give a command
the card has no row for is listed under **Your keys**. In this page **Cmd** is ⌘ on a Mac and Ctrl elsewhere,
**Ctrl** is the Mac's Control key, and **Alt** is Option on a Mac.

## Changing a shortcut

Preferences > Shortcuts lists every command with its keys, grouped as the
reference card groups them. Type in the search field to find a command by what it
does or by its keys: `crop`, `cmd shift e` and `⌘⇧E` all work.

- Click a key to change it, then press the new key with its modifiers. Escape
  keeps the keys as they were; Delete (or Backspace) takes that key away; a
  click anywhere else stops waiting for the key.
- Click **+** to give a command another key.
- **Reset** gives one command its default keys back; **Reset All** does it for
  every command.
- Rows with a lock are fixed: Undo, Redo and Select All (the text-editing keys,
  and the Edit menu's on a Mac), and keys a control reads itself, such as Up and
  Down over a slider.

A key you press is checked before it is used. These are refused, with the reason
shown under the row:

- the keys the system and text fields keep: Cmd+C, Cmd+V, Cmd+X, Cmd+A, Cmd+Z,
  Cmd+Shift+Z, and Ctrl+Y on Windows and Linux; on a Mac also Cmd+Q, Cmd+W,
  Cmd+H and Cmd+M, and the menu's own Cmd+O (Open Catalog…) and Cmd+Shift+O (Add
  Folder…);
- a modifier that already means something for that command: Shift after a rating
  goes on to the next photo, so a rating's key can't hold Shift;
- a plain key for a command that works while you type in a field (Preferences,
  these shortcuts, the filters, dismissing the messages): it needs Cmd, or an F
  key;
- a key a control keeps where the command works: Up or Down in Develop, alone or
  with Shift or Alt, which nudge the slider under the pointer, and Space with any
  modifier, which pans while a tool is open;
- on Windows, the keys Windows turns into Cut, Copy or Paste before RAWmakase sees
  them (see [Windows](#windows)).

## Conflicts

Two commands may share a key when they never work in the same place: J cycles the
grid's cell style in the Library and shows clipping in Develop. Where both work, a
press of the key runs **neither**: the message line names both commands, and
Preferences marks both rows ("Conflicts with …"). Tick **Show conflicts only** to
list them alone. The default shortcuts have no conflicts.

A key of a tool or a Library view comes before the same key everywhere else, as X
swaps the crop's orientation in Crop instead of rejecting the photo. When you give a
key to a tool's command that Develop also uses, the tool's command runs while the
tool is open, and Develop's otherwise; Preferences says so under the row. Only a
command that can run at that moment counts: with a text field holding the keys,
Crop doesn't, so Preferences keeps Cmd+, even if Crop has it too. When two commands
share a key there and something else has it below them, none of them runs.

## Extensions' commands

An enabled extension's commands are listed under **Extensions** in Preferences
> Shortcuts, as `<extension>/<command>` (`dev.jane.analog/film-look`), and take
keys like any other command. A default key from the extension's manifest isn't
checked against the built-in keys when it is written, so it may conflict with
one; Preferences then marks both rows and a press of it runs neither, as above.
Your keys for an extension's command are kept in `keymap.toml` while the
extension is disabled or uninstalled (listed as not known to this release), and
take effect again when it comes back.

## The key map file

Your shortcuts are kept in `keymap.toml` in RAWmakase's data folder (shown at the
top of Preferences > Shortcuts):

- macOS: `~/Library/Application Support/RAWmakase/keymap.toml`
- Windows: `%APPDATA%\RAWmakase\keymap.toml`
- Linux: `~/.local/share/rawmakase/keymap.toml` (`$XDG_DATA_HOME/rawmakase`)

`RAWMAKASE_DATA_DIR` moves the whole data folder. The file holds only the
commands you changed, each with its whole list of keys; every other command keeps
the current release's defaults, so new default shortcuts arrive with an update:

```toml
version = 1

[bindings]
crop = ["R"]                  # Crop on R alone, not C
loupe = ["E", "Return"]
"compare" = []                # Before alone has no key
"panels:left" = ["Cmd+Alt+L"]
"label:green" = ["F9"]        # a command with no default key
```

A key is written as its modifiers, then the key, joined by `+`, as the
reference card writes it: `Cmd+Alt+Shift+K`, `Shift+F8`, `Cmd+/`, `Esc`,
`Return`, `Page Up`. `Cmd`, `Ctrl`, `Alt` and `Shift` are the modifiers
(`Command`, `Control` and `Option` read too; outside the Mac `Ctrl` is
`Cmd`). Keys are letters, digits, `F1` to `F35`, `Esc`, `Return`, `Tab`,
`Space`, `Delete`, `Backspace`, `Up`, `Down`, `Left`, `Right`, `Home`,
`End`, `Page Up`, `Page Down`, `Insert`, and the symbols the keyboard types:
`` ` `` `-` `=` `+` `[` `]` `{` `}` `\` `|` `;` `:` `'` `,` `.` `/`
`?` `!`. The commands' ids are [listed below](#command-ids).

Preferences edits the file in place: your comments, the order of its lines, and
entries this release doesn't know (a newer release's or an extension's) stay as
they are. Older releases don't read the file at all.

When the file can't be used, RAWmakase says so at start-up and uses the default
shortcuts:

- If it isn't valid TOML, isn't shaped as above, or was written by a newer
  release (a higher `version`), it is left exactly as it is, and Preferences >
  Shortcuts shows a banner and changes nothing. Fix it and restart, or click
  **Reset All**, which keeps it as `keymap.toml.broken` and starts a new one.
- If one command's entry is wrong (`crop = "R"` instead of `["R"]`, a key with a
  typo, a refused key, or any entry for a fixed command, even `undo = []`), that
  command keeps its defaults, Preferences lists the
  entry as skipped, and the entry stays in the file until you give the command
  new keys.

Each change is saved on top of what the file holds at that moment, so a second
RAWmakase running at the same time, or an entry added by hand, keeps its changes;
a RAWmakase picks up the other's keys when it saves one of its own, or at its next
start. If the file was changed meanwhile so that it no longer reads, or a newer
release rewrote it, the change isn't saved, the file is left as it is, and the
default shortcuts apply as above.

If saving the file fails, the shortcuts in effect are whatever the file holds
afterwards, and the message line says what happened.

## Keyboard layouts

A shortcut goes by the character its key types, so it follows your layout: on
Dvorak, G is the key that types G. Two exceptions keep the keys where Lightroom
has them:

- The number row goes by its place, so the ratings and labels work on layouts
  whose digit row types other characters unshifted (AZERTY's `&é"'(-è_çà`), and
  Shift+5 is still 5 and go on.
- The Basic panel's `. , = -` go by their place.

Many layouts need Shift or Alt (Option, AltGr) to type a symbol: German types `/`
with Shift+7. On a symbol key Shift and Alt are therefore ignored unless the
shortcut names them, so Cmd+/ works on German and French keyboards as it does on a
US one. When you capture a symbol key in Preferences, it is saved without the
Shift or Alt you needed to type it, and as the command reads keys: a Basic
panel key by its place, anything else by what it types. A key you write in the
file is read the same way. For the Basic panel's slider commands (select the
next or previous slider, raise or lower it) every key goes by its place, as their
default keys do. For any other command a key goes by what it types, except a
digit, which goes by its place; when a number-row key could be either (AZERTY
types `'` on the 4 key), the digit's command comes first.

## macOS menu

The menu bar shows a command's key when it holds Cmd, every place the command
works has it, and no other command shares it: Preferences… (Cmd+,), Export…
(Cmd+Shift+E, the Library's and Develop's) and Keyboard Shortcuts (Cmd+/) by
default, and whatever you give them. An item on a key you gave it waits
whenever the key itself wouldn't run the command, as while you type in a field,
a menu is open or a control has the focus. An item still on its own menu key
(Cmd+Shift+E, Cmd+, or Cmd+/) works as it always has, while you type too. Plain keys aren't shown in the menu but work as listed.
The File menu's Open Catalog…, Add Folder… and Close Window and the Edit menu's
Undo, Redo, Cut, Copy, Paste and Select All keep their keys.

## Windows

Windows' keyboard handling turns some keys into Cut, Copy or Paste before
RAWmakase sees them: Insert with Ctrl or Shift, and Delete with Shift, whatever
else is held. Ctrl+Shift+X and Ctrl+Alt+Shift+X arrive the same way as
Ctrl+Shift+Delete. Preferences refuses them as keys on Windows. Ctrl+Alt+X works.

Because of this, **Move to Trash's default Ctrl+Alt+Shift+Delete doesn't work on
Windows** ([#972](https://github.com/pch/rawmakase/issues/972)). Use Move to
Trash from the thumbnail's menu, or give it another key in Preferences > Shortcuts.

## Command ids

The ids the key map file uses, which are also automation's and MIDI's action
names where they have one. Fixed commands keep their keys.

| Id | Command |
| --- | --- |
| `undo` | Undo (fixed) |
| `redo` | Redo (fixed) |
| `rating:0` | No stars |
| `rating:1` | One star |
| `rating:2` | Two stars |
| `rating:3` | Three stars |
| `rating:4` | Four stars |
| `rating:5` | Five stars |
| `pick` | Pick |
| `reject` | Reject |
| `unflag` | Unflag |
| `label:red` | Label red |
| `label:yellow` | Label yellow |
| `label:green` | Label green |
| `label:blue` | Label blue |
| `label:purple` | Label purple |
| `label:none` | No label |
| `toggle_label:red` | Toggle the red label |
| `toggle_label:yellow` | Toggle the yellow label |
| `toggle_label:green` | Toggle the green label |
| `toggle_label:blue` | Toggle the blue label |
| `toggle_label:purple` | Toggle the purple label |
| `copy` | Copy settings |
| `paste` | Paste settings |
| `paste_previous` | Paste settings from previous photo |
| `sync` | Sync settings |
| `reset` | Reset all settings |
| `auto_tone` | Auto settings |
| `auto_white_balance` | Auto white balance |
| `curve:linear` | Tone curve: linear |
| `curve:medium_contrast` | Tone curve: medium contrast |
| `curve:strong_contrast` | Tone curve: strong contrast |
| `export_dialog` | Export… |
| `export_previous` | Export with previous |
| `toggle:bw` | Convert to black & white or back to color |
| `treatment:bw` | Black & white |
| `treatment:color` | Color |
| `compare` | Before alone |
| `camera_thumbnail` | Camera thumbnail (the embedded JPEG) |
| `clipping` | Show shadow and highlight clipping |
| `zoom` | Toggle zoom / fit |
| `fit` | Fit |
| `crop` | Crop & Straighten |
| `remove` | Spot removal |
| `white_balance` | White balance selector |
| `mask` | Masking |
| `mixer:hue` | Color mixer: hue |
| `mixer:sat` | Color mixer: saturation |
| `mixer:lum` | Color mixer: luminance |
| `library` | Library |
| `develop` | Develop |
| `loupe` | Open the Loupe |
| `next` | Next photo |
| `previous` | Previous photo |
| `panels:sides` | Hide / show the side panels |
| `panels:all` | Hide / show the side panels and filmstrip |
| `panels:left` | Left panel |
| `panels:right` | Right panel |
| `panels:filmstrip` | Filmstrip |
| `panels:presets` | Fold / open Presets and History |
| `chrome` | Hide all but the photo |
| `messages:dismiss_all` | Dismiss the messages in the top bar |
| `chrome:show` | Leave the whole-window mode |
| `virtual_copy` | Create virtual copy |
| `preferences` | Preferences |
| `shortcuts` | Keyboard shortcuts |
| `shortcuts:clear_search` | Clear the shortcuts search |
| `flag:raise` | Raise the flag |
| `flag:lower` | Lower the flag |
| `toggle_pick` | Toggle pick |
| `rating:lower` | Lower the rating |
| `rating:raise` | Raise the rating |
| `filters` | Turn the filters off / on |
| `library:compare` | Compare the active photo with the next |
| `library:survey` | Survey the selected photos |
| `close_view` | Back to the grid |
| `grid:cell_style` | Cycle the grid cell style |
| `grid:row_up` | Move the active photo a row up |
| `grid:row_down` | Move the active photo a row down |
| `grid:page_up` | Move the active photo a page up |
| `grid:page_down` | Move the active photo a page down |
| `first_photo` | First photo |
| `last_photo` | Last photo |
| `quick:toggle` | Add to or take out of the Quick Collection |
| `quick:show` | Show the Quick Collection |
| `quick:clear` | Clear the Quick Collection |
| `delete` | Move to Trash, or remove from the catalog |
| `remove_from_catalog` | Remove from the catalog |
| `delete_rejected` | Delete the rejected photos |
| `trash` | Move to Trash |
| `rename` | Rename the photo, or the selected photos |
| `stack:group` | Group into stack |
| `stack:unstack` | Unstack |
| `stack:toggle` | Collapse / expand the stack |
| `stack:to_top` | Move to the top of the stack |
| `select_all` | Select all (fixed) |
| `select_none` | Select none |
| `deselect_active` | Deselect the active photo |
| `info` | Cycle photo info |
| `compare_view:previous_candidate` | Previous candidate |
| `compare_view:next_candidate` | Next candidate |
| `compare_view:make_select` | Make the candidate the select |
| `compare_view:swap` | Swap select and candidate |
| `zoom:100` | Zoom to 100% |
| `zoom:in` | Zoom in |
| `zoom:out` | Zoom out |
| `magnifier` | Magnifier over the photo |
| `before_after:copy_to_before` | Copy After to Before |
| `before_after:copy_to_after` | Copy Before to After |
| `before_after:swap` | Swap Before and After |
| `before_after:left_right` | Before and after, left and right |
| `before_after:top_bottom` | Before and after, top and bottom |
| `before_after:split` | Before and after, split |
| `match_exposures` | Match total exposures |
| `new_preset` | New Develop Preset |
| `targeted:tone_curve` | Targeted adjustment: Tone Curve |
| `targeted:hue` | Targeted adjustment: Hue |
| `targeted:saturation` | Targeted adjustment: Saturation |
| `targeted:luminance` | Targeted adjustment: Luminance |
| `targeted:black_white` | Targeted adjustment: B&W mix |
| `reference_view` | Reference View |
| `guided_upright` | Guided Upright: draw guides |
| `tool:finish` | Finish the crop or the guides |
| `tool:close` | Put the open tool away |
| `tool:delete` | Delete the selected spot, guide, red eye or mask |
| `brush:smaller` | Smaller brush or red eye (Shift: feather) |
| `brush:larger` | Larger brush or red eye (Shift: feather) |
| `crop:swap_orientation` | Swap the crop's orientation |
| `crop:aspect_lock` | Lock or unlock the crop's aspect |
| `crop:next_overlay` | Next crop overlay |
| `crop:turn_overlay` | Turn the crop overlay |
| `remove:new_source` | New source for the selected spot |
| `remove:hide_pins` | Hide spot pins |
| `remove:visualize` | Visualize spots |
| `mask:back` | Stop aiming, drop the mask not yet drawn, or close Masking |
| `mask:overlay` | Show or hide the mask overlay |
| `mask:switch_brush` | Switch between brushes A and B |
| `mask:new_brush` | New brush mask |
| `mask:new_linear` | New linear gradient mask |
| `mask:new_radial` | New radial gradient mask |
| `mask:new_color_range` | New color range mask |
| `basic:next` | Select the next Basic slider |
| `basic:previous` | Select the previous Basic slider |
| `basic:increase` | Raise the selected Basic slider (Shift: more, Option: less) |
| `basic:decrease` | Lower the selected Basic slider (Shift: more, Option: less) |
