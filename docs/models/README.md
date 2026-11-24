# State machine models

Status: Current. TLA+ models of state machines whose states are spread over
several fields and threads, checked with TLC. Each models the code named in
its header; change the model with the code.

| Model | Code | What TLC checks |
| --- | --- | --- |
| `Autosave` | `src/edit_session/save_state.rs`, `src/app/develop/autosave.rs`, `Editor::flush` | A clean edit is what the catalog holds; a job is on the saver only while it is Saving, and only for the open photo |
| `DenoiseQueue` (via `MCDenoiseQueue`) | `crates/rawmakase-denoise/src/lib.rs` | The worker never runs a job nobody wants (no Develop request, no export waiting, a cancelled export included), and a waiting export's job is never dropped, also after a GPU failure moves it to the CPU's key |

`DenoiseQueueBefore.cfg` models the queue before export waiters were counted
in one place: TLC reports, in four steps, an export cancelled while its job
waits and the worker running that job anyway.

Run them with TLC (`tla2tools.jar`, Java 11 or later):

```sh
cd docs/models
java -cp tla2tools.jar tlc2.TLC -config Autosave.cfg Autosave
java -cp tla2tools.jar tlc2.TLC -config DenoiseQueue.cfg MCDenoiseQueue
java -cp tla2tools.jar tlc2.TLC -config DenoiseQueueBefore.cfg MCDenoiseQueue  # finds the old bug
```

The models are small (two photos, two exports, a version bound): TLC explores
every interleaving of them, which says nothing about larger configurations or
about code the model leaves out.
