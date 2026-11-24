---------------------------- MODULE ExtensionSession ----------------------------
(***************************************************************************)
(* The extension session's app requests that carry shared-memory buffers: *)
(* `crates/rawmakase-extensions/src/session.rs` (`Session::request_with`,  *)
(* `cancel`, `poll`, `take`, `reconcile`, `enforce_deadlines`, `hang_up`)  *)
(* and its reader thread (`Reading::record`, `dropped_reply`).             *)
(*                                                                         *)
(* Three parties run at once:                                              *)
(*  - the program, which works on each request and answers it once (ok,    *)
(*    an error, or `invalid_request`, a protocol fault), or exits;         *)
(*  - the reader, which records a leased request's answer in the shared   *)
(*    `leased` map and then hands the line to a bounded queue that may     *)
(*    drop it;                                                             *)
(*  - the owner (the app's UI thread), which sends, cancels and polls.    *)
(* Time is abstract: a request's current timer (its deadline, the cancel  *)
(* grace after it, or its lease's grace) may pass at any step.            *)
(*                                                                         *)
(* What TLC checks:                                                        *)
(*  - each request has at most one outcome for the app, and, with fair   *)
(*    polling, exactly one;                                                *)
(*  - its buffers are let go of only once the program has let go of them  *)
(*    (it answered, and not with a fault) or its process has ended; a     *)
(*    hang-up kills the unit, which ends at a later step;                  *)
(*  - an answer the reader has recorded never leads to a hang-up;          *)
(*  - every lease ends once the process has ended.                         *)
(***************************************************************************)
EXTENDS Naturals, Sequences, FiniteSets

CONSTANTS Requests,  \* request ids
          QueueSize  \* the reader-to-owner queue's capacity

Kinds == {"ok", "err", "fault"}

VARIABLES
    st,        \* owner's view: none, pending, expired, lingering, done
    buf,       \* the buffers: none, leased, held (until the unit ends), freed
    prog,      \* program: idle (not asked), working, or what it answered (ok, err, fault)
    pipe,      \* answers written, not yet read: <<id, kind>>
    reading,   \* the line the reader is between recording and queueing, or <<>>
    queue,     \* reader-to-owner queue of answers
    rec,       \* leased map entry: [s |-> absent | empty | recorded, kind, dropped]
    due,       \* the request's current timer has passed
    outcomes,  \* app outcomes per request
    hung,      \* the session hung the program up
    closed,    \* the owner saw the reader finish
    killed,    \* the session killed the unit; it ends at a later step
    exited,    \* the program's process has ended
    chanClosed, \* the program closed its channel (it may still run)
    readerDone,
    badHang    \* the session hung up a program whose answer it had already seen

vars == <<st, buf, prog, pipe, reading, queue, rec, due, outcomes, hung, closed,
          killed, exited, chanClosed, readerDone, badHang>>

Absent == [s |-> "absent", kind |-> "ok", dropped |-> FALSE]
Empty == [s |-> "empty", kind |-> "ok", dropped |-> FALSE]
Recorded(k, d) == [s |-> "recorded", kind |-> k, dropped |-> d]
IsRecorded(x) == x.s = "recorded"

Init ==
    /\ st = [r \in Requests |-> "none"]
    /\ buf = [r \in Requests |-> "none"]
    /\ prog = [r \in Requests |-> "idle"]
    /\ pipe = <<>>
    /\ reading = <<>>
    /\ queue = <<>>
    /\ rec = [r \in Requests |-> Absent]
    /\ due = [r \in Requests |-> FALSE]
    /\ outcomes = [r \in Requests |-> 0]
    /\ hung = FALSE
    /\ closed = FALSE
    /\ killed = FALSE
    /\ exited = FALSE
    /\ chanClosed = FALSE
    /\ readerDone = FALSE
    /\ badHang = FALSE


(* `request_with`: leased before it can be answered, then pending.         *)
Send(r) ==
    /\ st[r] = "none" /\ ~hung /\ ~closed
    /\ st' = [st EXCEPT ![r] = "pending"]
    /\ buf' = [buf EXCEPT ![r] = "leased"]
    /\ rec' = [rec EXCEPT ![r] = Empty]
    /\ prog' = [prog EXCEPT ![r] = IF exited THEN "idle" ELSE "working"]
    /\ UNCHANGED <<pipe, reading, queue, due, outcomes, hung, closed, killed, exited, chanClosed, readerDone, badHang>>

(* `cancel`: settled now; the lease lingers until the program answers.     *)
Cancel(r) ==
    /\ st[r] \in {"pending", "expired"}
    /\ st' = [st EXCEPT ![r] = "lingering"]
    /\ outcomes' = [outcomes EXCEPT ![r] = @ + 1]
    /\ due' = [due EXCEPT ![r] = FALSE]
    /\ UNCHANGED <<buf, prog, pipe, reading, queue, rec, hung, closed, killed, exited, chanClosed, readerDone, badHang>>

Answer(r, k) ==
    /\ prog[r] = "working" /\ ~exited /\ ~chanClosed
    /\ prog' = [prog EXCEPT ![r] = k]
    /\ pipe' = Append(pipe, <<r, k>>)
    /\ UNCHANGED <<st, buf, reading, queue, rec, due, outcomes, hung, closed, killed, exited, chanClosed, readerDone, badHang>>

Exit ==
    /\ ~exited
    /\ exited' = TRUE
    /\ UNCHANGED <<st, buf, prog, pipe, reading, queue, rec, due, outcomes, hung, closed, killed, chanClosed, readerDone, badHang>>

(* `Reading::record`: a leased id's first answer is recorded.              *)
ReaderRecord ==
    /\ reading = <<>> /\ pipe /= <<>>
    /\ LET m == Head(pipe) IN
         /\ reading' = m
         /\ rec' = IF rec[m[1]] = Empty
                      THEN [rec EXCEPT ![m[1]] = Recorded(m[2], FALSE)]
                      ELSE rec
    /\ pipe' = Tail(pipe)
    /\ UNCHANGED <<st, buf, prog, queue, due, outcomes, hung, closed, killed, exited, chanClosed, readerDone, badHang>>

(* The queue takes the line, or drops it and marks the record dropped.     *)
ReaderQueue ==
    /\ reading /= <<>>
    /\ \/ /\ Len(queue) < QueueSize
          /\ queue' = Append(queue, reading)
          /\ UNCHANGED rec
       \/ /\ Len(queue) >= QueueSize
          /\ queue' = queue
          /\ rec' = IF IsRecorded(rec[reading[1]])
                       THEN [rec EXCEPT ![reading[1]].dropped = TRUE]
                       ELSE rec
    /\ reading' = <<>>
    /\ UNCHANGED <<st, buf, prog, pipe, due, outcomes, hung, closed, killed, exited, chanClosed, readerDone, badHang>>

(* The program closes its channel and goes on running: the reader ends.   *)
CloseChannel ==
    /\ ~chanClosed /\ ~exited
    /\ chanClosed' = TRUE
    /\ UNCHANGED <<st, buf, prog, pipe, reading, queue, rec, due, outcomes, hung, closed,
                   killed, exited, readerDone, badHang>>

ReaderFinish ==
    /\ (exited \/ chanClosed) /\ pipe = <<>> /\ reading = <<>> /\ ~readerDone
    /\ readerDone' = TRUE
    /\ UNCHANGED <<st, buf, prog, pipe, reading, queue, rec, due, outcomes, hung, closed, killed, exited, chanClosed, badHang>>

Tick(r) ==
    /\ st[r] \in {"pending", "expired", "lingering"} /\ ~due[r]
    /\ due' = [due EXCEPT ![r] = TRUE]
    /\ UNCHANGED <<st, buf, prog, pipe, reading, queue, rec, outcomes, hung, closed, killed, exited, chanClosed, readerDone, badHang>>

(***************************************************************************)
(* The owner's poll, as one step, over a record of the session's state:    *)
(* s = [st, buf, rec, due, outcomes, hang], hang the reason to hang up.    *)
(***************************************************************************)

(* `take`'s `settle` for an answer drained from the queue.                 *)
Take(s, m) ==
    LET r == m[1]
        k == m[2]
        leasedWas == s.rec[r] /= Absent
        fault == leasedWas /\ k = "fault"
        s1 == [s EXCEPT !.rec[r] = Absent, !.hang = s.hang \/ fault]
    IN IF s1.st[r] = "lingering"
         THEN [s1 EXCEPT !.st[r] = "done", !.buf[r] = IF fault THEN "held" ELSE "freed"]
       ELSE IF s1.st[r] \in {"pending", "expired"}
         THEN [s1 EXCEPT !.st[r] = "done", !.outcomes[r] = @ + 1,
                         !.buf[r] = IF fault THEN "held" ELSE "freed"]
       ELSE s1

RECURSIVE TakeAll(_, _)
TakeAll(s, q) == IF q = <<>> THEN s ELSE TakeAll(Take(s, Head(q)), Tail(q))

(* `reconcile`, one recorded id.                                           *)
Reconcile1(s, r) ==
    IF ~IsRecorded(s.rec[r]) THEN s
    ELSE LET x == s.rec[r]
             fault == x.kind = "fault"
             s1 == [s EXCEPT !.hang = s.hang \/ fault]
         IN IF s1.st[r] = "lingering"
              THEN [s1 EXCEPT !.st[r] = "done", !.rec[r] = Absent,
                              !.buf[r] = IF fault THEN "held" ELSE "freed"]
            ELSE IF (x.dropped \/ fault) /\ s1.st[r] \in {"pending", "expired"}
              THEN [s1 EXCEPT !.st[r] = "done", !.rec[r] = Absent,
                              !.outcomes[r] = @ + 1,
                              !.buf[r] = IF fault THEN "held" ELSE "freed"]
            ELSE s1

RECURSIVE Fold(_, _, _)
Fold(F(_, _), s, rs) ==
    IF rs = {} THEN s
    ELSE LET r == CHOOSE x \in rs : TRUE IN Fold(F, F(s, r), rs \ {r})

(* `enforce_deadlines`, one id: a pending request past its deadline is     *)
(* cancelled and expires; an expired one past its grace fails and lingers. *)
Deadline1(s, r) ==
    IF ~s.due[r] THEN s
    ELSE IF s.st[r] = "pending" THEN [s EXCEPT !.st[r] = "expired", !.due[r] = FALSE]
    ELSE IF s.st[r] = "expired"
      THEN [s EXCEPT !.st[r] = "lingering", !.due[r] = FALSE, !.outcomes[r] = @ + 1]
    ELSE s

(* The reader had finished: what is pending fails, its lease lingering.    *)
Finish1(s, r) ==
    IF s.st[r] \in {"pending", "expired"}
      THEN [s EXCEPT !.st[r] = "lingering", !.due[r] = FALSE, !.outcomes[r] = @ + 1]
      ELSE s

LeaseHang(s) == s.hang \/ \E r \in Requests : s.st[r] = "lingering" /\ s.due[r]

(* `hang_up`, one id.                                                      *)
Hang1(s, r) ==
    IF s.st[r] \in {"pending", "expired"}
      THEN [s EXCEPT !.st[r] = "done", !.outcomes[r] = @ + 1, !.buf[r] = "held",
                     !.rec[r] = Absent]
    ELSE IF s.st[r] = "lingering"
      THEN [s EXCEPT !.st[r] = "done", !.buf[r] = "held", !.rec[r] = Absent]
    ELSE [s EXCEPT !.rec[r] = Absent]

(* The unit has ended: lingering and held leases end; the leased map keeps *)
(* only pending ids.                                                       *)
Ended1(s, r) ==
    LET s1 == IF s.st[r] = "lingering" THEN [s EXCEPT !.st[r] = "done", !.buf[r] = "freed"]
              ELSE IF s.buf[r] = "held" THEN [s EXCEPT !.buf[r] = "freed"]
              ELSE s
    IN IF s1.st[r] \in {"pending", "expired"} THEN s1 ELSE [s1 EXCEPT !.rec[r] = Absent]

Poll ==
    LET s0 == [st |-> st, buf |-> buf, rec |-> rec, due |-> due,
               outcomes |-> outcomes, hang |-> FALSE]
        finished == readerDone
        s1 == IF closed THEN s0 ELSE TakeAll(s0, queue)
        s2 == IF closed THEN s1 ELSE Fold(Reconcile1, s1, Requests)
        s3 == IF closed THEN s2 ELSE Fold(Deadline1, s2, Requests)
        s4 == IF ~closed /\ finished THEN Fold(Finish1, s3, Requests) ELSE s3
        hangNow == ~hung /\ LeaseHang(s4)
        \* Hung for a lease, every lingering lease past its grace answered:
        \* an answer the reader recorded before this poll was seen in it.
        unjust == /\ hangNow /\ ~s4.hang
                  /\ \A r \in Requests :
                        (s4.st[r] = "lingering" /\ s4.due[r]) => IsRecorded(s4.rec[r])
        s5 == IF hangNow THEN Fold(Hang1, s4, Requests) ELSE s4
        s6 == IF exited THEN Fold(Ended1, s5, Requests) ELSE s5
    IN
    /\ st' = s6.st
    /\ buf' = s6.buf
    /\ rec' = s6.rec
    /\ due' = s6.due
    /\ outcomes' = s6.outcomes
    /\ queue' = IF closed THEN queue ELSE <<>>
    /\ hung' = (hung \/ hangNow)
    /\ closed' = (closed \/ finished)
    /\ killed' = (killed \/ hangNow)
    /\ UNCHANGED <<exited, chanClosed>>
    /\ badHang' = (badHang \/ unjust)
    /\ UNCHANGED <<prog, pipe, reading, readerDone>>

Next ==
    \/ \E r \in Requests : Send(r) \/ Cancel(r) \/ Tick(r)
    \/ \E r \in Requests, k \in Kinds : Answer(r, k)
    \/ Exit \/ CloseChannel \/ ReaderRecord \/ ReaderQueue \/ ReaderFinish \/ Poll

Fairness ==
    /\ WF_vars(Poll) /\ WF_vars(ReaderRecord) /\ WF_vars(ReaderQueue)
    /\ WF_vars(ReaderFinish) /\ WF_vars(killed /\ Exit)   \* a killed unit ends
    /\ \A r \in Requests : WF_vars(Tick(r))

Spec == Init /\ [][Next]_vars /\ Fairness

-----------------------------------------------------------------------------
TypeOK ==
    /\ st \in [Requests -> {"none", "pending", "expired", "lingering", "done"}]
    /\ buf \in [Requests -> {"none", "leased", "held", "freed"}]
    /\ outcomes \in [Requests -> 0..1]

AtMostOneOutcome == \A r \in Requests : outcomes[r] <= 1

(* Buffers are let go of only once the program has, or its process ended: *)
(* an answer that is a fault (it may hold handles it never adopted) keeps  *)
(* them until the unit has ended, killed or not.                          *)
LeaseSafe == \A r \in Requests :
    buf[r] = "freed" => (prog[r] \in {"idle", "ok", "err"} \/ exited)

(* A settled request's lease is either lingering or let go of.             *)
SettledHasNoLiveLease == \A r \in Requests :
    st[r] = "done" => buf[r] \in {"held", "freed"}

(* A lease lingers only while the program's answer can still reach it,   *)
(* whether the queue kept the answer or dropped it.                        *)
InFlight(r) ==
    \/ \E i \in 1..Len(pipe) : pipe[i][1] = r
    \/ \E i \in 1..Len(queue) : queue[i][1] = r
    \/ (reading /= <<>> /\ reading[1] = r)
AnswerReachesLease == \A r \in Requests :
    (st[r] = "lingering" /\ prog[r] \in {"ok", "err", "fault"})
        => (InFlight(r) \/ IsRecorded(rec[r]) \/ exited)

(* An answer the reader has recorded never leads to a hang-up for its     *)
(* lease. One still in the pipe may: time is abstract here, and the lease  *)
(* grace can pass before the reader gets to it, as it can in the code.     *)
NoHangAfterAnswer == ~badHang

(* Every request sent is answered once, and every lease ends, once polled. *)
EveryRequestSettles == \A r \in Requests :
    st[r] /= "none" ~> (outcomes[r] = 1 /\ buf[r] = "freed")
=============================================================================
