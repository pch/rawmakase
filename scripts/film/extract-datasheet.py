#!/usr/bin/env python3
# /// script
# requires-python = ">=3.11"
# dependencies = ["pdfplumber==0.11.10"]
# ///
"""Read a film's or paper's published curves out of its datasheet PDF.

Kodak's and Fujifilm's technical data sheets draw their curves as vector paths, so
the curves can be read exactly rather than traced by hand. Each stock is a file in
`stocks/`: where its datasheet is published and its SHA-256, the page, the plots on
it (where two ticks of each axis are, in PDF points) and which path in a plot is
which curve. The curves read from it are kept in `data/`, so a build needs no
network; `--refresh` downloads the datasheet again, refuses it if it differs from
the one the coordinates were read from, and reads the curves anew.

    uv run scripts/film/extract-datasheet.py portra-400             # summary
    uv run scripts/film/extract-datasheet.py --all --refresh        # every stock
    uv run scripts/film/extract-datasheet.py portra-400 --list 4    # a page's paths

A stock's plot names a page if it isn't the stock's `page`, its `region` (x0, top,
x1, bottom), `x` and `y` as two ticks each (point, value, point, value), and
`curves` as one sort key (`top`, `bottom`, `x0`, `x1`, `peak`, the x of a curve's
highest point, or `index`, the order `--list` prints them in) with the curves'
names in that order; `null` drops a curve the model doesn't use, and a name given
twice joins a curve drawn in pieces. `min_width` overrides the line weight that tells data from
gridlines (0.6 pt), and `min_points` the points a curve has at least (7; Fuji
draws some with a handful of Bézier segments); `chain` joins curves drawn as many
short pieces whose ends meet within that many points. To add a stock: run `--list` on its datasheet page, write its
file, and check the summary against the printed plots.
"""
import argparse
import hashlib
import io
import json
import re
import sys
import urllib.request
from pathlib import Path

import pdfplumber

HERE = Path(__file__).resolve().parent
STOCKS = HERE / 'stocks'
DATA = HERE / 'data'


def stocks():
    return sorted(p.stem for p in STOCKS.glob('*.json'))


def stock(name):
    return json.loads((STOCKS / f'{name}.json').read_text())


def axis(p0, v0, p1, v1):
    """Linear map from a PDF coordinate to a value, from two ticks."""
    return lambda p: v0 + (p - p0) * (v1 - v0) / (p1 - p0)


def dump(data):
    """JSON with lists of numbers, names and points on one line."""
    text = json.dumps(data, indent=1)
    return re.sub(r'\[\s*([^\[\]{}]*?)\s*\]', lambda m: '[' + ', '.join(i.strip() for i in m.group(1).split(',')) + ']',
                  text) + '\n'


def flatten(path, steps=16):
    """The points along a PDF path, with its Bézier segments subdivided."""
    points = []
    for op, *args in path:
        if op == 'm' or op == 'l':
            points.append(args[0])
        elif op == 'c':
            (x0, y0), (x1, y1), (x2, y2), (x3, y3) = points[-1], *args
            for i in range(1, steps + 1):
                t = i / steps
                u = 1 - t
                points.append((u**3 * x0 + 3 * u * u * t * x1 + 3 * u * t * t * x2 + t**3 * x3,
                               u**3 * y0 + 3 * u * u * t * y1 + 3 * u * t * t * y2 + t**3 * y3))
        elif op == 'v':
            # Bézier with the first control point at the current point.
            (x0, y0), (x2, y2), (x3, y3) = points[-1], *args
            for i in range(1, steps + 1):
                t = i / steps
                u = 1 - t
                points.append((u**3 * x0 + 3 * u * u * t * x0 + 3 * u * t * t * x2 + t**3 * x3,
                               u**3 * y0 + 3 * u * u * t * y0 + 3 * u * t * t * y2 + t**3 * y3))
        elif op == 'y':
            (x0, y0), (x1, y1), (x3, y3) = points[-1], *args
            for i in range(1, steps + 1):
                t = i / steps
                u = 1 - t
                points.append((u**3 * x0 + 3 * u * u * t * x1 + 3 * u * t * t * x3 + t**3 * x3,
                               u**3 * y0 + 3 * u * u * t * y1 + 3 * u * t * t * y3 + t**3 * y3))
    return points


