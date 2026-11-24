--------------------------- MODULE DenoiseQueue ---------------------------
(* AI Denoise's job queue (crates/rawmakase-denoise/src/lib.rs): Develop
   requests (Service::request), export waiters (Service::entry with its
   Waiting guard) and the one worker (Service::work, Service::finish).
   A job is a key; Develop's interest in it is a `wanted` flag, export's
   the waiters still inside `entry`.

   Fixed = TRUE models the code: a queued job runs for an export only while
   that export's Waiting guard is counted in Queue.waiting (see `exported`).
   Fixed = FALSE models the code before it, which counted export waiters in
   Job.waiters, a count that only ever went up: TLC finds a cancelled
   export's job run anyway (DenoiseQueueBefore.cfg). *)
EXTENDS Naturals, FiniteSets, TLC

CONSTANTS Keys, Exports, Fixed, Cpu
\* Cpu: the key a GPU job is retried under on the CPU (GpuFailed in
\* Service::finish); a key mapped to itself is already the CPU's.

VARIABLES
    pending,    \* Queue.pending: key -> [develop: BOOLEAN, waiters: Nat], or absent
    running,    \* Queue.running: the key run, or "none"
    exp,        \* each export: [key, st: "idle" | "waiting" | "gone"]
    waiting,    \* Queue.waiting: key -> live Waiting guards
    wanted,     \* Develop's `wanted` flag per key
    wasted,     \* the worker took a job nobody wanted
    moved,      \* Outcome::Moved: the keys moved to the CPU's
    cached      \* results in the AI cache on disk

vars == <<pending, running, exp, waiting, wanted, wasted, moved, cached>>

None == "none"

Init ==
    /\ pending = [k \in {} |-> 0]
    /\ running = None
    /\ exp \in [Exports -> [key : Keys, st : {"idle"}]]
    /\ waiting = [k \in Keys |-> 0]
    /\ wanted = [k \in Keys |-> FALSE]
    /\ wasted = FALSE
    /\ moved = {}
    /\ cached = {}

Queued(k) == k \in DOMAIN pending

\* Develop asks for k: joins the running job, or the queued one, or adds one.
DevelopRequest(k) ==
    /\ ~wanted[k]
    \* Once the GPU is off (gpu_off), every request is keyed for the CPU.
    /\ moved /= {} => Cpu[k] = k
    /\ wanted' = [wanted EXCEPT ![k] = TRUE]
    /\ pending' = IF running = k THEN pending
                  ELSE IF Queued(k)
                       THEN [pending EXCEPT ![k].develop = TRUE]
                       ELSE pending @@ (k :> [develop |-> TRUE, waiters |-> 0])
    /\ UNCHANGED <<running, exp, waiting, wasted, moved, cached>>

\* Develop no longer wants k (another photo opened): its flag drops.
DevelopLeaves(k) ==
    /\ wanted[k]
    /\ wanted' = [wanted EXCEPT ![k] = FALSE]
    /\ UNCHANGED <<pending, running, exp, waiting, wasted, moved, cached>>

\* Service::entry: the Waiting guard counts it, then the job is joined or queued.
ExportStarts(e) ==
    /\ exp[e].st = "idle"
    \* Keyed for the CPU once the GPU is off (gpu_off).
    /\ LET k == IF moved /= {} THEN Cpu[exp[e].key] ELSE exp[e].key IN
       /\ waiting' = [waiting EXCEPT ![k] = @ + 1]
       /\ pending' = IF running = k THEN pending
                     ELSE IF Queued(k)
                          THEN [pending EXCEPT ![k].waiters = @ + 1]
                          ELSE pending @@ (k :> [develop |-> FALSE, waiters |-> 1])
       /\ exp' = [exp EXCEPT ![e] = [key |-> k, st |-> "waiting"]]
    /\ UNCHANGED <<running, wanted, wasted, moved, cached>>

\* The export is cancelled, or fails: dropping Waiting leaves Queue.waiting.
\* Nothing touches Job.waiters.
ExportCancelled(e) ==
    /\ exp[e].st = "waiting"
    /\ waiting' = [waiting EXCEPT ![exp[e].key] = @ - 1]
    /\ exp' = [exp EXCEPT ![e].st = "gone"]
    /\ UNCHANGED <<pending, running, wanted, wasted, moved, cached>>

\* Export waiters for k: on k itself, or on a key moved to k and not yet
\* followed (they follow within one 100 ms wait).
ExportWaiters(k) == waiting[k] + (IF \E j \in moved : Cpu[j] = k /\ j /= k
                                   THEN waiting[CHOOSE j \in moved : Cpu[j] = k /\ j /= k]
                                   ELSE 0)

\* Whether the queued job k is kept (the retain in Service::work).
Kept(k) ==
    \/ pending[k].develop /\ (wanted[k] \/ \E j \in moved : Cpu[j] = k /\ wanted[j])
    \/ IF Fixed THEN ExportWaiters(k) > 0 ELSE pending[k].waiters > 0

\* Somebody still wants k's result.
Wanted(k) == wanted[k] \/ (\E j \in moved : Cpu[j] = k /\ wanted[j]) \/ ExportWaiters(k) > 0

\* Service::work: drop the jobs not kept, take the first of the rest.
Take(k) ==
    /\ running = None
    /\ Queued(k)
    /\ Kept(k)
    /\ running' = k
    /\ pending' = [j \in (DOMAIN pending) \ {k} |-> pending[j]]
    /\ wasted' = (wasted \/ ~Wanted(k))
    /\ UNCHANGED <<exp, waiting, wanted, moved, cached>>

\* A queued job nobody keeps is dropped by the same retain.
Drop(k) ==
    /\ Queued(k)
    /\ ~Kept(k)
    /\ pending' = [j \in (DOMAIN pending) \ {k} |-> pending[j]]
    /\ UNCHANGED <<running, exp, waiting, wanted, wasted, moved, cached>>

\* Service::finish: the waiters take the result and leave.
Finish ==
    /\ running /= None
    /\ LET k == running IN
       /\ exp' = [e \in Exports |->
                    IF exp[e].key = k /\ exp[e].st = "waiting"
                    THEN [exp[e] EXCEPT !.st = "gone"] ELSE exp[e]]
       /\ waiting' = [waiting EXCEPT ![k] = 0]
       /\ cached' = cached \cup {k}
    /\ running' = None
    /\ UNCHANGED <<pending, wanted, wasted, moved>>

