"""Decode the blind reviews and combine them with the metrics.

Usage: aggregate.py --run RUN_DIR

Reads metrics.json, key.json and review_<model>.json; writes summary.json and prints it.
Composite score per RAWmakase source = 50 % panel (mean "overall" x 10)
+ 25 % IoU x 100 + 25 % boundary F x 100, both against Lightroom.
"""
import argparse, glob, itertools, json, os
import numpy as np
from scipy.stats import spearmanr

CRITERIA = ["bleed", "missed_sky", "edges", "sky_objects", "reflections", "overall"]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--run", required=True)
    run = ap.parse_args().run
    M = json.load(open(os.path.join(run, "metrics.json")))
    key = json.load(open(os.path.join(run, "key.json")))
    reviews = {os.path.basename(p)[7:-5]: json.load(open(p))["photos"]
               for p in sorted(glob.glob(os.path.join(run, "review_*.json")))}
    photos = list(key)
    sources = list(next(iter(key.values())).values())
    scores, best = {}, {}
    for m, ph in reviews.items():
        for p in photos:
            d = ph.get(p)
            if d is None:
                continue
            for L, s in key[p].items():
                if L in d:
                    scores[(m, p, s)] = d[L]
            if d.get("best") in key[p]:
                best[(m, p)] = key[p][d["best"]]

    def mean(vals):
        vals = [v for v in vals if v is not None and not (isinstance(v, float) and np.isnan(v))]
        return float(np.mean(vals)) if vals else None

    # Holdout vs tuned, from the photo paths and scripts/sky-eval/tuned.txt.
    tuned = [l.strip().replace("\\", "/") for l in open(os.path.join(os.path.dirname(os.path.abspath(__file__)), "tuned.txt"))
             if l.strip() and not l.startswith("#")]
    paths = {os.path.splitext(os.path.basename(l.strip()))[0]: l.strip().replace("\\", "/")
             for l in open(os.path.join(run, "photos.txt"), encoding="utf-8") if l.strip()}

    def is_tuned(p):  # a stem, or a folder ("commons/") among the photo's path components
        dirs = paths.get(p, "").split("/")[:-1]
        for t in tuned:
            if t.endswith("/"):
                want = [c for c in t.split("/") if c]
                if want and any(dirs[i:i + len(want)] == want for i in range(len(dirs) - len(want) + 1)):
                    return True
            elif p == t:
                return True
        return False
    group = {p: "tuned" if is_tuned(p) else "holdout" for p in photos}

    def stats(subset):
        out = {}
        for s in sources:
            # A reviewer may leave a criterion out; mean() skips the missing ones.
            e = {c: mean([scores[(m, p, s)].get(c) for m in reviews for p in subset if (m, p, s) in scores])
                 for c in CRITERIA}
            e["best_votes"] = sum(1 for (m, p), v in best.items() if v == s and p in subset)
            if s != "lightroom":
                e["iou"] = mean([M["photos"][p][f"{s}_vs_lightroom"]["iou"] for p in subset])
                e["boundary_f"] = mean([M["photos"][p][f"{s}_vs_lightroom"]["boundary_f"] for p in subset])
                if e["overall"] is not None:
                    e["composite"] = 50 * e["overall"] / 10 + 25 * e["iou"] + 25 * (e["boundary_f"] or 0)
            out[s] = e
        return out

    summary = {"photos": photos, "sources": sources, "reviewers": list(reviews), "group": group,
               "per_source": stats(photos), "per_group": {}, "per_photo": {}, "reviewer_agreement": {}, "notes": {}}
    for g in sorted(set(group.values())):
        sub = [p for p in photos if group[p] == g]
        summary["per_group"][g] = {"n": len(sub), "per_source": stats(sub)}
    for p in photos:
        summary["per_photo"][p] = {
            s: {m: scores[(m, p, s)]["overall"] for m in reviews if (m, p, s) in scores} for s in sources}
        summary["per_photo"][p]["best"] = {m: best.get((m, p)) for m in reviews}
        summary["notes"][p] = {s: {m: scores[(m, p, s)].get("notes", "") for m in reviews if (m, p, s) in scores}
                               for s in sources}
    vec = {m: [scores.get((m, p, s), {}).get("overall", np.nan) for p in photos for s in sources] for m in reviews}
    for a, b in itertools.combinations(reviews, 2):
        summary["reviewer_agreement"][f"{a}~{b}"] = float(spearmanr(vec[a], vec[b], nan_policy="omit").correlation)
    json.dump(summary, open(os.path.join(run, "summary.json"), "w"), indent=1)

    print("source      " + " ".join(f"{c:>11}" for c in CRITERIA) + "   best  IoU    BF     composite")
    for s, e in summary["per_source"].items():
        f = lambda v, w=11, d=1: f"{v:>{w}.{d}f}" if v is not None else " " * (w - 1) + "-"
        print(f"{s:<11} " + " ".join(f(e[c]) for c in CRITERIA)
              + f"   {e['best_votes']:>4}  {f(e.get('iou'), 5, 3)}  {f(e.get('boundary_f'), 5, 3)}  {f(e.get('composite'), 6)}")
    for g, d in summary["per_group"].items():
        print(f"{g} ({d['n']} photos): " + ", ".join(
            f"{s} " + (f"composite {e['composite']:.1f}" if e.get("composite") is not None else f"panel {e['overall'] or 0:.1f}")
            for s, e in d["per_source"].items()))
    for k, v in summary["reviewer_agreement"].items():
        print(f"agreement {k}: rho {v:.2f}")


if __name__ == "__main__":
    main()
