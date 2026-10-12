# Film looks

These scripts make RAWmakase's [film looks](../../docs/lightroom-profiles.md#film-looks)
and their RMKS Film presets from Kodak's, Fujifilm's and Agfa's published technical
datasheets.

```sh
uv run scripts/film/film-look.py --all                 # assets/looks, the presets and their lists
uv run scripts/film/film-look.py --all --resume        # finish a build that stopped
uv run scripts/film/film-look.py portra-400 --print --push 1 -o ~/film-looks   # one look, to try
uv run scripts/film/extract-datasheet.py --all --refresh   # read every datasheet again
```

They need [uv](https://docs.astral.sh/uv/); the Python dependencies are pinned in
each script, so a run reproduces the shipped files.

- `stocks/` has a file per film or paper: where its datasheet is published and its
  SHA-256, the page, where two ticks of each plot's axes are and which path is which
  curve, and what the datasheet says about the stock (speed, reference exposure,
  granularity, viewing light).
- `data/` keeps the curves read from each datasheet, so a build needs no network.
  `extract-datasheet.py --refresh` downloads the datasheets again, refuses one that
  changed, and reads them anew.
- `extract-datasheet.py` reads a datasheet's spectral sensitivity, characteristic and
  spectral dye density curves out of the PDF's vector paths. Its docstring describes
  the stock file format.
- `film-look.py` follows the light through the film (and the paper, for a print) for
  each entry of a 25³ RGB table, and writes a Lightroom look profile and a preset
  choosing it. `--all` makes every stock's looks, scanned or projected and printed,
  and a negative's +1 and -1, and lists them for the app
  (`camera_profiles/film_files.in`, `presets/film_files.in`). Its docstring
  describes the model and what it assumes where the datasheets say nothing.

To add a stock, run `extract-datasheet.py STOCK --list PAGE` on its datasheet to see
the page's paths, tick marks and labels, write `stocks/STOCK.json` (see the others),
run `extract-datasheet.py STOCK --refresh`, and check the curves it reads against the
printed plots. Datasheets whose plots are scanned images can't be read.
