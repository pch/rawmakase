#!/bin/sh
# Make the film looks RAWmakase ships (assets/looks) and their presets
# (assets/presets/Film), downloading Kodak's datasheets on the way. Run from the
# repository root; needs uv and a network connection.
set -eu
for args in "portra-400" "portra-400 --print" "kodachrome-64" "kodachrome-64 --print"; do
    # shellcheck disable=SC2086 # the film and its options
    uv run --quiet scripts/film/film-look.py $args -o assets/looks --presets assets/presets/Film
done
