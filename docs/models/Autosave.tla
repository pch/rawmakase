---------------------------- MODULE Autosave ----------------------------
(* The open photo's save lifecycle: SaveState (src/edit_session/save_state.rs),
   the autosave thread (src/app/develop/autosave.rs) and the synchronous save
   before navigation (Editor::flush, src/app/develop/workflow.rs).
   Versions stand for recipes: `ver` is the edit on screen, `stored[d]` what
   the catalog holds for photo d. *)
EXTENDS Naturals

CONSTANTS Photos, MaxVer

VARIABLES
    doc,        \* the photo open in Develop
    ver,        \* the version of its edit on screen
    st,         \* SaveState: "Clean" | "Pending" | "Saving" | "Failed" | "Protected"
    changed,    \* Saving { changed: Some(_) }
    saver,      \* Autosave's Saver: "Stopped" | "Idle" | "Saving"
    job,        \* the job on the saver: [d, v] or <<>>
    done,       \* a completion waiting in the channel: "none" | "ok" | "err" | "lost"
    stored      \* the catalog

vars == <<doc, ver, st, changed, saver, job, done, stored>>

NoJob == [d |-> "none", v |-> 0]

Init ==
    /\ doc \in Photos
    /\ stored = [p \in Photos |-> 0]
    /\ ver = 0
    /\ st = "Clean"
    /\ changed = FALSE
    /\ saver = "Stopped"
    /\ job = NoJob
    /\ done = "none"

NeedsSave == st \in {"Pending", "Saving", "Failed"}

MarkChanged ==
    CASE st = "Protected" -> UNCHANGED <<st, changed>>
      [] st = "Saving"    -> changed' = TRUE /\ UNCHANGED st
      [] OTHER            -> st' = "Pending" /\ UNCHANGED changed

\* SaveState::finished: the result applies only while Saving.
Finished(ok) ==
    IF st = "Saving"
    THEN /\ st' = IF ~ok THEN "Failed" ELSE IF changed THEN "Pending" ELSE "Clean"
         /\ changed' = FALSE
    ELSE UNCHANGED <<st, changed>>

Edit ==
    /\ st /= "Protected"
    /\ ver < MaxVer
    /\ ver' = ver + 1
    /\ MarkChanged
    /\ UNCHANGED <<doc, saver, job, done, stored>>

\* Editor::autosave once the edit settled (ready) and nothing is in flight.
Submit ==
    /\ st \in {"Pending", "Failed"}
    /\ saver /= "Saving"
    /\ saver' = "Saving"
    /\ job' = [d |-> doc, v |-> ver]
    /\ st' = "Saving" /\ changed' = FALSE
    /\ UNCHANGED <<doc, ver, done, stored>>

\* The saver writes the job; a failed write leaves the catalog as it was.
WorkerSaves ==
    /\ job /= NoJob /\ done = "none"
    /\ \/ /\ stored' = [stored EXCEPT ![job.d] = job.v] /\ done' = "ok"
       \/ /\ done' = "err" /\ UNCHANGED stored
    /\ job' = NoJob
    /\ UNCHANGED <<doc, ver, st, changed, saver>>

\* The saver thread is gone, after or before writing.
WorkerLost ==
    /\ job /= NoJob /\ done = "none"
    /\ \/ stored' = [stored EXCEPT ![job.d] = job.v]
       \/ UNCHANGED stored
    /\ done' = "lost" /\ job' = NoJob
    /\ UNCHANGED <<doc, ver, st, changed, saver>>

\* Autosave::poll / wait, then Editor::background_saved.
Receive ==
    /\ saver = "Saving" /\ done /= "none"
    /\ saver' = IF done = "lost" THEN "Stopped" ELSE "Idle"
    /\ Finished(done = "ok")
    /\ done' = "none"
    /\ UNCHANGED <<doc, ver, job, stored>>

\* Editor::flush, then opening photo `p` (load_raw, header_ready): it waits for
\* the save in flight, saves synchronously, and opens only once nothing needs
\* saving.
Navigate(p) ==
    /\ saver /= "Saving"
    /\ \/ /\ ~NeedsSave
          /\ UNCHANGED stored
       \/ /\ NeedsSave
          /\ stored' = [stored EXCEPT ![doc] = ver]
    /\ doc' = p
    /\ ver' = stored'[p]
    /\ st' = "Clean" /\ changed' = FALSE
    /\ UNCHANGED <<saver, job, done>>

Next ==
    \/ Edit \/ Submit \/ WorkerSaves \/ WorkerLost \/ Receive
    \/ \E p \in Photos : Navigate(p)

Spec == Init /\ [][Next]_vars

\* A clean edit is what the catalog holds.
CleanIsStored == st = "Clean" => stored[doc] = ver
\* A job is on the saver only while it is Saving.
JobOnlyWhileSaving == job /= NoJob => saver = "Saving"
\* A save in flight carries an edit of the open photo.
JobIsOpenPhoto == job /= NoJob => job.d = doc
=============================================================================
