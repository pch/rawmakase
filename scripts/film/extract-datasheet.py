#!/usr/bin/env python3
# /// script
# requires-python = ">=3.11"
# dependencies = ["pdfplumber==0.11.10"]
# ///
"""Read a film's or paper's published curves out of its Kodak datasheet PDF.

Kodak's technical data sheets draw their curves as vector paths, so the curves can
be read exactly rather than traced by hand. Each entry in FILMS names where its PDF
is published and its SHA-256, the page, the plots on it (where each axis' ticks
are, in PDF points), and which path in a plot is which curve. The datasheets are
not kept in the repository: `datasheet()` downloads one on every run, refuses it if
it differs from the one these coordinates were read from, and extracts its curves
in memory, which `film-look.py` turns into a look profile.

    uv run scripts/film/extract-datasheet.py portra-400            # summary
    uv run scripts/film/extract-datasheet.py portra-400 -o p.json  # the curves
    uv run scripts/film/extract-datasheet.py portra-400 --list 4   # a page's paths

To add a film: render its datasheet page, run with `--list` to see its paths and
tick marks, and add an entry to FILMS.
"""
import argparse
import hashlib
import io
import json
import sys
import urllib.request
from pathlib import Path

import pdfplumber


def axis(p0, v0, p1, v1):
    """Linear map from a PDF coordinate to a value, from two ticks."""
    return lambda p: v0 + (p - p0) * (v1 - v0) / (p1 - p0)


