"""Blind review of the panels in RUN_DIR/review by Claude models, through the `claude` CLI.

Usage: review.py --run RUN_DIR [--models opus,sonnet] [--batch 8] [--prompts-only]

Photos are reviewed in batches (overview + three crops each, so 8 photos = 32 images per
call). Each call runs in RUN_DIR/review (which holds only the blind panels, not key.json),
may only use the Read tool, and its reply is merged into RUN_DIR/review_<model>.json.
Batches already in that file are skipped, so an interrupted run can be resumed.

--prompts-only writes RUN_DIR/prompts/batch<N>.md and calls nothing: use it when the CLI
cannot run (e.g. not signed in) and the batches are given to reviewers another way; save
each reply as RUN_DIR/prompts/batch<N>.<model>.json and rerun with --merge.

Haiku is refused: in the first run it described one photo the wrong way round.
"""
import argparse, glob, json, os, subprocess, sys
from concurrent.futures import ThreadPoolExecutor

HERE = os.path.dirname(os.path.abspath(__file__))


def extract_json(text):
    start, end = text.find("{"), text.rfind("}")
    if start < 0 or end < start:
        raise ValueError("no JSON object in the reply")
    return json.loads(text[start:end + 1])


def prompt_for(run_dir, letters, stems):
    p = open(os.path.join(HERE, "review_prompt.md"), encoding="utf-8").read()
    return (p.replace("{FOLDER}", os.path.join(run_dir, "review"))
            .replace("{LETTERS}", ", ".join(letters))
            .replace("{PHOTOS}", "\n".join(f"- {s}" for s in stems)))


def load(path):
    return json.load(open(path, encoding="utf-8")) if os.path.exists(path) else {"photos": {}}


def save(path, data):
    json.dump(data, open(path, "w", encoding="utf-8"), indent=1)


def call(model, run_dir, prompt, tag):
    proc = subprocess.run(
        ["claude", "-p", prompt, "--model", model, "--allowedTools", "Read", "--output-format", "text"],
        cwd=os.path.join(run_dir, "review"), capture_output=True, text=True, encoding="utf-8", timeout=3600)
    raw = os.path.join(run_dir, "prompts", f"{tag}.{model}.raw.txt")
    open(raw, "w", encoding="utf-8").write(proc.stdout + "\n--- stderr ---\n" + proc.stderr)
    if "Failed to authenticate" in proc.stdout + proc.stderr:
        raise SystemExit(f"{model}: the claude CLI is not signed in; run `claude` once and log in, "
                         f"or use --prompts-only (see {raw})")
    return extract_json(proc.stdout)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--run", required=True)
    ap.add_argument("--models", default="opus,sonnet")
    ap.add_argument("--batch", type=int, default=8)
    ap.add_argument("--prompts-only", action="store_true")
    ap.add_argument("--merge", action="store_true", help="merge prompts/batch<N>.<model>.json replies")
    args = ap.parse_args()
    run_dir = os.path.abspath(args.run)
    models = [m.strip() for m in args.models.split(",") if m.strip()]
    if "haiku" in models:
        sys.exit("Haiku is not used for this review (unreliable on the first run).")
    key = json.load(open(os.path.join(run_dir, "key.json")))
    stems = list(key)
    letters = list(next(iter(key.values())))
    batches = [stems[i:i + args.batch] for i in range(0, len(stems), args.batch)]
    os.makedirs(os.path.join(run_dir, "prompts"), exist_ok=True)

    if args.merge:
        for m in models:
            out = os.path.join(run_dir, f"review_{m}.json")
            data = load(out)
            for f in sorted(glob.glob(os.path.join(run_dir, "prompts", f"*.{m}.json"))):  # batch01, batch02, ...
                data["photos"].update(extract_json(open(f, encoding="utf-8").read())["photos"])
            save(out, data)
            print(f"{m}: {len(data['photos'])} of {len(stems)} photos")
        return

    for i, b in enumerate(batches, 1):
        open(os.path.join(run_dir, "prompts", f"batch{i:02d}.md"), "w", encoding="utf-8").write(
            prompt_for(run_dir, letters, b))
    print(f"{len(batches)} batches of up to {args.batch} photos in {os.path.join(run_dir, 'prompts')}")
    if args.prompts_only:
        return

    def work(job):
        m, i, b = job
        reply = call(m, run_dir, prompt_for(run_dir, letters, b), f"batch{i:02d}")
        json.dump(reply, open(os.path.join(run_dir, "prompts", f"batch{i:02d}.{m}.json"), "w"), indent=1)
        return m, i
    jobs = []
    for m in models:
        done = load(os.path.join(run_dir, f"review_{m}.json"))["photos"]
        jobs += [(m, i, b) for i, b in enumerate(batches, 1) if not all(s in done for s in b)]
    with ThreadPoolExecutor(4) as pool:
        for m, i in pool.map(work, jobs):
            print(f"{m}: batch {i} done")
    args.merge = True
    for m in models:
        out = os.path.join(run_dir, f"review_{m}.json")
        data = load(out)
        for f in sorted(glob.glob(os.path.join(run_dir, "prompts", f"*.{m}.json"))):  # batch01, batch02, ...
            data["photos"].update(json.load(open(f, encoding="utf-8"))["photos"])
        save(out, data)
        print(f"{m}: {len(data['photos'])} of {len(stems)} photos")


if __name__ == "__main__":
    main()
