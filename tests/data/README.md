# Test data

Small samples committed to the repository.

- **`BOM.md`** – bill of materials: every third-party-derived file with its origin
  (publisher, dataset page, download URL, retrieval date, original file), licence,
  licence evidence, required attribution, extraction rules, modifications,
  checksum, schema validation result, GDAL reference and axis-order evidence.
  **Generated — do not edit.**
- **`samples.toml`** – the curated list (source, selection rules, what each sample
  covers, reviewed licence, axis order and the evidence for it). Edit this, then rebuild.
- **`samples.json`** – machine-readable manifest, generated together with `BOM.md`.
- `samples/**` – the extracted samples; `*.gdal.txt` next to each is GDAL 3.13's
  `ogrinfo -al` output (GDAL's *interpretation*, not ground truth — see the axis-order
  check in the BOM).
- `gdal/` – GML snippets from GDAL's test suite (MIT, see `LICENSE-GDAL.txt`).

Rebuild after changing `samples.toml` (needs `example_data/` and Docker for GDAL):

    scripts/corpus/reference_data.py          # once: PRG + Natural Earth boundaries (~390 MB)
    scripts/corpus/build_samples.py [--only NAME] [--no-validate] [--no-gdal]

## Axis order

Every sample records its true coordinate order (`axis_order = "x/y"` or `"y/x"`), and the
build fails unless `scripts/corpus/axis_evidence.py` confirms it. The script reads every
position both ways and tests each reading against:

- `crs_area` — the CRS's area of use (always checked);
- `axis_places` — regions the features are known to lie in, justified by something other
  than the geometry (`why`): a TERYT code in the attributes or file name (PRG
  voivodeship/powiat/gmina), or the publisher's country (Natural Earth, 5 km tolerance);
- `axis_same_as` — another sample with the same features, e.g. the same WFS layer in
  another version.

The recorded order must have ≥ 95 % of positions inside every region, and at least one check
must put > 10 % of the swapped reading outside. `axis_scope = "source"` runs the checks on
the whole original document when a few sampled features can't tell the readings apart
(used for NGI: 3 positions fit Belgium both ways).

## Policy

Every file must be one of:
1. **Hand-written** for this project — project licence.
2. **Snippets from GDAL's test suite** (MIT) — keep the notice in `LICENSE-GDAL.txt`.
3. **Excerpts of openly licensed data** — listed in `BOM.md` with licence and attribution.

Licences are taken from the data owner's own terms (the dataset's ISO metadata, the terms
shipped with the data, the owner's open data policy), not from a service's GetCapabilities
`Fees`/`AccessConstraints`, which often describe access only and can be outdated (Vienna
still says CC BY 3.0 AT; the Estonian AF service says "none" while its data owner PRIA
licenses the data CC BY-SA 3.0 EE). Attributions follow the form each licence prescribes and
note that the data was changed (excerpted) where the licence requires it (dl-de/by, GeoNutzV,
SITG, NGI, CC).

Share-alike excerpts (CC BY-SA) are allowed: those files stay under their licence, which
is recorded in `BOM.md`; they are test inputs and are not combined into project code.
Data under a no-derivatives licence (e.g. CC BY-ND) is not excerpted. GDAL's
`autotest/ogr/data` files are not copied here (mixed provenance).
