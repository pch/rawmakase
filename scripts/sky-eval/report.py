"""Build RUN_DIR/report/index.html with its images in RUN_DIR/report/img/.

Usage: report.py --run RUN_DIR [--title TITLE] [--findings FINDINGS.html]

The page links its images as files (img/<stem>.jpg), never as data: URIs, so it can be
published as an Artifact with `files` or opened locally. --findings inserts hand-written
HTML (per-photo findings, conclusions) after the tables.
"""
import argparse, html, json, os, re, statistics
from PIL import Image

CSS = open(os.path.join(os.path.dirname(os.path.abspath(__file__)), "report.css"), encoding="utf-8").read()


VIEWER = """
<dialog id="viewer" aria-label="Panel full screen">
  <div class="vbar">
    <span id="vcap"></span>
    <span class="vbtns">
      <button type="button" id="vprev" aria-label="Previous panel">←</button>
      <button type="button" id="vnext" aria-label="Next panel">→</button>
      <button type="button" id="vfit" aria-label="Toggle fit and 100 %">100 %</button>
      <button type="button" id="vfs" aria-label="Browser full screen">⛶</button>
      <button type="button" id="vclose" aria-label="Close">✕</button>
    </span>
  </div>
  <div id="vstage" class="fit"><img id="vimg" alt=""></div>
</dialog>
<script>
(() => {
  // Pin the per-photo table's second header row right under the first, whatever its height.
  for (const t of document.querySelectorAll("table.perphoto")) {
    const first = t.tHead && t.tHead.rows[0];
    if (first) for (const th of t.tHead.rows[1].cells) th.style.top = first.getBoundingClientRect().height + "px";
  }
})();
(() => {
  const items = [...document.querySelectorAll("button.zoom")];
  const dlg = document.getElementById("viewer"), img = document.getElementById("vimg"),
        cap = document.getElementById("vcap"), stage = document.getElementById("vstage"),
        fit = document.getElementById("vfit");
  let i = 0;
  const show = (n) => {
    i = (n + items.length) % items.length;
    img.src = items[i].dataset.full;
    img.alt = items[i].dataset.caption;
    cap.textContent = `${i + 1} / ${items.length} · ${items[i].dataset.caption}`;
    stage.scrollTo(0, 0);
  };
  const setFit = (on) => { stage.classList.toggle("fit", on); fit.textContent = on ? "100 %" : "Fit"; };
  items.forEach((b, n) => b.addEventListener("click", () => { setFit(true); show(n); dlg.showModal(); }));
  document.getElementById("vprev").onclick = () => show(i - 1);
  document.getElementById("vnext").onclick = () => show(i + 1);
  document.getElementById("vclose").onclick = () => dlg.close();
  fit.onclick = () => setFit(!stage.classList.contains("fit"));
  img.onclick = (e) => {
    const wasFit = stage.classList.contains("fit");
    const r = img.getBoundingClientRect(), fx = (e.clientX - r.left) / r.width, fy = (e.clientY - r.top) / r.height;
    setFit(!wasFit);
    if (wasFit) requestAnimationFrame(() => stage.scrollTo(fx * img.naturalWidth - stage.clientWidth / 2, fy * img.naturalHeight - stage.clientHeight / 2));
  };
  document.getElementById("vfs").onclick = () => {
    try {
      const p = document.fullscreenElement ? document.exitFullscreen() : dlg.requestFullscreen();
      if (p && p.catch) p.catch(() => {});
    } catch (_) {}
  };
  dlg.addEventListener("keydown", (e) => {
    if (e.key === "ArrowLeft") { show(i - 1); e.preventDefault(); }
    if (e.key === "ArrowRight") { show(i + 1); e.preventDefault(); }
  });
  dlg.addEventListener("close", () => { if (document.fullscreenElement) document.exitFullscreen().catch(() => {}); });
})();
</script>
"""


def fmt(v, d=1):
    return "–" if v is None or v != v else f"{v:.{d}f}"


