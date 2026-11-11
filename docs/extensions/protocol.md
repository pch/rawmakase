# Extension protocol reference

Version 1 (`PROTOCOL` in `crates/rawmakase-extension-protocol`). What the app
and an extension's program say to each other. The SDK
(`crates/rawmakase-extension`) speaks it for you; this page is for reading logs,
writing an SDK in another language, and reviewing changes. The types are in
`message.rs`, `method.rs` and `controls.rs` of the protocol crate; where this
page and they differ, they are right. Viewer input is in `viewer.rs` and buffer
descriptors in `buffer.rs`.

## Transport

- The app creates the channel before starting the program, and the program
  attaches to it once:
  - **Linux, macOS**: one end of a Unix socket pair, as file descriptor 3,
    named in `RAWMAKASE_EXTENSION_FD=3`.
  - **Windows**: a named pipe, `\\.\pipe\rawmakase-ext-<app pid>-<random>`, named
    in `RAWMAKASE_EXTENSION_PIPE`. The app creates it as the first and only
    instance of its name and refusing remote clients, and accepts only a
    client whose process id is the program's. The program opens it for
    overlapped I/O, so reading and writing don't wait on each other.
- Stdin is the null device; stdout and stderr go to
  `<data>/extension-data/<id>/log.txt`, rotated at 1 MiB.
- The program runs in its working folder in its package, with every process it
  starts in one unit the app ends whole: a process group on Linux and macOS, a
  job object that kills its processes when closed on Windows (the app starts
  the program suspended and puts it in the job before it runs). When the
  program exits, the rest of its unit is ended at once.
- **Framing**: one JSON object per line, UTF-8, `\n`-terminated, at most
  1 MiB (`MAX_LINE`) without the newline. A longer line is skipped to its
  newline and reported; the session goes on. Bulk data never travels in JSON.

### API mode

The app starts the program with `RAWMAKASE_EXTENSION_API` set to its
manifest's `api` (`ENV_API`), and the SDK reads it before it reads the first
line, so both sides know the rules from the start. Without it, a program
serves API 1. The SDK refuses to serve an `api` it doesn't know, and the
program exits.

Both modes follow the recovery rules above for now: a malformed request is
answered `invalid_request`, an oversized line is skipped, and the session goes
on. API 2 adds handle passing in the next protocol change; from then on, a
framing fault in an API 2 session (a line short of its handles, a malformed or
oversized line) ends the program, since the app is the only sender of handles
and a fault there means corruption, not input to tolerate.

### Reserved: handles

Not built yet; specified so that buffers fit without a protocol change. Handle
passing arrives in the next protocol change, with API 2:

