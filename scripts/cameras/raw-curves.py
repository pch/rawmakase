#!/usr/bin/env python3
"""Write data/raw-curves.toml, the linearization curves RAWmakase's own RAW
decoders take from Adobe DNG Converter where the RAW file does not carry the
one Adobe reads it with.

Nikon's D3, D300 and D700 write their 12-bit lossy NEFs with a 257-point
curve of their own (linearization table version 0x44 0x20), but Adobe DNG
Converter 18.7 linearizes them with the 683-entry curve of the first kind's
files (version 0x44 0x10, the D40 to D3000 bodies' lossy NEFs), which these
files do not hold. Give it DNGs Adobe DNG Converter made of such files (one
or more, of any of the three bodies; they must all hold one curve): it reads
their LinearizationTable with exiftool and writes the table. Nothing of the
RAW or the DNG is kept.

  python3 scripts/cameras/raw-curves.py converted.dng [more.dng ...]
"""
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
TABLE = ROOT / 'data/raw-curves.toml'
HEADER = """# Linearization curves for RAWmakase's own RAW decoders
# (crates/rawmakase-raw/src/facts.rs), where the file does not carry the one
# Adobe DNG Converter reads it with. source is adobe-dng: the LinearizationTable
# Adobe DNG Converter 18.7 writes for the cameras' files (2026-10-17). Written by
# scripts/cameras/raw-curves.py.
"""
MODELS = ['D3', 'D300', 'D700']


def table(dng):
    values = subprocess.run(['exiftool', '-b', '-n', '-LinearizationTable', str(dng)],
                            capture_output=True, text=True, check=True).stdout.split()
    if not values:
        sys.exit(f'{dng}: no LinearizationTable')
    return [int(v) for v in values]


def main():
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    tables = [table(d) for d in sys.argv[1:]]
    if any(t != tables[0] for t in tables):
        sys.exit('the DNGs hold different curves')
    values = tables[0]
    lines = [HEADER, '[[curve]]', 'make = "Nikon"',
             'models = [' + ', '.join(f'"{m}"' for m in MODELS) + ']',
             'bits = 12', 'version = [0x44, 0x20]', 'source = "adobe-dng"', 'values = [']
    for i in range(0, len(values), 16):
        lines.append('    ' + ', '.join(str(v) for v in values[i:i + 16]) + ',')
    lines.append(']')
    TABLE.write_text('\n'.join(lines) + '\n')
    print(f'{TABLE}: {len(values)} values')


if __name__ == '__main__':
    main()