def timings(run):
    """Per source ("candidate", "baseline"), per photo stem: total ms, and where the dump
    reported them, the model's, the edge's and the modes' ms and which modes ran. A
    source dumped more than once in the run keeps its last dump."""
    path = os.path.join(run, "timings.txt")
    out = {}
    if not os.path.exists(path):
        return out
    src = None
    stem = lambda f: os.path.splitext(os.path.basename(f))[0]
    for line in open(path, encoding="utf-8", errors="replace"):
        line = line.strip()
        if line.startswith("# "):
            src = line[2:].strip()
            out[src] = {}
            continue
        if src is None:
            continue
        m = re.match(r"^timing (.+) model (\d+) edge (\d+) modes (\d+) sparse ([01]) dense ([01])$", line)
        if m:
            out[src].setdefault(stem(m[1]), {}).update(
                model=int(m[2]), edge=int(m[3]), modes=int(m[4]), sparse=m[5] == "1", dense=m[6] == "1")
            continue
        m = re.match(r"^(.+) \d+x\d+ (\d+) ms$", line)
        if m:
            out[src].setdefault(stem(m[1]), {})["total"] = int(m[2])
    return out


def mean(xs):
    xs = [x for x in xs if x is not None]
    return sum(xs) / len(xs) if xs else None