def inside(c, region):
    x0, top, x1, bottom = region
    return c['x0'] >= x0 and c['x1'] <= x1 and c['top'] >= top and c['bottom'] <= bottom


def is_data_curve(c, min_width=0.6, min_points=7):
    """A plotted curve rather than a frame, gridline, tick or label leader: a stroked
    open path of a plot's line weight, with more than a handful of points and some
    extent in both directions."""
    return (c.get('stroke') and not c.get('fill') and len(c['pts']) >= min_points
            and (c.get('linewidth') or 0) >= min_width
            and c['x1'] - c['x0'] > 5 and c['bottom'] - c['top'] > 5)


def peak(c):
    """The x of a curve's highest point (smallest PDF y)."""
    return min(c['pts'], key=lambda p: p[1])[0]


def plot_curves(page, plot):
    """The data curves inside a plot's region, in page order. With `chain`, curves
    drawn as many short pieces (or dashes) are first joined end to end: pieces whose
    ends lie within `chain` points of each other become one curve."""
    width = plot.get('min_width', 0.6)
    if 'chain' in plot:
        pieces = [c for c in page.curves + page.lines
                  if inside(c, plot['region']) and c.get('stroke', True) and not c.get('fill')
                  and (c.get('linewidth') or 0) >= width]
        return [c for c in chain(pieces, plot['chain'])
                if is_data_curve(c, width, plot.get('min_points', 7))]
    return [c for c in page.curves
            if inside(c, plot['region'])
            and is_data_curve(c, width, plot.get('min_points', 7))]


def chain(pieces, tolerance):
    """Polylines joined from pieces whose ends meet within `tolerance` points, as
    curves with the fields the rest of this script reads."""
    segments = [flatten(c['path']) if c.get('path') else [(c['x0'], c['top']), (c['x1'], c['bottom'])]
                for c in pieces]
    used = [False] * len(segments)
    near = lambda a, b: abs(a[0] - b[0]) <= tolerance and abs(a[1] - b[1]) <= tolerance
    curves = []
    for i, first in enumerate(segments):
        if used[i]:
            continue
        used[i] = True
        line = list(first)
        grew = True
        while grew:
            grew = False
            for j, seg in enumerate(segments):
                if used[j]:
                    continue
                if near(line[-1], seg[0]):
                    line += seg[1:]
                elif near(line[-1], seg[-1]):
                    line += seg[::-1][1:]
                elif near(line[0], seg[-1]):
                    line = seg[:-1] + line
                elif near(line[0], seg[0]):
                    line = seg[::-1][:-1] + line
                else:
                    continue
                used[j] = grew = True
        xs, ys = [p[0] for p in line], [p[1] for p in line]
        curves.append({'pts': line, 'path': [('m', line[0])] + [('l', p) for p in line[1:]],
                       'x0': min(xs), 'x1': max(xs), 'top': min(ys), 'bottom': max(ys),
                       'stroke': True, 'fill': False, 'linewidth': max((c.get('linewidth') or 0) for c in pieces)})
    return curves


def read_plot(page, plot):
    curves = plot_curves(page, plot)
    (key, names), = plot['curves'].items()
    if len(curves) != len(names):
        raise ValueError(f'expected {len(names)} curves in {plot["region"]}, found {len(curves)}: '
                         + ', '.join(f'x0={c["x0"]:.1f} top={c["top"]:.1f} n={len(c["pts"])}' for c in curves))
    if key != 'index':
        curves.sort(key=peak if key == 'peak' else lambda c: c[key])
    to_x, to_y = axis(*plot['x']), axis(*plot['y'])
    pieces = {}
    for name, c in zip(names, curves):
        # A name given more than once joins the pieces of a curve drawn in several paths.
        if name is not None:
            pieces.setdefault(name, []).extend((to_x(x), to_y(y)) for x, y in flatten(c['path']))
    out = {}
    for name, pts in pieces.items():
        pts.sort()
        # Drop repeated x values left by subdivision, keeping the curve a function of x.
        clean = []
        for x, y in pts:
            if clean and abs(x - clean[-1][0]) < 1e-6:
                continue
            clean.append((round(x, 4), round(y, 4)))
        out[name] = clean
    return out