- A message carrying handles names their count, `"handles": n`. On Linux and
  macOS the handles travel as `SCM_RIGHTS` ancillary data on the `sendmsg`
  that holds the line's first byte, and the reader (`recvmsg`) attaches them to
  that line. On Windows the app duplicates each handle into the program
  (`DuplicateHandle`, holding the program's process handle) and the message
  carries their values. Readers ignore the field today; nothing sends it.
- Only the app sends handles, and it allocates every buffer, in both
  directions.

### Buffers

A pixel buffer in shared memory is described in the request that carries it.
The pixels never travel in JSON. Until handle passing arrives, a descriptor
only describes; `handle` will index the message's handles.

```json
{"handle":0,"bytes":3686400,"width":1280,"height":960,"stride":3840,
 "channels":3,"sample":"u8",
 "meaning":{"color":{"primaries":"srgb","white":"d65","transfer":"srgb","alpha":"none"}},
 "region":{"space":"image_frame","rect":[0,0,1,1],"scale":0.25,"overlap":0},
 "access":"read"}
```

| Field | Meaning |
| --- | --- |
| `handle` | The index of the buffer's handle among the message's handles |
| `bytes` | The mapping's length |
| `width`, `height` | Pixels |
| `stride` | Bytes from one row's start to the next's |
| `channels` | 1 to 4, interleaved: 1 for `coverage`, 3 for colour, 4 for colour with alpha |
| `sample` | `u8`, `u16` or `f32`; `f32` may hold values below 0 and above 1 |
| `meaning` | `"coverage"` (how much of each pixel a mask covers, 0 to the sample's maximum), or `{"color":{…}}` with `primaries` (`srgb`, `rec2020`, `pro_photo`, `adobe_rgb`, `camera`), `white` (`d65`, `d50`, `camera`), `transfer` (`linear`, `srgb`, `{"gamma":g}`) and `alpha` (`none`, `straight`, `premultiplied`) |
| `region` | Where the pixels lie: `space` (`image_frame`, `developed`, `output`, `camera`), `rect` (`[x0, y0, x1, y1]`, normalised to that space), `scale` (buffer pixels per full-resolution pixel), `overlap` (pixels beyond `rect` on each side, for tiles) |
| `access` | `read` (the app filled it) or `write` (the program fills it) |

**Layout.** Samples are little-endian and channels interleaved. Rows run top to
bottom, row `r` starting at byte `r * stride`. Pixel centres map onto `rect` as
the image frame maps them: pixel column `i` sits at `u` where
`i = u * width - 0.5`, and likewise for rows. `overlap` pixels lie outside the
rect on each side and count in `width` and `height`; beyond the photo's edge
they repeat the edge pixel. `camera` primaries and white carry camera RGB
before white balance, for raw patches.

**Checks.** A receiver refuses a descriptor, with `invalid_request`, unless:

- it has at least one pixel, and as many channels as its meaning;
- `stride >= width * channels * sample size`;
- `stride * height <= bytes <= 256 MiB`, computed without overflow;
- `rect` is finite, ordered and non-empty within [0, 1], `scale` finite and
  positive, and `overlap` leaves at least one pixel of the rect;
- a gamma is finite and positive;
- every value is one it knows. Unlike parameters, a descriptor refuses fields
  it doesn't know, since a later field may change how the bytes are laid out.

A request's buffers hold at most 256 MiB together (`MAX_REQUEST_BYTES`), and
one extension's requests in flight at most 512 MiB (`MAX_EXTENSION_BYTES`),
counting requests cancelled but not yet answered.

### Viewer input

What the user points at on the photo, in its image frame (the camera-oriented
default crop, before lens correction, Transform and crop), normalised to
[0, 1] from the top left:

| `kind` | Fields |
| --- | --- |
| `point` | `x`, `y` |
| `points` | `clicks`: `[{"x":…,"y":…,"positive":true}]` |
| `box` | `x0`, `y0`, `x1`, `y1` |
| `prompt` | `clicks`, and optionally `bounds`: `{"x0":…,"y0":…,"x1":…,"y1":…}` |

Every coordinate is finite and in [0, 1], a box is ordered and not empty, and
there are at most 32 clicks (`MAX_CLICKS`). There is no minimum box size: the
app ignores a drag under 1% of the photo as a gesture, but a box it derives,
such as a mask's extent, may be thinner.

## Envelope

| Message | Shape |
| --- | --- |
| Request | `{"t":"req","id":n,"m":method,"p":{…}}` (`p` may be left out) |
| Success | `{"t":"ok","id":n,"v":value}` |
| Failure | `{"t":"err","id":n,"code":code,"message":text}` |
| Progress | `{"t":"progress","id":n,"done":0.5,"text":"…"}` (both optional) |
| Cancel | `{"t":"cancel","id":n}` |
| Event | `{"t":"event","e":name,"p":{…}}` |

Ids are per direction: each side numbers its own requests. Every request gets
exactly one reply. Fields a message or its parameters don't know are ignored.
A line that isn't a message is reported; when it was a request with a
readable id, it is answered `invalid_request`.

## Session

1. **Handshake.** The app sends `hello`; the program answers with its
   protocol and the contributions it implements. Another protocol on either
   side ends the session with `unsupported_protocol`. Nothing else is
   answered before `hello` but with `invalid_request`.
2. **Requests** go both ways. The SDK runs the app's requests one at a time;
   its reader keeps reading meanwhile, so cancellation and the app's replies
   to the program's own requests arrive while a handler runs.
3. **Deadlines.** Every app request has one: 30 s by default, 5 s for `hello`.
   At its deadline the app sends `cancel`, stops admitting the request (a
   proposal belonging to it is refused `not_admitted`), and fails it with
   `deadline` when the program answers, or 2 s later.
4. **Cancellation.** `cancel` asks the other side to stop; the reply is then
   `cancelled`. A request the app cancels is settled at once: a reply arriving
   afterwards is dropped, so a request has one outcome, whichever was recorded
   first.
5. **Flow.** The app never blocks on a program: what it sends goes through a
   bounded queue (64 messages), and when the program stops reading and the
   queue is full, the send fails with `channel_full`. The program's operations
   (`adapter.operate`, reads, proposals) go into another bounded queue the app
   drains each frame; when it is full they are answered `channel_full` at once.
   `adapter.device` is applied as it is read, outside that queue, so a
   disconnection is never held up behind its device's input.
6. **End.** On quit the app sends `quitting`, closes its side (Linux and
   macOS), and ends the unit if the program hasn't exited within 1 s or by the
   quit deadline. A program that exits fails every request it held with
   `exited`; it is started again at its next request, or at once when it
   starts with the app (`with-app`), until it is Failed.

## App to program

### `hello` (request)

```json
{"protocol":1,"app_version":"0.5.0","extension":"dev.jane.analog",
 "settings":{"default-stock":"Tri-X"},
 "contributions":["command/auto-film-look","panel/looks","mask-generator/subject"],
 "data_dir":"…/extension-data/dev.jane.analog","cache_dir":"…","session":3}
```

`settings` holds every setting, defaults filled in. `contributions` are the
manifest's, as `<table>/<id>`. `session` numbers this program start, so a
restarted program's devices are never taken for the previous run's. Reply:
`{"protocol":1,"implements":["command/auto-film-look","panel/looks"]}`.

### `command.run` (request)

`{"command":"auto-film-look","target":snapshot,"input":null}`. `target` is a
[snapshot](#snapshots-and-proposals) of what the command applies to (absent for
`applies-to = "none"`); `input` is viewer input (phase 3). Reply: any value
(`null` for nothing). Edits go as `propose.settings` while it runs.

### Panels (requests)

- `panel.open` `{"panel":id}`
- `panel.changed` `{"panel":id,"control":id,"value":value}`, sent when the user
  commits a value (a text field on Enter or when it loses focus)
- `panel.clicked` `{"panel":id,"control":id}`, for a button

Each is answered with the panel's controls, whole: `{"controls":[…]}`
([controls](#panels)). A value that doesn't validate is answered with the
controls, the offending one carrying the error as its `help`.

### `mask.generate` (request)

Asks a [mask generator](manifest.md#mask-generator) for a mask's coverage:

```json
{"generator":"subject","input":{"kind":"prompt","clicks":[…],"bounds":{…}},
 "image":buffer,"coverage":buffer,"image_key":"…"}
```

- `input` is [viewer input](#viewer-input), or absent or `null` for an
  automatic mask.
- `image` is the photo as the model sees it, [buffer](#buffers) for reading:
  3-channel `u8` sRGB, region `image_frame` `[0, 0, 1, 1]`.
- `coverage` is where the program writes the mask, a buffer for writing:
  1-channel `u8` `coverage`, the image's size and region. Both are allocated by
  the app, in their own handles, 256 MiB at most together.
- `image_key` is the app's digest of the image's size, pixels and rendering, so
  the program can keep its analysis of a photo without hashing.

The SDK refuses a request whose input or buffers don't pass their checks, or
don't fit together, with `invalid_request`, before the handler runs. Reply:

```json
{"model":"sam2.1-hiera-small@0123456789abcdef","model_key":"…","empty":"no_subject"}
```

`model` names the model, saved with the mask as its provenance. `model_key`
stands for everything besides the pixels that the result depends on (the
model's identity and weights, and the versions of its contract and
processing), saved in the mask's input identity. Each is 1 to 128 bytes.
`empty` is set when nothing was found: `no_subject`, `no_sky` or
`nothing_there`. A generator whose model isn't installed answers `needs_setup`;
one whose runtime or worker is missing answers `unavailable`.

### `mask.setup` (request)

`{"generator":"subject"}`, sent when the user agrees to the setup a
`needs_setup` error asked for. The program installs what the generator needs,
reporting `progress`, and answers `null`.

### Events

| Event | Parameters |
| --- | --- |
| `setting-changed` | `{"id":setting,"value":value}` |
| `selection-changed` | none; read it with `read.selection` |
| `photo-opened` | `{"photo":id}` or `{"photo":null}` |
| `catalog-closed` | none; tokens from it are refused from now on |
| `quitting` | none; the channel closes next |

## Program to app

| Request | Parameters | Reply |
| --- | --- | --- |
| `read.capabilities` | none | The control layer's parameters and actions, as `rawmakase control capabilities` prints them |
| `read.action` | `{"spec":"…"}` | `{"supported":bool}`: whether a device's button may run it, an action name, a shortcut that stands for an action, or a held modifier (`hold:shift`), for a program to check one a user typed |
| `read.selection` | none | A snapshot of the Library's selection |
| `read.open-photo` | none | A snapshot of the photo open in Develop (no photos when none is) |
| `read.photos` | `{"ids":[…]}` | A snapshot of these photos |
| `propose.settings` | `{"op":n,"token":t,"changes":[{"photo":id,"settings":{…}}]}` | `{"applied":[ids],"refused":[{"photo":id,"code":code}]}` |
| `adapter.device` | `{"adapter":id,"device":id,"state":"connected"\|"disconnected"\|"removed","epoch":n}` | `null` |
| `adapter.operate` | `{"adapter":id,"device":id,"epoch":n,"ops":[…]}` | `{"applied":n}` |
| `notify` | `{"level":"info"\|"warning"\|"error","text":"…"}` | `null` |
| `status` | `{"text":"…"}` or `{"text":null}` | `null` |

Event: `panel-update` `{"panel":id,"controls":[…]}` replaces an open panel's
controls unasked.

### Snapshots and proposals

A read returns `{"token":t,"photos":[{"id":n,"path":"…","settings":{…}}]}`.
Settings use the control layer's names and units; on a JPEG, PNG or TIFF,
`temperature` and `tint` are its relative Temp and Tint (−100 to 100) in a
read as in a proposal. The token is opaque: it names
the catalog, its session and structure, and each photo's file and edit as
read. A proposal sends it back; the app checks each photo just before applying
and refuses one whose file, edit or metadata moved on as `stale_target`,
applying the rest, each as one History step. `op` is the id of the app's
`command.run` request the proposal belongs to; once that request is answered,
cancelled or past its deadline, the proposal is refused `not_admitted` and
nothing changes.

What the app checks, photo by photo:

- A token of a catalog that has since closed, been switched for another or
  read again (which a removal that may reuse ids does) is refused whole,
  `stale_target`.
- The photo open in Develop is checked against Develop's state now, whether or
  not it was open when read. Read from Develop, it must still be the same load
  with no edit since; read from the catalog, Develop must show just what the
  catalog holds (nothing unsaved, not protected, the saved settings) and the
  catalog state must be the same. Alone in the proposal, it changes as one
  step of its History.
- A photo read from Develop that is no longer open is accepted only if, when
  it was read, Develop showed just what the catalog holds. Otherwise the read
  saw settings the catalog never had, or a protected edit, and the photo is
  refused `stale_target`.
- Any other photo must still have the file and the catalog state it was read
  with, checked before its file is opened and again inside the catalog's one
  write for the proposal, so nothing changed meanwhile is overwritten. It is
  changed as a batch edit changes it, with a History step, off the UI thread;
  the reply comes when that work is done. One Undo covers every photo of the
  proposal: when the open photo is among others, it is saved first and
  changed with them, then opens again with what was written.
- A cancellation of the command (answered, cancelled, past its deadline, the
  extension disabled, its program gone, the app quitting) or the user's
  Cancel before that write leaves every photo unchanged; the reply is then
  `not_admitted`, sent as soon as the command stops being admitted, without
  waiting for the work to stop, or `cancelled` for the user's Cancel. A
  program that cancels its proposal is answered `cancelled` and its work stops
  the same way. Once the write has begun it completes or fails as a whole, and
  what it wrote stands, with its Undo; the reply then says what it wrote.
- While another batch edit runs, the photos of a proposal that needs one are
  refused `busy`.
- A read returns at most 200 photos and a proposal changes at most 200
  (`too_large`). A setting name the control layer doesn't know, or a value that
  isn't a finite number, refuses that photo `invalid_request`.

A photo's settings in a read are Develop's for the open photo and, for any
other, the settings Develop would open it with: its saved RAWmakase edit, else
its Lightroom edit, else the raw defaults, so a read means the same whether or
not the photo is open. A photo whose settings can't be read (an edit that
can't be read, or protected because its file changed; a file that can't be
opened) has no settings and an `error` saying why, and a proposal for it is
refused `stale_target`. Photos not open are read off the UI thread, and the
reply comes when that is done; a program that cancels its read is answered
`cancelled`.

### Control adapters

`adapter.device` and `adapter.operate` name a `[[control-adapter]]` the
extension declares and the app launches (`launch = "app"`); any other is
refused, `adapter_disabled` for `adapter.operate`, `invalid_request` for
`adapter.device`. An externally launched adapter doesn't use this protocol at
all; it sends control requests naming itself
([Externally launched adapters](overview.md#externally-launched-adapters)).

`adapter.device` reports a device: `device` is the program's id for it,
`epoch` counts up each time it connects. The app applies it on its reader
thread as it reads it, ahead of any operation still queued. A `connected`
epoch must be higher than any before for that device (`invalid_request`
otherwise); a `disconnected` or `removed` of an earlier epoch than the latest
changes nothing, so a stopped thread's late report never ends its successor's
connection. Each program start has devices of its own: the app numbers the
start (`hello`'s `session`) and keeps a gesture to one start, device and epoch,
so a restarted program reusing device ids and epochs never continues one.

`adapter.operate` is answered with how many operations ran; when the device
isn't connected in `epoch` before the first runs, it is refused
`stale_target`. A disconnection, the program's end and disabling the
extension end the device's gesture. Only an `adapter.operate` that ran an
operation counts as useful work for the crash count; `adapter.device` doesn't.

### Device operations

`ops` are applied in order with the app's device semantics, each only while
`epoch` is the device's current one, read just before it runs, so input
queued before a disconnection never runs. An operation that fails (no photo
open, a parameter the app doesn't know) is said in the app's top bar, and the
rest still run:

| Operation | Fields | Meaning |
| --- | --- | --- |
| `adjust` | `param`, `ticks` (signed) | A relative turn of a parameter's dial |
| `value` | `param`, `value` (0–127) | A fader's absolute position, mapped onto the parameter's range |
| `photo` | `ticks`, `detent` | The photo dial's raw ticks: the app counts `detent` ticks per photo, per device and epoch, the first tick of a turn moving at once; where the dial does nothing (the Library grid) ticks are dropped before they count |
| `action` | `spec`, `advance` | An action in the control layer's notation (`undo`, `cmd+shift+z`, `mixer:hue`), the device's held modifiers already merged into a shortcut; `advance` moves to the next photo after a metadata action |

## Panels

A panel is a list of controls in one column. Each is an object with a `type`,
an `id` (required for those that send a value), and optionally `label`,
`help`, `disabled` and `visible_when` (`{"control":id,"equals":value}`):

| `type` | Fields |
| --- | --- |
| `section` | `title`, `controls`, `collapsed` |
| `slider` | `value`, `min`, `max`, `step`, `unit`, `centred` |
| `angle` | `value` (degrees) |
| `toggle` | `value` |
| `choice`, `segmented` | `options` (`[{"id":…,"title":…}]`), `value` (an option's id) |
| `color` | `value` (`[r,g,b]`, or `[r,g,b,a]` with `alpha`), sRGB 0–1 |
| `curve` | `points` (`[[x,y],…]`, 0–1, increasing x) |
| `number` | `value`, `min`, `max` |
| `text` | `value`, `multiline` |
| `file` | `value`, `extensions` |
| `folder` | `value` |
| `button` | `title`; clicking sends `panel.clicked` |
| `label`, `help` | `text` |
| `progress` | `value` (0–1, or none for unknown) |
| `image` | `path`, a PNG the program wrote in its data or cache folder |

The app refuses a panel that nests sections deeper than 8, holds more than 1000
controls, reuses an id, or has a value outside its range.

## Error codes

| Code | Meaning |
| --- | --- |
| `cancelled` | The request was cancelled |
| `deadline` | The request outlived its deadline |
| `invalid_request` | Its parameters don't fit its method, it came before `hello`, or the line wasn't a message |
| `unknown_method` | The method isn't part of this protocol |
| `stale_target` | A proposal's photo moved on since its snapshot |
| `disabled` | The extension or the contribution is disabled |
| `busy` | The side is doing something that excludes this request now |
| `channel_full` | The other side isn't keeping up; nothing was done |
| `too_large` | The reply or message would exceed a line |
| `unsupported_protocol` | The other side speaks another protocol |
| `exited` | The program exited, or the channel closed, before the reply |
| `not_admitted` | A proposal for an app request no longer in flight |
| `needs_setup` | The contribution needs something installed first. The message is JSON, `{"what":"the Subject model","bytes":184000000}`, for the app's setup card; a message that doesn't read as that is shown as text |
| `unavailable` | What the contribution runs on, such as a runtime library or a remote worker, isn't available; the message says why |

The control layer's own codes (`no_document`, `not_editable`, …, [automation](../automation.md))
pass through unchanged.
