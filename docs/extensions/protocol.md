# Extension protocol reference

Version 1 (`PROTOCOL` in `crates/rawmakase-extension-protocol`). What the app
and an extension's program say to each other. The SDK
(`crates/rawmakase-extension`) speaks it for you; this page is for reading logs,
writing an SDK in another language, and reviewing changes. The types are in
`message.rs`, `method.rs` and `controls.rs` of the protocol crate; where this
page and they differ, they are right.

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

### Reserved: handles and buffers (phase 3)

Not built in phase 2 (decision D7); specified now so they fit without a
protocol change:

- A message carrying handles names their count, `"handles": n`. On Linux and
  macOS the handles travel as `SCM_RIGHTS` ancillary data on the `sendmsg`
  that holds the line's first byte, and the reader (`recvmsg`) attaches them to
  that line. On Windows the app duplicates each handle into the program
  (`DuplicateHandle`, holding the program's process handle) and the message
  carries their values. Phase-2 readers ignore the field; nothing sends it.
- A pixel buffer's descriptor: `width`, `height`, `stride` (bytes per row),
  `channels`, `sample` (`f32` or `u16`), `space` (the colour space), `transfer`
  (linear or the encoding), and `region` (`x`, `y`, `width`, `height` of the
  photo it covers, in the photo's image frame). The app allocates every
  buffer, in both directions.

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
   `exited`; it is started again at its next request.

## App to program

### `hello` (request)

```json
{"protocol":1,"app_version":"0.5.0","extension":"dev.jane.analog",
 "settings":{"default-stock":"Tri-X"},
 "contributions":["command/auto-film-look","panel/looks"],
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
Settings use the control layer's names and units. The token is opaque: it names
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
  with no edit since; read from the catalog, it must have nothing unsaved and
  the same catalog state. It changes as one step of its History.
- Any other photo must still have the file and the catalog state it was read
  with; that is checked again inside the catalog's write, so nothing changed
  meanwhile is overwritten. It is changed as a batch edit changes it, with a
  History step, and one Undo covers every such photo of the proposal.
- A read returns at most 200 photos and a proposal changes at most 200
  (`too_large`). A setting name the control layer doesn't know, or a value that
  isn't a finite number, refuses that photo `invalid_request`.

A photo's settings in a read are Develop's for the open photo and its saved
RAWmakase edit for any other (none for a photo never edited in RAWmakase).
`adapter.device` and `adapter.operate` are refused `adapter_disabled` in this
version: the app runs no control adapter of an extension yet.

### Device operations

`ops` are applied in order with the app's device semantics, and refused unless
`epoch` is the device's current one, so input queued before a disconnection
never runs:

| Operation | Fields | Meaning |
| --- | --- | --- |
| `adjust` | `param`, `ticks` (signed) | A relative turn of a parameter's dial |
| `value` | `param`, `value` (0–127) | A fader's absolute position, mapped onto the parameter's range |
| `photo` | `ticks`, `detent` | The photo dial: `detent` ticks per photo; ignored where it does nothing (the Library grid) |
| `action` | `spec`, `advance` | An action in the control layer's notation (`history.undo`); `advance` moves to the next photo after |

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

The control layer's own codes (`no_document`, `not_editable`, …, [automation](../automation.md))
pass through unchanged.
