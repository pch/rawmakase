#!/usr/bin/env bash
# Select Sky evaluation: this checkout (candidate) against a baseline commit and Lightroom.
# See scripts/sky-eval/README.md. Run from anywhere in Git Bash:
#
#   scripts/sky-eval/run.sh                      # main vs this checkout, Opus + Sonnet review
#   SKIP_REVIEW=1 scripts/sky-eval/run.sh        # metrics and panels only
#   BASELINE_REF=abc1234 BASELINE_EDGE=2560 scripts/sky-eval/run.sh
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
SAMPLES="${SAMPLES:-$REPO/../rawmakase-samples/sky}"
LR_DIR="${LR_DIR:-$SAMPLES/LR-mask-exports}"
DATA="${DATA:-$REPO/../rawmakase-testdata/sky-eval}"
PHOTOS="${PHOTOS:-$HERE/photos.txt}"
BASELINE_REF="${BASELINE_REF:-main}"
BASELINE_EDGE="${BASELINE_EDGE:-1280}"
CANDIDATE_EDGE="${CANDIDATE_EDGE:-1280}"
REVIEWERS="${REVIEWERS:-opus,sonnet}"
SKY_MODEL="${RAWMAKASE_SKY_MODEL:-$LOCALAPPDATA/RAWmakase/models/upernet-convnext-base-v1}"
ORT_LIB="${RAWMAKASE_ORT_LIB:-$LOCALAPPDATA/Programs/RAWmakase/onnxruntime.dll}"
win() { cygpath -m "$1"; }

case ",$REVIEWERS," in *,haiku,*) echo "Haiku is not used for this review." >&2; exit 1;; esac

mkdir -p "$DATA"/{runs,masks}
PY="$DATA/.venv/Scripts/python"
if [ ! -x "$PY" ]; then
  python -m venv "$DATA/.venv"
  "$PY" -m pip install -q numpy pillow scipy tifffile
fi

cand_sha="$(git -C "$REPO" rev-parse --short HEAD)"
dirty=""; git -C "$REPO" diff --quiet HEAD -- crates src tests Cargo.toml Cargo.lock || dirty="-dirty"
base_sha="$(git -C "$REPO" rev-parse --short "$BASELINE_REF")"
RUN="$DATA/runs/$(date +%Y-%m-%d_%H%M)-${cand_sha}${dirty}-vs-${base_sha}"
mkdir -p "$RUN"

