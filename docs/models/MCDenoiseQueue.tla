-------------------------- MODULE MCDenoiseQueue --------------------------
(* Model values for DenoiseQueue: photo a runs on the GPU and moves to the
   CPU's key ac when it fails; photo b runs on the CPU. *)
EXTENDS DenoiseQueue
CONSTANTS a, ac, b
MCKeys == {a, ac, b}
MCCpu == (a :> ac) @@ (ac :> ac) @@ (b :> b)
=============================================================================
