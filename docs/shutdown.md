# Shutdown contract

What each long-lived worker thread may block on, how it is told to stop, and whether
the UI thread may wait for it when RAWmakase quits. Code that adds a worker, or
changes what one waits on, updates its row here.

## Rules

1. **Signal, then close, then join.** At exit the UI thread first sets every
   cancel and stop flag. It then drops the channel ends the workers wait on, and
   only then joins the workers listed as joinable below, with a deadline. GPU
   resources go last: the preview renderer is joined before eframe drops the
   painter and device.
2. **Join only what is bounded and needs nothing from the UI thread.** A worker is
   joinable when, once signalled, it finishes within its current job and never
   waits for a reply from the UI thread. Everything else stays detached, keeps a
   stop signal it checks, and has a comment saying why it is not joined.
3. **No blanket join in `Drop`.** Dropping a handle signals; joining is a decision
   made in one exit hook, in the order above.
4. **Never hold a bounded receiver while joining its sender.** A worker blocked in
   `send` on a full channel never sees its stop signal.
5. **The exit hook runs on every exit path, but cannot refuse one.** On macOS,
   quitting from the Dock or by logging out with nothing pending closes the window
   without a close request, so eframe calls `App::on_exit` but never the close
   guard, and the process ends right after it. `on_exit` therefore saves what it
   can: the edit and the autosave in flight. Pending work is kept from that path
   before it starts; see Quit on macOS below.
6. **One deadline bounds quitting.** The loaders, previews, exports and output
   jobs read photo files, and the catalog may be on a network share, so a stalled
   read or write can outlast any job. Saving the edit and joining the workers share
   one deadline: a save still running when it passes is abandoned, and a worker
   still running is left detached to end with the process. Nothing is awaited
   without a deadline.

## Workers

"Stop" is the signal the worker checks; "Join" is what the exit hook may do.

Each worker the exit hook waits for keeps its `JoinHandle`, and hands it over as a
`task::Stopping` when asked to stop.

| Worker | Where | Blocks on | Stop | Join |
| --- | --- | --- | --- | --- |
| Develop loader, with its full-size loader and prefetcher | `app/worker/loader.rs` | A job; file I/O and LibRaw decodes; decode cache reads and writes | All three `Latest`s stopped, between jobs; the load `Task`'s cancel within one; `prefetch_cancel` for a prefetch | Yes, all three, after cancelling the load and the prefetch |
| Preview renderer | `app/worker/renderer.rs` | A job; CPU and GPU renders, up to 10 s per GPU submission; the egui renderer lock, briefly | `Latest` stopped; the After and Before render `Task`s | Yes, before the painter and device are dropped |
| Reference View loader | `app/worker/reference.rs` | A job; file I/O and decodes | `Latest` stopped; the reference `Task` | Yes |
| Autosave | `app/autosave.rs` | The job channel; one SQLite commit | Its job sender dropped | Yes, after the save at exit and dropping the job sender, which lets a save in flight finish |
| Export queue | `export/queue.rs` | A batch: decodes, renders and file writes | `closed`, between batches; the running batch's cancel, between photos and stages | Yes, after cancelling the running batch |
| Preview builds | `app/preview_build.rs` | A queue of photos: decodes, renders and one SQLite write each | `closed`, between photos; the running build's cancel, inside the decode and render | Yes, after cancelling |
| Command output jobs | `app/commands/output.rs` | One export or preview job | Each job's cancel, between stages | Yes, after cancelling; the close guard waits for them |
| MIDI listener | `app/automation/midi.rs` | A 2 s wait on its stop channel between port scans | The device dropped, which closes the channel | Yes; it stops at once |
| Control socket listener | `app/automation/socket.rs` | A blocking `accept`; then its connections, which it joins | The stop flag, seen after a wake-up connection | Yes, under the deadline: if the wake-up connection fails, `accept` never returns |
| Control socket connections | `app/automation/socket.rs` | A 5 s read; **a wait for the UI thread's reply: 3 s, or up to 8 s for a `wait` command**; a 5 s write | The stop flag; the request queue's receiver dropped, which answers the wait at once | Through the listener, after the request queue's receiver is dropped |
| Library thumbnails | `app/library/previews.rs` | The request channel; preview cache and file reads; **a send on a result channel bounded at 24** | Its channels dropped | Yes, after dropping the result receiver |
| Stored preview readers (opening, neighbour) | `app/stand_in.rs` | A job; one preview cache read and JPEG decode | Both `Latest`s stopped, between jobs | Yes, both |
| Library edited previews | `app/library/previews.rs` | A queue of renders | Its closed flag, which cancels the render under way; its channels dropped | Yes, after closing |
| Library screen previews | `app/library/screen.rs` | A job; renders | `ScreenPreviews` closed, which cancels the renders under way | Yes, after closing |
| Library background readers | `app/library/background.rs` | File reads in batches, possibly on a network share | The reader's cancel, per batch | No: a stalled share can hang a read |
| Volume probe | `app/library/volumes.rs` | `is_dir` and free space on every mount | None (one check per thread) | No: it can hang on a stalled mount |
| Subject selection | `app/subject_mask/worker.rs` | A job: the input render, the photo's analysis (a few seconds: embedding, saliency, a grid of decodes) when not kept, then a decode for a click; each ONNX Runtime call is cancellable within about 15 ms; it keeps its session until it ends | Its cancel flag, raised at exit; its mailbox dropped | Yes, under the deadline; the thread owns the runtime, which is never unloaded under a running call |
| Selection model install | `app/subject_mask/models.rs` | HTTP for five files, as requests resuming where the file stopped (connect 20 s, 30 s per response, 60 s for each body in total, 2 min each request) and file writes, in 256 KB chunks | Its cancel flag, between chunks and requests, and while waiting for another window's install | Yes, under the deadline; a partial download is kept for the next install to resume, under the folder's install lock |
| Availability check | `app/library/availability.rs` | File metadata for every photo | None | No, for the same reason |
| Update checker | `app/updates.rs` | Up to an hour between checks; a check or download of up to 15 min | Its request sender dropped, between requests | No: a download has no cancel |
| GVFS bridge reaper (Linux) | `platform/network.rs` | The bridge process, which outlives the app | None | Never |