def list_page(page):
    """A page's paths (data curves numbered as `index` counts them within a region at
    the default line weight), tick-like lines and numeric labels."""
    for c in page.curves:
        mark = 'data ' if is_data_curve(c) else '     '
        print(f'{mark}curve n={len(c["pts"]):3} x0={c["x0"]:6.1f} x1={c["x1"]:6.1f} top={c["top"]:6.1f} '
              f'bottom={c["bottom"]:6.1f} width={c.get("linewidth") or 0:.2f}')
    for line in page.lines:
        if min(line['x1'] - line['x0'], line['bottom'] - line['top']) < 1:
            print(f'line x0={line["x0"]:6.2f} x1={line["x1"]:6.2f} top={line["top"]:6.2f} bottom={line["bottom"]:6.2f}')
    for w in page.extract_words():
        if any(ch.isdigit() for ch in w['text']) and len(w['text']) < 6:
            print(f'label {w["text"]!r} x={(w["x0"] + w["x1"]) / 2:6.2f} y={(w["top"] + w["bottom"]) / 2:6.2f}')


def download(film):
    """The datasheet's bytes, checked against the one its coordinates were read from."""
    request = urllib.request.Request(film['url'], headers={'User-Agent': 'Mozilla/5.0 (RAWmakase film looks)'})
    with urllib.request.urlopen(request, timeout=60) as response:
        pdf = response.read()
    digest = hashlib.sha256(pdf).hexdigest()
    if digest != film['sha256']:
        raise ValueError(f'{film["url"]} changed (SHA-256 {digest}): check its plots before updating its stock')
    return pdf


def extract(name):
    """A stock's curves, read from its datasheet downloaded now."""
    film = stock(name)
    pdf = pdfplumber.open(io.BytesIO(download(film)))
    data = {k: v for k, v in film.items() if k != 'plots'}
    data['curves'] = {plot_name: read_plot(pdf.pages[plot.get('page', film['page']) - 1], plot)
                      for plot_name, plot in film['plots'].items()}
    return data


def datasheet(name, refresh=False):
    """A stock's curves: from `data/` when they were read from the datasheet its stock
    file names, else read from it now and kept there."""
    film = stock(name)
    cached = DATA / f'{name}.json'
    if not refresh and cached.exists():
        data = json.loads(cached.read_text())
        if data.get('sha256') == film['sha256'] and data.get('plots') == film['plots']:
            return {**{k: v for k, v in film.items() if k != 'plots'}, 'curves': data['curves']}
    data = extract(name)
    DATA.mkdir(exist_ok=True)
    cached.write_text(dump({'sha256': film['sha256'], 'plots': film['plots'], 'curves': data['curves']}))
    return data


def summary(name, data):
    for plot, curves in data['curves'].items():
        for curve, pts in curves.items():
            print(f'{name:22} {plot:15} {curve:17} {len(pts):4} points, x {pts[0][0]:7.2f}..{pts[-1][0]:7.2f}, '
                  f'y {min(p[1] for p in pts):6.3f}..{max(p[1] for p in pts):6.3f}')


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('stock', nargs='?', help=', '.join(stocks()))
    ap.add_argument('--all', action='store_true', help='every stock in stocks/')
    ap.add_argument('--refresh', action='store_true', help='download the datasheets and read them again')
    ap.add_argument('--list', type=int, metavar='PAGE', help="print a page's paths, ticks and labels")
    args = ap.parse_args()
    if args.list:
        list_page(pdfplumber.open(io.BytesIO(download(stock(args.stock)))).pages[args.list - 1])
        return
    names = stocks() if args.all else [args.stock]
    failed = []
    for name in names:
        try:
            summary(name, datasheet(name, args.refresh))
        except Exception as e:  # report every stock, then fail
            print(f'{name}: {e}', file=sys.stderr)
            failed.append(name)
    if failed:
        sys.exit(f'failed: {", ".join(failed)}')


if __name__ == '__main__':
    main()