\* GpuFailed: the running job is queued again under the CPU's key, with its
\* Develop listeners and its export count (Job.waiters carries over).
GpuFails ==
    /\ running /= None
    /\ Cpu[running] /= running
    /\ LET k == running
           c == Cpu[k]
           old == IF Queued(c) THEN pending[c] ELSE [develop |-> FALSE, waiters |-> 0]
       IN /\ moved' = moved \cup {k}
          /\ pending' = [j \in (DOMAIN pending) \cup {c} |->
                           IF j = c
                           THEN [develop |-> old.develop \/ wanted[k],
                                 waiters |-> old.waiters + waiting[k]]
                           ELSE pending[j]]
    /\ running' = None
    /\ UNCHANGED <<exp, waiting, wanted, wasted, cached>>

\* An export waiting on a moved key leaves it and waits on the CPU's key
\* (the Outcome::Moved arm of Service::entry).
Follow(e) ==
    /\ exp[e].st = "waiting"
    /\ exp[e].key \in moved
    /\ LET k == exp[e].key IN
       /\ waiting' = [waiting EXCEPT ![k] = @ - 1, ![Cpu[k]] = @ + 1]
       /\ exp' = [exp EXCEPT ![e].key = Cpu[k]]
    /\ UNCHANGED <<pending, running, wanted, wasted, moved, cached>>

\* Service::entry, a waiter whose key is neither queued nor running and has
\* no outcome in memory: the cache has it ...
CacheHit(e) ==
    /\ exp[e].st = "waiting"
    /\ exp[e].key \notin moved
    /\ running /= exp[e].key
    /\ ~Queued(exp[e].key)
    /\ exp[e].key \in cached
    /\ waiting' = [waiting EXCEPT ![exp[e].key] = @ - 1]
    /\ exp' = [exp EXCEPT ![e].st = "gone"]
    /\ UNCHANGED <<pending, running, wanted, wasted, moved, cached>>

\* ... or it is queued again, to run over.
Requeue(e) ==
    /\ exp[e].st = "waiting"
    /\ exp[e].key \notin moved
    /\ running /= exp[e].key
    /\ ~Queued(exp[e].key)
    /\ exp[e].key \notin cached
    /\ pending' = pending @@ (exp[e].key :> [develop |-> FALSE, waiters |-> 1])
    /\ UNCHANGED <<running, exp, waiting, wanted, wasted, moved, cached>>

Next ==
    \/ \E k \in Keys : DevelopRequest(k) \/ DevelopLeaves(k) \/ Take(k) \/ Drop(k)
    \/ \E e \in Exports : ExportStarts(e) \/ ExportCancelled(e) \/ Follow(e)
    \/ \E e \in Exports : CacheHit(e) \/ Requeue(e)
    \/ Finish \/ GpuFails

Spec == Init /\ [][Next]_vars

\* A waiting export's job is never dropped from under it: its key (or the
\* CPU's, once moved) is queued, running or cached. Requeue would recover,
\* but only by running the job over.
NotStranded ==
    \A e \in Exports :
        LET k == IF exp[e].key \in moved THEN Cpu[exp[e].key] ELSE exp[e].key
        IN exp[e].st = "waiting" => (Queued(k) \/ running = k \/ k \in cached)

\* The worker never spends a run (minutes of the model) on a result nobody wants.
NoWastedRun == ~wasted
=============================================================================