FILMS = {
    'portra-400': {
        'url': 'https://imaging.kodakalaris.com/sites/default/files/files/products/e4050_portra_400.pdf',
        'sha256': 'e83ac6775d37832a4cb466892a3e1cf4c88917a6ee93384e59d6924b1cd97e3a',
        'name': 'Kodak Portra 400',
        'source': 'Kodak Alaris, KODAK PROFESSIONAL PORTRA 400 Film, publication E-4050 (2016-02)',
        'type': 'negative',
        'page': 4,
        'notes': {
            'characteristic': 'Daylight exposure, Status M densitometry, Log H Ref -1.44 (normal exposure of a midscale neutral).',
            'sensitivity': 'Daylight, Status M, sensitivity = 1/exposure (erg/cm2) for 0.2 above D-min.',
            'dye_density': 'Diffuse spectral density of a midscale neutral and of D-min (no per-dye curves published).',
        },
        'log_h_ref': -1.44,
        'plots': {
            # Ticks: x -4.0 at 81.16 and 0.0 at 228.74; density 0 at 287.37 and 3 at 148.97.
            'characteristic': {
                'region': (81, 100, 266, 288),
                'x': axis(81.16, -4.0, 228.74, 0.0), 'y': axis(287.37, 0.0, 148.97, 3.0),
                'curves': {'top': ['blue', 'green', 'red']},
            },
            # Gridlines: 400 nm at 136.4, 600 nm at 216.5; log sensitivity 1 at 456.7, 3 at 380.9.
            'sensitivity': {
                'region': (76, 343, 277, 495),
                'x': axis(136.4, 400., 216.5, 600.), 'y': axis(456.7, 1.0, 380.9, 3.0),
                'curves': {'x0': ['blue', 'green', 'red']},
            },
            # Ticks: 400 nm at 357.14, 700 nm at 541.74; density 0 at 288.06, 2.5 at 103.71.
            'dye_density': {
                'region': (356, 104, 543, 289),
                'x': axis(357.14, 400., 541.74, 700.), 'y': axis(288.06, 0.0, 103.71, 2.5),
                'curves': {'top': ['midscale_neutral', 'minimum']},
            },
        },
    },
    'kodachrome-64': {
        'url': 'https://125px.com/docs/film/kodak/e55-2009_06.pdf',
        'sha256': '89ef8788a2a830e84e342d9f6f200336f31b0327adbc4ae5396517b3cfc0d2aa',
        'name': 'Kodak Kodachrome 64',
        'source': 'Eastman Kodak, KODACHROME 25, 64, and 200 Professional Film, publication E-55 (2009-06), page 4',
        'type': 'reversal',
        'page': 4,
        'notes': {
            'characteristic': 'Daylight exposure 1/50 s, process K-14, Status A densitometry.',
            'sensitivity': 'Effective exposure 1.4 s, K-14, E.N.D. density 1.00, sensitivity = 1/exposure (erg/cm2).',
            'dye_density': 'Dyes normalized to form a visual density of 1.0 for a viewing illuminant of 3200 K.',
        },
        'viewing_illuminant_k': 3200,
        'plots': {
            # Ticks: x -3.0 at 72.07 and 0.0 at 210.46; density 0 at 302.0 and 3 at 163.62.
            'characteristic': {
                'region': (72, 117, 257, 302.5),
                'x': axis(72.07, -3.0, 210.46, 0.0), 'y': axis(302.0, 0.0, 163.62, 3.0),
                'curves': {'top': ['red', 'green', 'blue']},
            },
            # Gridlines: 300 nm at 94.75, 700 nm at 255.52; log sensitivity 1 at 424.68, -1 at 500.42.
            'sensitivity': {
                'region': (74, 386, 276, 539),
                'x': axis(94.75, 300., 255.52, 700.), 'y': axis(424.68, 1.0, 500.42, -1.0),
                'curves': {'x0': ['blue', 'green', 'red']},
            },
            # Ticks: 400 nm at 350.78, 700 nm at 535.13; density 0 at 303.95, 1.0 at 176.94.
            'dye_density': {
                'region': (350, 119, 536, 305),
                'x': axis(350.78, 400., 535.13, 700.), 'y': axis(303.95, 0.0, 176.94, 1.0),
                'curves': {'top': ['visual_neutral', 'cyan', 'magenta', 'yellow']},
            },
        },
    },
    'endura-premier': {
        'url': 'https://business.kodakmoments.com/sites/default/files/files/products/e4070.pdf',
        'sha256': '94ead00f26a4bbf8ce1a991ff73add049514e38f32d6e88aa7a8a1e3e2d7982e',
        'name': 'Kodak Endura Premier',
        'source': 'Kodak Alaris, KODAK PROFESSIONAL ENDURA Premier Paper, publication E-4070 (2017-12)',
        'type': 'paper',
        'page': 4,
        'notes': {
            'characteristic': 'Exposure 0.5 s, process RA-4, Status A densitometry.',
            'sensitivity': 'Effective exposure 0.5 s, RA-4, sensitivity = 1/exposure (erg/cm2).',
            'dye_density': 'Diffuse spectral density of each dye, each normalized to a peak of 1.0 (page 5).',
        },
        'plots': {
            # Ticks: x -2.0 at 426.16 and -1.0 at 487.72; density 1 at 195.65 and 2 at 134.07.
            'characteristic': {
                'region': (364, 80, 552, 258),
                'x': axis(426.16, -2.0, 487.72, -1.0), 'y': axis(195.65, 1.0, 134.07, 2.0),
                'curves': {'top': ['red', 'green', 'blue']},
            },
            # Gridlines: 400 nm at 410.75, 700 nm at 531.03; log sensitivity 1 at 353.61, -1 at 429.41.
            'sensitivity': {
                'region': (350, 315, 552, 468),
                'x': axis(410.75, 400., 531.03, 700.), 'y': axis(353.61, 1.0, 429.41, -1.0),
                'curves': {'x0': ['blue', 'green', 'red']},
            },
            # Ticks: 450 nm at 119.63, 650 nm at 242.54; density 0.5 at 198.02 and 1.0 at 161.19.
            'dye_density': {
                'page': 5,
                'region': (88, 80, 275, 236),
                'x': axis(119.63, 450., 242.54, 650.), 'y': axis(198.02, 0.5, 161.19, 1.0),
                'curves': {'peak': ['yellow', 'magenta', 'cyan']},
            },
        },
    },
    'radiance-iii': {
        'url': 'https://125px.com/docs/unsorted/kodak/e1766.pdf',
        'sha256': 'f44b76b7f19bddbe9ff01f4539f0936abfd32900b77896309c7ace89c998e83c',
        'name': 'Kodak Ektachrome Radiance III',
        'source': 'Eastman Kodak, KODAK EKTACHROME RADIANCE III Paper, publication E-1766 (2003-02)',
        'type': 'paper',
        'page': 5,
        'notes': {
            'characteristic': 'Reversal paper. Exposure 1/2 s, 2850 K + CC filtration, process R-3, Status A reflection densitometry.',
            'sensitivity': 'Effective exposure 1 s, R-3, sensitivity = 1/exposure (erg/cm2).',
            'dye_density': 'Diffuse spectral density of each dye, process R-3.',
        },
        'plots': {
            # Ticks: x 1.0 at 132.77 and 3.0 at 225.01; density 1 at 217.71 and 3 at 125.47.
            'characteristic': {
                'region': (86, 100, 272, 265),
                'x': axis(132.77, 1.0, 225.01, 3.0), 'y': axis(217.71, 1.0, 125.47, 3.0),
                'curves': {'top': ['red', 'blue', 'green']},
            },
            # Gridlines: 400 nm at 284.96, 650 nm at 385.44; log sensitivity 1 at 382.38, -1 at 458.11.
            'sensitivity': {
                'region': (224, 344, 426, 496),
                'x': axis(284.96, 400., 385.44, 650.), 'y': axis(382.38, 1.0, 458.11, -1.0),
                'curves': {'x0': ['blue', 'green', 'red']},
            },
            # Ticks: 400 nm at 362.79, 600 nm at 485.68; density 0.5 at 229.41 and 2.0 at 118.81.
            'dye_density': {
                'region': (362, 100, 548, 267),
                'x': axis(362.79, 400., 485.68, 600.), 'y': axis(229.41, 0.5, 118.81, 2.0),
                'curves': {'peak': ['yellow', 'magenta', 'cyan']},
            },
        },
    },
}


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