def runtime_section(T, photos, D, edges):
    """The run time per photo of both sources and where the candidate's went."""
    cand, base = T.get("candidate", {}), T.get("baseline", {})
    both = [p for p in photos if cand.get(p, {}).get("total") is not None and base.get(p, {}).get("total") is not None]
    if not both:
        return ""
    split = [p for p in both if "model" in cand[p]]
    s = lambda ms: fmt(ms / 1000 if ms is not None else None, 2)
    signed = lambda ms: "–" if ms is None else f"{ms / 1000:+.2f}"  # "+0.30", "-0.30"
    pct = lambda a, b: fmt(100 * a / b if a is not None and b else None, 0) + " %"
    spct = lambda a, b: f"{100 * a / b:+.0f} %" if a is not None and b else "– %"
    tb, tc = mean(base[p]["total"] for p in both), mean(cand[p]["total"] for p in both)
    h = ['<section><h2>Run time</h2>',
         f'<p>Seconds per photo for <code>select</code> (preparing the input, the model, the sky\'s edge), '
         f'release build, one photo after another on the same machine in this run; {len(both)} photos timed '
         f'on both sides. Input long edge: {D("baseline")} {edges.get("baseline", "?")} px, '
         f'{D("candidate")} {edges.get("candidate", "?")} px.</p>']
    h.append('<div class="scroll fit"><table class="grid"><thead><tr><th>Seconds per photo</th><th class="n">Mean</th>'
             '<th class="n">Median</th><th class="n">Slowest</th></tr></thead><tbody>')
    stats = {}
    for src, rows in [("baseline", base), ("candidate", cand)]:
        v = sorted(rows[p]["total"] for p in both)
        stats[src] = [mean(v), statistics.median(v), v[-1]]
        h.append(f'<tr><td>{D(src)}</td>' + "".join(f'<td class="n">{s(x)}</td>' for x in stats[src]) + '</tr>')
    # Mean and median only: the slowest photo of each source need not be the same photo.
    d = [c - b for b, c in zip(stats["baseline"][:2], stats["candidate"][:2])]
    h.append('<tr class="sum"><td>Longer by</td>' + "".join(f'<td class="n">{signed(x)}</td>' for x in d) + '<td class="n"></td></tr>')
    h.append('<tr class="sum"><td></td>' + "".join(f'<td class="n">{spct(x, b)}</td>' for x, b in zip(d, stats["baseline"])) + '<td class="n"></td></tr>')
    h.append("</tbody></table></div>")
    if split:
        ran = [p for p in split if cand[p]["sparse"] or cand[p]["dense"]]
        idle = [p for p in split if p not in ran]
        sparse_n = sum(cand[p]["sparse"] for p in split)
        dense_n = sum(cand[p]["dense"] for p in split)
        both_n = sum(cand[p]["sparse"] and cand[p]["dense"] for p in split)
        tm = mean(cand[p]["model"] for p in split)
        te = mean(cand[p]["edge"] for p in split)
        tmodes = mean(cand[p]["modes"] for p in split)
        tsplit = mean(cand[p]["total"] for p in split)
        tbase = mean(base[p]["total"] for p in split)
        extra = tsplit - tbase
        h.append('<h3>Where the branch\'s time goes</h3>'
                 f'<p>The edge modes ran on <b>{len(ran)} of {len(split)}</b> photos '
                 f'(sparse on {sparse_n}, dense on {dense_n}, both on {both_n}); on the other {len(idle)} the edge '
                 f'is the model\'s answer snapped as main snaps it, and the modes cost only the check for a crown '
                 f'or a lattice.</p>')
        h.append('<div class="scroll fit"><table class="grid"><thead><tr><th rowspan="2">Photos</th>'
                 '<th rowspan="2" class="n">Count</th>'
                 f'<th colspan="4" class="grp">{D("candidate")}, s</th><th class="grp">{D("baseline")}, s</th>'
                 '<th colspan="2" class="grp">Longer by</th></tr><tr>'
                 '<th class="n first">Total</th><th class="n">Model</th><th class="n">Edge</th>'
                 '<th class="n" title="the part of the edge spent weighing and running the edge modes">Modes</th>'
                 '<th class="n first">Total</th><th class="n first">s</th><th class="n">%</th></tr></thead><tbody>')
        for label, ps in [("Modes ran", ran), ("No mode ran", idle), ("All", split)]:
            if not ps:
                continue
            a, b = mean(cand[p]["total"] for p in ps), mean(base[p]["total"] for p in ps)
            h.append(f'<tr{" class=\"sum\"" if label == "All" else ""}><td>{label}</td><td class="n">{len(ps)}</td>'
                     f'<td class="n first">{s(a)}</td>'
                     f'<td class="n">{s(mean(cand[p]["model"] for p in ps))}</td>'
                     f'<td class="n">{s(mean(cand[p]["edge"] for p in ps))}</td>'
                     f'<td class="n">{s(mean(cand[p]["modes"] for p in ps))}</td>'
                     f'<td class="n first">{s(b)}</td><td class="n first">{signed(a - b)}</td><td class="n">{spct(a - b, b)}</td></tr>')
        h.append("</tbody></table></div>")
        h.append(f'<p>On average {D("candidate")} takes <b>{s(extra)} s ({pct(extra, tbase)}) longer</b> than '
                 f'{D("baseline")}. The new edge code (weighing the kinds, the sparse and dense modes, mixing) '
                 f'accounts for <b>{s(tmodes)} s</b> of that, <b>{pct(tmodes, extra)}</b> of the difference and '
                 f'{pct(tmodes, tsplit)} of {D("candidate")}\'s time; the rest, {s(extra - tmodes)} s, is the model\'s '
                 f'input preparation and main\'s snapping at {edges.get("candidate", "?")} px instead of '
                 f'{edges.get("baseline", "?")} px; the photo\'s render is not timed. '
                 f'Model {s(tm)} s, edge {s(te)} s on average.</p>')
    else:
        h.append('<p>This run has no breakdown of the candidate\'s time (dumped by a harness without '
                 '<code>timing</code> lines).</p>')
    h.append("</section>")
    return "".join(h)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--run", required=True)
    ap.add_argument("--title", default="Select Sky Holdout Scorecard")
    ap.add_argument("--findings")
    ap.add_argument("--thumb", type=int, default=1400, help="long edge of the inline panels")
    ap.add_argument("--full", type=int, default=2824, help="long edge of the full-screen panels")
    ap.add_argument("--compact", action="store_true", help="two images per photo (overview, stacked crops), used inline and full screen: for publishing large runs")
    args = ap.parse_args()
    run = args.run
    S = json.load(open(os.path.join(run, "summary.json")))
    M = json.load(open(os.path.join(run, "metrics.json")))
    key = json.load(open(os.path.join(run, "key.json")))
    meta = json.load(open(os.path.join(run, "run.json"))) if os.path.exists(os.path.join(run, "run.json")) else {}
    out = os.path.join(run, "report")
    os.makedirs(os.path.join(out, "img"), exist_ok=True)
    base, cand = M["baseline"], M["candidate"]
    order = [base, cand, "lightroom"]
    srcs = sorted(S["sources"], key=lambda x: order.index(x) if x in order else len(order))
    rv = S["reviewers"]
    names = M.get("display", {})
    D = lambda s: html.escape(names.get(s, s))

    h = ['<!doctype html>', '<meta charset="utf-8">', f"<title>{html.escape(args.title)}</title>",
         '<meta name="claude:viewport-fit-sides" content="cover">',
         '<link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=IBM+Plex+Sans:wght@400;600&family=IBM+Plex+Mono:wght@400;600&family=Fraunces:opsz,wght@9..144,600&display=swap">',
         f"<style>{CSS}</style><main>"]
    h.append(f'<header style="display:grid;gap:8px"><div class="eyebrow">RAWmakase · Select Sky · '
             f'{html.escape(meta.get("candidate_ref", cand))} vs {html.escape(meta.get("baseline_ref", base))} · '
             f'{html.escape(meta.get("date", ""))}</div><h1>{html.escape(args.title)}</h1>'
             f'<p class="lede">{len(S["photos"])} photos. Reviewers: {", ".join(rv)} (blind, masks shuffled per photo). '
             f'Lightroom reference from exports: {html.escape(meta.get("lr_format", "see metrics.json"))}.</p></header>')

    h.append('<section><h2>Result</h2><div class="scores">')
    for s in srcs:
        e = S["per_source"][s]
        if "composite" in e:
            kind, big = "Score (composite)", fmt(e.get("composite"), 0)
            sub = f'panel {fmt(e["overall"])}/10 · IoU {fmt(e.get("iou"), 2)} · BF {fmt(e.get("boundary_f"), 2)}'
        else:
            kind, big = "Panel only (the reference for IoU and BF)", fmt(e["overall"]) + "<small>/10</small>"
            sub = "no score: IoU and BF are measured against it"
        h.append(f'<div class="score"><span class="src">{D(s)}</span><span class="kind">{kind}</span><b>{big}</b>'
                 f'<span>{sub} · best {e["best_votes"]}×</span></div>')
    h.append('</div><p style="margin-top:12px">Composite = 50 % panel (mean "overall" × 10) + 25 % IoU × 100 + 25 % boundary F × 100 against Lightroom.</p></section>')

    if len(S.get("per_group", {})) > 1:
        # One column per figure, each right-aligned under a right-aligned header; the best
        # figure of each kind in a row is marked.
        def kinds(s):  # the score where there is a panel, IoU and BF where measured, best votes for all
            e = S["per_source"][s]
            return ([("composite", "Score")] if "composite" in e else []) + [("overall", "Panel")] \
                + ([("iou", "IoU"), ("boundary_f", "BF")] if "iou" in e else []) + [("best_votes", "Best")]
        cols = [(s, k, lab) for s in srcs for k, lab in kinds(s)]
        h.append('<section><h2>Holdout and tuned photos</h2><div class="scroll fit"><table class="grid"><thead><tr>'
                 '<th rowspan="2">Group</th><th rowspan="2" class="n">Photos</th>'
                 + "".join(f'<th colspan="{sum(1 for c in cols if c[0] == s)}" class="grp">{D(s)}</th>' for s in srcs)
                 + "</tr><tr>" + "".join(f'<th class="n{" first" if i == 0 or cols[i - 1][0] != c[0] else ""}">{c[2]}</th>' for i, c in enumerate(cols))
                 + "</tr></thead><tbody>")
        for g, d in S["per_group"].items():
            vals = [d["per_source"][s].get(k) for s, k, _ in cols]
            best = {k: max((v for (s2, k2, _), v in zip(cols, vals) if k2 == k and v is not None), default=None) for _, k, _ in cols}
            if best.get("best_votes") == 0:  # no votes (no review): nothing to mark
                best["best_votes"] = None
            digits = {"composite": 1, "overall": 2, "iou": 3, "boundary_f": 3, "best_votes": 0}
            cells = "".join(
                f'<td class="n{" first" if i == 0 or cols[i - 1][0] != c[0] else ""}{" best" if v is not None and best[c[1]] is not None and round(v, digits[c[1]]) == round(best[c[1]], digits[c[1]]) and sum(1 for c2 in cols if c2[1] == c[1]) > 1 else ""}">{fmt(v, digits[c[1]])}</td>'
                for i, (c, v) in enumerate(zip(cols, vals)))
            h.append(f"<tr><td>{html.escape(g)}</td><td class=\"n\">{d['n']}</td>" + cells + "</tr>")
        h.append("</tbody></table></div><p>Holdout: photos the sky modes were not tuned on (scripts/sky-eval/tuned.txt). "
                 "Score = composite (see above); BF = boundary F; Best = how often a reviewer named the source "
                 "best. The best figure of each kind per row is marked.</p></section>")

    T = timings(run)
    h.append(runtime_section(T, S["photos"], D, {"baseline": meta.get("baseline_edge"), "candidate": meta.get("candidate_edge")}))

    # Per photo: a two-row header (source, then IoU / BF / panel), narrow numeric columns,
    # the photo name pinned left and the header pinned on top, in a box no taller than the
    # window so its horizontal scrollbar is always in reach; it may use the full screen width.
    ordered = [base] + [s for s in srcs if s not in (base, cand, "lightroom")] + ([cand] if cand != base else [])
    def short(s):  # column label; the full name goes in the tooltip
        if s == "lightroom":
            return "LR"
        word = names.get(s, s).split(" ")[0]
        return html.escape(word if len(word) <= 8 else ("branch" if s == cand else s))
    groups = [(short(s), D(s), [(f"{s}_vs_lightroom", "iou", "IoU"), (f"{s}_vs_lightroom", "boundary_f", "BF")]) for s in ordered]
    if base != cand:
        groups.append((f"{short(cand)} vs {short(base)}", f"{D(cand)} vs {D(base)}",
                       [(f"{cand}_vs_{base}", "iou", "IoU"), (f"{cand}_vs_{base}", "boundary_f", "BF")]))
    panel_srcs = ordered + ["lightroom"]
    # Best per row: IoU and BF among the sources against Lightroom, the panel among all
    # three; the source with the best panel is named in the last column.
    quality = [g for g in groups if g[2][0][0].endswith("_vs_lightroom")]
    h.append('<section><h2>Per photo</h2><p>IoU and boundary F against Lightroom; panel = mean '
             '"overall" of the reviewers, 0–10. In each row the best IoU, BF and panel are marked, and the last '
             'column names the source with the best panel ("=" for a tie). Names are shortened; hover for the full name.</p>'
             '<div class="wide"><div class="scroll tall fit"><table class="perphoto"><thead><tr><th rowspan="2" class="name">Photo</th>'
             + "".join(f'<th colspan="{len(c)}" class="grp" title="{t}">{g}</th>' for g, t, c in groups)
             + f'<th colspan="{len(panel_srcs)}" class="grp">Panel</th><th rowspan="2" class="grp">Best</th>'
             + ('<th colspan="3" class="grp">Seconds</th>' if T else "")
             + '<th rowspan="2" class="grp" title="the share of the photo where Lightroom\'s mask can be read; '
               'IoU and BF count only these pixels">valid %</th></tr><tr>'
             + "".join(f'<th class="n{" first" if j == 0 else ""}">{lab}</th>' for _, _, c in groups for j, (_, _, lab) in enumerate(c))
             + "".join(f'<th class="n{" first" if j == 0 else ""}" title="{D(s)}">{short(s)}</th>' for j, s in enumerate(panel_srcs))
             + (f'<th class="n first" title="{D(base)}">{short(base)}</th><th class="n" title="{D(cand)}">{short(cand)}</th>'
                '<th class="n" title="the edge modes\' share of the candidate\'s time">modes</th>' if T else "")
             + "</tr></thead><tbody>")
    def top(values):  # indexes of the largest values (rounded as shown), none if fewer than two
        shown = [None if v is None or v != v else round(v, 3) for v in values]
        known = [v for v in shown if v is not None]
        if len(known) < 2:
            return set()
        return {i for i, v in enumerate(shown) if v == max(known)}
    for p in S["photos"]:
        pp = S["per_photo"][p]
        panel = [sum(pp[s].values()) / len(pp[s]) if pp.get(s) else None for s in panel_srcs]
        best = {}
        for f in ["iou", "boundary_f"]:
            vals = [M["photos"][p][c[0][0]][f] for _, _, c in quality]
            best[f] = {quality[i][2][0][0] for i in top(vals)}
        win = top(panel)
        cells = []
        for gi, (_, _, c) in enumerate(groups):
            for j, (k, f, _) in enumerate(c):
                mark = " best" if k in best.get(f, set()) else ""
                cells.append(f'<td class="n{" first" if j == 0 else ""}{mark}">{fmt(M["photos"][p][k][f], 3)}</td>')
        cells += [f'<td class="n{" first" if j == 0 else ""}{" best" if j in win else ""}">{fmt(v)}</td>' for j, v in enumerate(panel)]
        winner = "=" if len(win) > 1 else (short(panel_srcs[next(iter(win))]) if win else "–")
        times = ""
        if T:
            tb, tc = T.get("baseline", {}).get(p, {}), T.get("candidate", {}).get(p, {})
            sec = lambda ms: fmt(ms / 1000 if ms is not None else None, 1)
            ran = "+".join(k for k in ["sparse", "dense"] if tc.get(k))
            times = (f'<td class="n first">{sec(tb.get("total"))}</td><td class="n">{sec(tc.get("total"))}</td>'
                     f'<td class="n" title="{ran or "no mode ran"}">{sec(tc.get("modes")) if ran else "–"}</td>')
        h.append(f'<tr><td class="name" title="{html.escape(p)}">{html.escape(p)}</td>' + "".join(cells)
                 + f'<td class="winner first">{winner}</td>' + times
                 + f'<td class="n first">{fmt(M["photos"][p].get("valid_pct"), 0)}</td></tr>')
    h.append("</tbody></table></div></div></section>")

    h.append('<section><h2>Claude panel</h2><div class="scroll"><table><thead><tr><th>Source</th>'
             + "".join(f"<th>{c.replace('_', ' ')}</th>" for c in ["bleed", "missed_sky", "edges", "sky_objects", "reflections", "overall"])
             + "<th>best</th></tr></thead><tbody>")
    for s in srcs:
        e = S["per_source"][s]
        h.append(f"<tr><td>{D(s)}</td>" + "".join(
            f'<td class="n">{fmt(e[c])}</td>' for c in ["bleed", "missed_sky", "edges", "sky_objects", "reflections", "overall"])
            + f'<td class="n">{e["best_votes"]}</td></tr>')
    h.append("</tbody></table></div><p>" + " · ".join(
        f"{html.escape(k)} ρ {v:.2f}" for k, v in S["reviewer_agreement"].items()) + "</p></section>")

    if args.findings:
        h.append(open(args.findings, encoding="utf-8").read())

    h.append('<section style="display:grid;gap:18px"><h2 style="margin:0">Panels and reviewer notes</h2>'
             '<p>Click a panel to open it full screen; ← → step through all panels, click toggles fit and 100 %, Esc closes.</p>')
    if not args.compact:
        os.makedirs(os.path.join(out, "img", "full"), exist_ok=True)
    for p in S["photos"]:
        # The reviewers wrote about letters; name the sources instead.
        # Only this photo's letters, and not the one in "X-ray" or "Y-axis".
        unblind = lambda t: re.sub(r"\b([XYZWVU])\b(?![-\w])", lambda m: names.get(key[p][m.group(1)], key[p][m.group(1)])
                                   if m.group(1) in key[p] else m.group(1), t)
        h.append(f'<div class="photo"><h3>{html.escape(p)}</h3><figure>')
        parts = {}
        for part in ["overview", "crop1", "crop2", "crop3"]:
            src = os.path.join(run, "panels", f"{p}__{part}.jpg")
            if not os.path.exists(src):  # runs from before named panels existed
                src = os.path.join(run, "review", f"{p}__{part}.jpg")
            if os.path.exists(src):
                parts[part] = Image.open(src).convert("RGB")
        if args.compact and any(k.startswith("crop") for k in parts):
            # One image for the three crops, stacked, to keep the file count down.
            crops = [v for k, v in parts.items() if k.startswith("crop")]
            w = max(c.width for c in crops)
            stack = Image.new("RGB", (w, sum(c.height for c in crops) + 8 * (len(crops) - 1)), (40, 40, 40))
            y = 0
            for c in crops:
                stack.paste(c, (0, y))
                y += c.height + 8
            parts = {"overview": parts["overview"], "crops": stack} if "overview" in parts else {"crops": stack}
        for part, im in parts.items():
            name = f"{p}__{part}.jpg"
            full = im.copy()
            full.thumbnail((args.full, args.full))
            if args.compact:  # the same file inline and full screen
                full.save(os.path.join(out, "img", name), quality=80)
                inline, big = f"img/{name}", f"img/{name}"
            else:
                full.save(os.path.join(out, "img", "full", name), quality=82)
                im.thumbnail((args.thumb, args.thumb))
                im.save(os.path.join(out, "img", name), quality=78)
                inline, big = f"img/{name}", f"img/full/{name}"
            caption = f"{p} · {part}"
            h.append(f'<button class="zoom" type="button" data-full="{html.escape(big)}" '
                     f'data-caption="{html.escape(caption)}"><img loading="lazy" alt="{html.escape(p)}: {part}, original and masks" '
                     f'src="{html.escape(inline)}"></button>')
        h.append(f'<figcaption>Red = sky, yellow = 50 % contour. Crops: where the masks disagree most.</figcaption></figure><ul>')
        for s in srcs:
            for m, n in S["notes"][p][s].items():
                o = S["per_photo"][p][s].get(m)
                h.append(f"<li><strong>{D(s)}</strong> · {html.escape(m)} {o}/10: {html.escape(unblind(n))}</li>")
        h.append("</ul></div>")
    h.append("</section></main>")
    h.append(VIEWER)
    open(os.path.join(out, "index.html"), "w", encoding="utf-8").write("\n".join(h))
    print(os.path.join(out, "index.html"))


if __name__ == "__main__":
    main()
