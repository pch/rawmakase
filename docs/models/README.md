# State machine models

Status: Current. TLA+ models of state machines whose states are spread over
several fields and threads, checked with TLC. Each models the code named in
its header; change the model with the code.

| Model | Code | What TLC checks |
| --- | --- | --- |
| `Autosave` | `src/edit_session/save_state.rs`, `src/app/develop/autosave.rs`, `Editor::flush` | A clean edit is what the catalog holds; a job is on the saver only while it is Saving, and only for the open photo |
| `DenoiseQueue` (via `MCDenoiseQueue`) | `crates/rawmakase-denoise/src/lib.rs` | The worker never runs a job nobody wants (no Develop request, no export waiting, a cancelled export included), and a waiting export's job is never dropped, also after a GPU failure moves it to the CPU's key |
| `ExtensionSession` | `crates/rawmakase-extensions/src/session.rs` (`request_with`, `cancel`, `poll`, `reconcile`, `hang_up`, the reader's `record`) | A request with buffers has exactly one outcome for the app, also when the reader's queue drops its answer; its buffers are let go of only once the program has answered (and not with a fault) or its process ended, a hang-up's kill ending the process only later; an answer the reader recorded never leads to a hang-up for its lease |

`DenoiseQueueBefore.cfg` models the queue before export waiters were counted
in one place: TLC reports, in four steps, an export cancelled while its job
waits and the worker running that job anyway.

Run them with TLC (`tla2tools.jar`, Java 11 or later):

```sh
cd docs/models
java -cp tla2tools.jar tlc2.TLC -config Autosave.cfg Autosave
java -cp tla2tools.jar tlc2.TLC -config DenoiseQueue.cfg MCDenoiseQueue
java -cp tla2tools.jar tlc2.TLC -config DenoiseQueueBefore.cfg MCDenoiseQueue  # finds the old bug
java -cp tla2tools.jar tlc2.TLC -config ExtensionSession.cfg ExtensionSession
```

`ExtensionSession` abstracts time: any timer may pass at any step, so it
checks orderings, not durations, and an answer still unread when its lease's
grace passes may end in a hang-up, as it can in the code. It doesn't tell
one error from another: a dropped answer that ends as `deadline` instead of
`channel_full` passes. Each poll is one step, so the reader doesn't
interleave inside it. It leaves out requests without buffers, the program's
own requests, a handle the program sends the app (`fault`), `stop` and
`Drop` handing leases to the unit, the `api < 2` refusal, and the host above
the session (crash counting, the idle stop).

The models are small (two photos, two exports, a version bound): TLC explores
every interleaving of them, which says nothing about larger configurations or
about code the model leaves out.