def is_data_curve(c):
    """A plotted curve rather than a frame, gridline, tick or label leader: a stroked
    open path of a plot's line weight, with more than a handful of points and some
    extent in both directions."""
    return (c.get('stroke') and not c.get('fill') and len(c['pts']) > 6
            and (c.get('linewidth') or 0) >= 0.6
            and c['x1'] - c['x0'] > 5 and c['bottom'] - c['top'] > 5)


def peak(c):
    """The x of a curve's highest point (smallest PDF y)."""
    return min(c['pts'], key=lambda p: p[1])[0]


def read_plot(page, plot):
    curves = [c for c in page.curves if inside(c, plot['region']) and is_data_curve(c)]
    (key, names), = plot['curves'].items()
    if len(curves) != len(names):
        sys.exit(f'expected {len(names)} curves in {plot["region"]}, found {len(curves)}: '
                 + ', '.join(f'x0={c["x0"]:.1f} top={c["top"]:.1f} n={len(c["pts"])}' for c in curves))
    curves.sort(key=peak if key == 'peak' else lambda c: c[key])
    out = {}
    for name, c in zip(names, curves):
        pts = [(plot['x'](x), plot['y'](y)) for x, y in flatten(c['path'])]
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
    for c in page.curves:
        print(f'curve n={len(c["pts"]):3} x0={c["x0"]:6.1f} x1={c["x1"]:6.1f} top={c["top"]:6.1f} '
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
        sys.exit(f'{film["url"]} changed (SHA-256 {digest}): check its plots before updating FILMS')
    return pdf


def datasheet(name):
    """A film's or paper's curves, read from its datasheet downloaded now."""
    film = FILMS[name]
    pdf = pdfplumber.open(io.BytesIO(download(film)))
    data = {k: v for k, v in film.items() if k not in ('plots', 'page', 'sha256')}
    data['curves'] = {plot_name: read_plot(pdf.pages[plot.get('page', film['page']) - 1], plot)
                      for plot_name, plot in film['plots'].items()}
    return data


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('film', help=', '.join(FILMS))
    ap.add_argument('-o', '--out', type=Path, help='write the curves to this JSON file')
    ap.add_argument('--list', type=int, metavar='PAGE', help="print a page's paths, ticks and labels")
    args = ap.parse_args()
    if args.list:
        list_page(pdfplumber.open(io.BytesIO(download(FILMS[args.film]))).pages[args.list - 1])
        return
    data = datasheet(args.film)
    for plot, curves in data['curves'].items():
        for name, pts in curves.items():
            print(f'{plot:15} {name:17} {len(pts):4} points, x {pts[0][0]:7.2f}..{pts[-1][0]:7.2f}, '
                  f'y {min(p[1] for p in pts):6.3f}..{max(p[1] for p in pts):6.3f}')
    if args.out:
        args.out.write_text(json.dumps(data, indent=1) + '\n')
        print(args.out)


if __name__ == '__main__':
    main()