# Photo list: paths relative to $SAMPLES, '#' comments allowed.
: > "$RUN/photos.txt"
tr -d '\r' < "$PHOTOS" | grep -v '^\s*#' | sed '/^\s*$/d' | while read -r p; do
  case "$p" in /*|?:*) f="$p";; *) f="$SAMPLES/$p";; esac
  [ -f "$f" ] || { echo "missing photo: $f" >&2; exit 1; }
  win "$f" >> "$RUN/photos.txt"
done

# Masks for every photo in the list; cached by commit and edge unless the tree is dirty.
have_all() { while read -r f; do [ -f "$1/$(basename "${f%.*}").png" ] || return 1; done < "$RUN/photos.txt"; }
dump() {  # dump <checkout> <edge> <out> [target dir]
  ( cd "$1"
    export RAWMAKASE_SKY_LIST="$(win "$RUN/photos.txt")" RAWMAKASE_SKY_MODEL="$(win "$SKY_MODEL")" \
           RAWMAKASE_ORT_LIB="$(win "$ORT_LIB")" RAWMAKASE_SKY_EDGE="$2" RAWMAKASE_SKY_OUT="$(win "$3")" \
           RAWMAKASE_SKY_SAVE_INPUT=1  # the model's input, so evaluate.py can register to Lightroom's frame
    [ -n "${4:-}" ] && export CARGO_TARGET_DIR="$4"
    cargo test --release --test sky_eval_dump -- --ignored --nocapture 2>&1 \
      | { grep -E " ms$|^timing |panicked|error" || [ $? -eq 1 ]; } ) | tee -a "$RUN/timings.txt"
}

CAND_MASKS="$DATA/masks/${cand_sha}${dirty}-${CANDIDATE_EDGE}"
[ -n "$dirty" ] && CAND_MASKS="$RUN/masks/candidate"
# REMEASURE=1 computes cached masks again (the same masks) to time both sides in this run.
if [ -n "${REMEASURE:-}" ] || ! have_all "$CAND_MASKS"; then
  echo "== candidate masks ($cand_sha$dirty, edge $CANDIDATE_EDGE)"
  echo "# candidate" >> "$RUN/timings.txt"
  dump "$REPO" "$CANDIDATE_EDGE" "$CAND_MASKS"
fi

BASE_MASKS="$DATA/masks/${base_sha}-${BASELINE_EDGE}"
if [ -n "${REMEASURE:-}" ] || ! have_all "$BASE_MASKS"; then
  echo "== baseline masks ($BASELINE_REF = $base_sha, edge $BASELINE_EDGE)"
  WT="$DATA/worktree-baseline"
  if [ -d "$WT" ]; then git -C "$WT" checkout -q --detach "$base_sha"; else git -C "$REPO" worktree add -q --detach "$WT" "$base_sha"; fi
  sed '/sky-eval: timings start/,/sky-eval: timings end/d' "$REPO/tests/sky_eval_dump.rs" > "$WT/tests/sky_eval_dump.rs"
  # The tool links the selection models; a baseline that doesn't list them for its tests gets them for this run.
  grep -q '^rawmakase-selection = ' <(sed -n '/^\[dev-dependencies\]/,/^\[/p' "$WT/Cargo.toml")     || sed -i '/^\[dev-dependencies\]/a rawmakase-selection = { path = "crates/rawmakase-selection" }' "$WT/Cargo.toml"
  echo "# baseline" >> "$RUN/timings.txt"
  dump "$WT" "$BASELINE_EDGE" "$BASE_MASKS" "$(win "$DATA/target-baseline")"
  rm "$WT/tests/sky_eval_dump.rs"; git -C "$WT" checkout -q -- Cargo.toml Cargo.lock
fi

lr_format="JPEG"; ls "$LR_DIR"/*_original.tif* >/dev/null 2>&1 && lr_format="TIFF where present, else JPEG"
cat > "$RUN/run.json" <<EOF
{"date": "$(date +%Y-%m-%d)", "candidate_ref": "$(git -C "$REPO" rev-parse --abbrev-ref HEAD) @ ${cand_sha}${dirty}",
 "baseline_ref": "$BASELINE_REF @ $base_sha", "candidate_edge": $CANDIDATE_EDGE, "baseline_edge": $BASELINE_EDGE,
 "candidate_masks": "$(win "$CAND_MASKS")", "baseline_masks": "$(win "$BASE_MASKS")",
 "reviewers": "$REVIEWERS", "lr_format": "$lr_format"}
EOF

echo "== metrics and panels"
"$PY" -I "$HERE/evaluate.py" --run "$(win "$RUN")" --lr "$(win "$LR_DIR")" \
  --masks "baseline=$(win "$BASE_MASKS")" --masks "candidate=$(win "$CAND_MASKS")" \
  --name "baseline=$BASELINE_REF ($base_sha)" \
  --name "candidate=$(git -C "$REPO" rev-parse --abbrev-ref HEAD) ($cand_sha$dirty)"

if [ -z "${SKIP_REVIEW:-}" ]; then
  echo "== blind review: $REVIEWERS"
  "$PY" -I "$HERE/review.py" --run "$(win "$RUN")" --models "$REVIEWERS"
fi

echo "== summary"
"$PY" -I "$HERE/aggregate.py" --run "$(win "$RUN")"
"$PY" -I "$HERE/report.py" --run "$(win "$RUN")" ${FINDINGS:+--findings "$(win "$FINDINGS")"}
echo "run: $RUN"