One-shot jobs stay detached: Auto, Upright, Auto straighten, the Point Color and
Targeted Adjustment samples, the onboarding scan, catalog open and import, file
dialogs, bulk import, the preset scan, Sync Settings, folder relink and move, the
folder availability probe (which can hang on a stalled mount like the volume
probe), the update receipt acknowledgement and the watermark fonts. They report through the event channel or a generation check,
so a result that arrives after its document is gone is dropped. File dialogs on
macOS run their panel on the main thread, so they are never joined from it.

## The exit sequence

1. Close guard, on a window close: refuse while an export, Sync Settings, a
   folder change or a command output job runs, and save the edit after the
   autosave in flight, on the autosave thread. A save the catalog does not answer
   within the deadline keeps the window open, saying so, with Close without saving.
   A close refused only to let work finish is asked for again once it has.
2. `on_exit`, on every path, with one deadline for all of it:
   1. Cancel: the load, render, reference and prefetch tasks, the running export
      batch, the command output jobs and the library's renders.
   2. Close: drop the update request sender, the control request receiver and the
      library's channels.
   3. Stop: the `Latest` workers, the export queue, the command output jobs, the
      library's previews, MIDI and the control socket.
   4. Save the edit, if the close guard did not, as it does, while those wind down;
      a failure, or a save still running at the deadline, is written to stderr.
      Then drop the autosave job sender.
   5. Join the joinable workers above until the deadline. The renderer is among
      them, so it is done before eframe drops the device; any worker still
      running at the deadline is left detached.
   6. Delete the temporary files of exports still being written, on a thread of
      its own waited for 0.5 s: they are on the folder an export may have stalled on.
3. eframe drops the editor, then the painter. The detached workers end with the
   process.

## Quit on macOS

The close guard refuses to close while an export, Sync Settings, a folder change
or a command output job runs, and asks before closing when the edit cannot be
saved. winit's app menu binds Quit to
`terminate:`, which skips it, so at launch the app points that item at the key
window's `performClose:` instead (`platform/quit.rs`): Cmd-Q and the menu's Quit
then close the window as its close button does, and the guard runs.

Quitting from the Dock or by logging out still sends `terminate:`. winit's app
delegate has no `applicationShouldTerminate:`, so the app adds one
(`platform/quit.rs`). While any of that work runs, or an edit, Copy Name or
metadata field is not saved yet (pending, being saved or failed), it cancels the
quit and closes the window instead, so the close guard saves first and asks if
that fails, restoring a minimized window to show its question. With nothing
pending it lets the quit go at once, as before. A logout the app cancels this way stops, and
macOS says the app interrupted it.

## Where the code stands

The exit hook (`app/exit.rs`) runs the sequence above (#269, #270, #330). Two
writes still happen on the UI thread without a deadline: a Copy Name or metadata
field being typed in the Library, which the close guard and the exit hook save
through the catalog session before the edit, and the session file. Neither is
expected to stall the way a photo's catalog on a network share can, but a catalog
that stops answering mid-typing would still hold up closing.
