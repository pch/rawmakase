# Film looks

These scripts make RAWmakase's [film looks](../../docs/lightroom-profiles.md#film-looks)
from Kodak's published technical datasheets. The datasheets are not kept in the
repository: every run downloads them, checks each against the SHA-256 its plot
coordinates were read from, and reads the curves in memory.

```sh
scripts/film/build-assets.sh       # assets/looks and assets/presets/Film
uv run scripts/film/film-look.py portra-400 -o ~/film-looks          # one look, to try
uv run scripts/film/film-look.py kodachrome-64 --print -o ~/film-looks
uv run scripts/film/extract-datasheet.py portra-400 -o portra.json   # the curves
```

They need [uv](https://docs.astral.sh/uv/) and a network connection; the Python
dependencies are pinned in each script, so a run reproduces the shipped files.

- `extract-datasheet.py` reads a datasheet's spectral sensitivity, characteristic and
  spectral dye density curves out of the PDF's vector paths. `FILMS` names each
  film's and paper's datasheet, its page and where each plot's axis ticks are.
- `film-look.py` follows the light through the film for each entry of a 33³ RGB table
  and writes a Lightroom look profile and a preset choosing it. Its docstring
  describes the model and what it assumes where the datasheets say nothing.

To add a film, run `extract-datasheet.py FILM --list PAGE` on its datasheet to see the
page's paths, tick marks and labels, add it to `FILMS` (and a name to `NAMES` and its
grain to `GRAIN` in `film-look.py`), and check the summary `extract-datasheet.py FILM`
prints against the printed plots.
