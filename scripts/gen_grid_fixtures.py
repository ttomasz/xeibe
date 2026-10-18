#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["pyproj==3.8.0"]
# ///
"""Small grid files for `crates/xeibe-reproject`'s committed tests
(`docs/reprojection.md`, level 3), in `tests/data/reprojection/grids/`.

* A few real PROJ-data grids, small and openly licensed, downloaded from
  `cdn.proj.org` and checked against pinned SHA-256 sums. Their operations
  and CRS pairs get PROJ's vectors from `scripts/gen_transformation_vectors.py
  --grids tests/data/reprojection/grids`.
* `tiled/us_noaa_stgeorge.tif`: the St. George Island NADCON grid in 16×16
  tiles, under its own name, so that a transform reading it loads only the
  tiles a box needs.
* The same public-domain grid (NOAA's `us_noaa_c2hpgn.tif`) re-encoded every
  way PROJ-data's GeoTIFFs can be (tiles, LZW, no compression, big-endian,
  scaled integers, rows from the south, pixel-is-area, subgrids), by GDAL in
  the `scripts/gdal` container, with PROJ's `hgridshift` on each as the
  oracle: `variants.jsonl`.

    scripts/gen_grid_fixtures.py
"""

from __future__ import annotations

import hashlib
import json
import math
import shutil
import subprocess
import sys
import urllib.request
from pathlib import Path

import pyproj

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "tests" / "data" / "reprojection" / "grids"
CDN = "https://cdn.proj.org/"

# file, sha256, producer, licence, what it is for
REAL = [
    (
        "us_noaa_stgeorge.tif",
        "8d34c59a8f0d700b1b723b146c6a24ca6634fc6773beede4219e4ddd7c92f07b",
        "NOAA",
        "Public Domain",
        "NADCON: St. George Island to NAD83 (1) and to WGS 84 (1), EPSG:1457 and 1698",
    ),
    (
        "us_noaa_nadcon5_sg1952_nad83_1986_stgeorge.tif",
        "bf11ee3e7c2ac25113cdc4cd919284b39448b7b3e2d65dd320180a2c6994e852",
        "NOAA",
        "Public Domain",
        "NADCON5, biquadratic: St. George Island to NAD83 (2), EPSG:8546",
    ),
    (
        "us_noaa_prvi.tif",
        "b2d96872b2600324eb1df8edd2af61cac747f4732e10bbde0152bb0857a657e2",
        "NOAA",
        "Public Domain",
        "NADCON: Puerto Rico to NAD83 (1) and to WGS 84 (4), EPSG:1461 and 15841",
    ),
    (
        "us_noaa_nadcon5_pr40_nad83_1986_prvi.tif",
        "91a00353598a30ffc24d3d00e3295da0a95ee378cc3dbcc1d149c6c75ace3931",
        "NOAA",
        "Public Domain",
        "NADCON5: Puerto Rico to NAD83 (2), EPSG:8668",
    ),
    (
        "us_noaa_pvhpgn.tif",
        "a6db010f956526a011ba719c45fe32e5aff987eeb1cb03f2b8338208a77c619d",
        "NOAA",
        "Public Domain",
        "NTv2 (HPGN): Puerto Rico to WGS 84 (3), with prvi the candidates for Puerto Rico to WGS 84",
    ),
    (
        "us_noaa_c2hpgn.tif",
        "0a8fbdae79524e93d19a5d471939c8270c1a907b770daf5b6b6493a58d71d7b1",
        "NOAA",
        "Public Domain",
        "Guam's HPGN grid, in no EPSG operation PROJ maps: the source of the variants",
    ),
    (
        "fr_ign_RGM04versRGM23.tif",
        "0ad51d816048e8ef0e2d419e45f0f71777e8396f3511a6907130373d7da8a7a8",
        "IGN France",
        "Licence Ouverte / Open Licence (Etalab)",
        "Geocentric translations by grid (IGN): RGM04 to RGM23 (2), EPSG:10683",
    ),
    (
        "fr_ign_RALD2016.tif",
        "60bc9eca05bd39f5b2f38dfbd0c7e86f4d9a858bb49455343b482caa2c512f6e",
        "IGN France",
        "Licence Ouverte / Open Licence (Etalab)",
        "Geoid models: RGAF09 to IGN 1992 LD height (1) and to IGN 2008 LD height (1), EPSG:5507 and 9131",
    ),
    (
        "fr_ign_RALDW842016.tif",
        "52e0cd958d2b89df0c0ea3cc012242082d9492c366b0c4b3e8c1711b51ee4cb8",
        "IGN France",
        "Licence Ouverte / Open Licence (Etalab)",
        "The same geoid models from RRAF 1991, EPSG:4566, 9132 and 9642: the other candidates",
    ),
    (
        "dk_kds_dvr90_evrf2019.tif",
        "eefa27633123c70ac2e608aec8750b9b9d730809d8da48ef469e37636f09eba6",
        "Agency for Climate Data (Denmark)",
        "CC BY 4.0",
        "Vertical offset grid, interpolated in ETRS89: DVR90 height to EVRF2019 height (1), EPSG:10858",
    ),
    (
        "nz_linz_wellht1953-nzvd2016.tif",
        "7dfd7f545692301956338387982f5ba4e65fb41217883486347d0965cea04824",
        "Land Information New Zealand",
        "CC BY 4.0",
        "Vertical offset grid, interpolated in NZGD2000: NZVD2016 height to Wellington 1953 height (1), (2), EPSG:7872 and 9324",
    ),
]
SOURCE = "us_noaa_c2hpgn.tif"

# Run inside the GDAL container: re-encode the source grid. Each variant
# keeps the values (and so PROJ's results) or changes them deliberately.
RECODE = r"""
import sys
import numpy as np
from osgeo import gdal
gdal.UseExceptions()
src_path, out = sys.argv[1], sys.argv[2]
src = gdal.Open(src_path)
gt = src.GetGeoTransform()
bands = [src.GetRasterBand(i + 1) for i in range(src.RasterCount)]
data = [b.ReadAsArray().astype(np.float64) for b in bands]
w, h = src.RasterXSize, src.RasterYSize

def write(name, options, arrays=data, geotransform=gt, dtype=gdal.GDT_Float32,
          scale=None, area=False, append=False, extra=None):
    path = f"{out}/{name}"
    opts = list(options) + (["APPEND_SUBDATASET=YES"] if append else [])
    ds = gdal.GetDriverByName("GTiff").Create(path, arrays[0].shape[1], arrays[0].shape[0],
                                               len(arrays), dtype, opts)
    ds.SetGeoTransform(geotransform)
    ds.SetProjection(src.GetProjection())
    md = dict(src.GetMetadata())
    md.update(extra or {})
    md["AREA_OR_POINT"] = "Area" if area else "Point"
    ds.SetMetadata(md)
    for i, a in enumerate(arrays):
        b = ds.GetRasterBand(i + 1)
        b.SetDescription(bands[i].GetDescription())
        b.SetUnitType(bands[i].GetUnitType())
        b.SetMetadata(bands[i].GetMetadata())
        if scale:
            b.SetScale(scale[0]); b.SetOffset(scale[1])
            a = np.round((a - scale[1]) / scale[0])
        b.WriteArray(a)
    ds = None
    print(name)

write("variant_tiled_deflate.tif", ["TILED=YES", "BLOCKXSIZE=16", "BLOCKYSIZE=16",
      "COMPRESS=DEFLATE", "PREDICTOR=3", "INTERLEAVE=PIXEL"])
write("variant_lzw_strips.tif", ["COMPRESS=LZW", "PREDICTOR=2", "INTERLEAVE=BAND", "BLOCKYSIZE=4"])
write("variant_raw_bigendian.tif", ["COMPRESS=NONE", "ENDIANNESS=BIG", "INTERLEAVE=PIXEL"])
write("variant_int32_scaled.tif", ["COMPRESS=DEFLATE", "PREDICTOR=2", "INTERLEAVE=PIXEL"],
      dtype=gdal.GDT_Int32, scale=(1e-6, -0.25))
# Rows stored from the south: a positive y resolution (GDAL writes a
# ModelTransformation tag).
write("variant_bottom_up.tif", ["COMPRESS=DEFLATE"], arrays=[a[::-1] for a in data],
      geotransform=(gt[0], gt[1], 0, gt[3] + gt[5] * (h - 1), 0, -gt[5]))
# Pixel-is-area: the same node positions, the corner half a cell out.
write("variant_pixel_is_area.tif", ["COMPRESS=DEFLATE"], area=True,
      geotransform=(gt[0] - gt[1] / 2, gt[1], 0, gt[3] - gt[5] / 2, 0, gt[5]))
# A parent and a finer child over its middle, with different values so that
# the result tells which grid was read.
cx0, cy0, cw, ch = w // 4, h // 4, w // 2, h // 2
child = [np.kron(a[cy0:cy0 + ch, cx0:cx0 + cw], np.ones((2, 2)))[: 2 * ch - 1, : 2 * cw - 1] + 0.05
         for a in data]
cgt = (gt[0] + gt[1] * cx0, gt[1] / 2, 0, gt[3] + gt[5] * cy0, 0, gt[5] / 2)
write("variant_subgrids.tif", ["COMPRESS=DEFLATE"], extra={"grid_name": "PARENT"})
write("variant_subgrids.tif", ["COMPRESS=DEFLATE"], arrays=child, geotransform=cgt, append=True,
      extra={"grid_name": "CHILD", "parent_grid_name": "PARENT"})
"""


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def download() -> list[dict]:
    OUT.mkdir(parents=True, exist_ok=True)
    rows = []
    for name, digest, producer, licence, purpose in REAL:
        path = OUT / name
        if not path.exists():
            print(f"downloading {CDN}{name}")
            with urllib.request.urlopen(CDN + name) as response:
                path.write_bytes(response.read())
        got = sha256(path)
        if digest and got != digest:
            sys.exit(f"{name}: sha256 {got}, expected {digest} (the CDN file changed?)")
        rows.append({"file": name, "sha256": got, "producer": producer, "licence": licence, "purpose": purpose})
    return rows


def recode() -> list[str]:
    for old in OUT.glob("variant_*.tif"):
        old.unlink()
    # The container mounts tests/data read-only: GDAL writes under target/.
    work = ROOT / "target" / "grid-fixtures"
    shutil.rmtree(work, ignore_errors=True)
    (work / "out").mkdir(parents=True)
    script = work / "recode.py"
    script.write_text(RECODE)
    result = subprocess.run(
        [str(ROOT / "scripts" / "gdal"), "python3", str(script), str(OUT / SOURCE), str(work / "out")],
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode != 0:
        sys.exit(result.stderr)
    names = sorted({line.strip() for line in result.stdout.splitlines() if line.strip().endswith(".tif")})
    for name in names:
        shutil.copyfile(work / "out" / name, OUT / name)
    tiled = "us_noaa_stgeorge.tif"
    result = subprocess.run(
        [str(ROOT / "scripts" / "gdal"), "gdal_translate", "-q", "-co", "TILED=YES", "-co", "BLOCKXSIZE=16",
         "-co", "BLOCKYSIZE=16", "-co", "COMPRESS=DEFLATE", "-co", "PREDICTOR=3",
         str(OUT / tiled), str(work / "out" / f"tiled_{tiled}")],
        capture_output=True,
        text=True,
        check=False,
    )  # fmt: skip
    if result.returncode != 0:
        sys.exit(result.stderr)
    (OUT / "tiled").mkdir(exist_ok=True)
    shutil.copyfile(work / "out" / f"tiled_{tiled}", OUT / "tiled" / tiled)
    return names


def points(path: Path) -> list[tuple[float, float]]:
    """Nodes, cell middles, near-edge and outside points of the source grid."""
    info = json.loads(
        subprocess.run(
            [str(ROOT / "scripts" / "gdal"), "gdalinfo", "-json", str(path)],
            capture_output=True,
            text=True,
            check=False,
        ).stdout
    )
    w, s = info["cornerCoordinates"]["lowerLeft"]
    e, n = info["cornerCoordinates"]["upperRight"]
    pts = []
    for i in range(7):
        for j in range(7):
            pts.append((w + (e - w) * (i + 0.37) / 7, s + (n - s) * (j + 0.61) / 7))
    pts += [(w, s), (e, n), (w - 1e-9, s), (e + 0.5, n), ((w + e) / 2, n + 1e-7), ((w + e) / 2, s - 0.5)]
    return pts


def oracle(files: list[str]) -> list[dict]:
    src_pts = points(OUT / SOURCE)
    records = []
    for name in files:
        path = (OUT / name).resolve()
        pipe = (
            "+proj=pipeline +step +proj=unitconvert +xy_in=deg +xy_out=rad "
            f"+step +proj=hgridshift +grids={path} +step +proj=unitconvert +xy_in=rad +xy_out=deg"
        )
        t = pyproj.Transformer.from_pipeline(pipe)
        for forward in (True, False):
            for x, y in src_pts:
                out = t.transform(x, y, direction="FORWARD" if forward else "INVERSE", errcheck=False)
                ok = all(math.isfinite(v) for v in out)
                records.append({"file": name, "forward": forward, "in": [x, y], "out": list(out) if ok else None})
    return records


def main() -> int:
    pyproj.network.set_network_enabled(False)
    real = download()
    variants = recode()
    records = oracle([SOURCE, *variants])
    with (OUT / "variants.jsonl").open("w") as f:
        f.write(json.dumps({"generator": "scripts/gen_grid_fixtures.py", "proj": pyproj.proj_version_str}) + "\n")
        for r in records:
            f.write(json.dumps(r, separators=(",", ":")) + "\n")
    lines = [
        "# Grid fixtures",
        "",
        "Generated by `scripts/gen_grid_fixtures.py`; do not edit.",
        "",
        f"PROJ-data grids from <{CDN}> (PROJ's GeoTIFF conversions of each producer's",
        "grid), unchanged:",
        "",
        "| File | Producer | Licence | SHA-256 | For |",
        "|---|---|---|---|---|",
    ]
    for r in real:
        lines.append(f"| `{r['file']}` | {r['producer']} | {r['licence']} | `{r['sha256'][:16]}…` | {r['purpose']} |")
    lines += [
        "",
        f"`variant_*.tif`: `{SOURCE}` (NOAA, public domain) re-encoded by GDAL, with",
        "PROJ's `hgridshift` results on each in `variants.jsonl`. `tiled/us_noaa_stgeorge.tif`:",
        "the St. George Island grid re-encoded in 16×16 tiles.",
        "",
        "Attribution for `fr_ign_RGM04versRGM23.tif`: derived from work by IGN France,",
        "converted to GeoTIFF by the PROJ project; Licence Ouverte / Open Licence,",
        "<https://www.etalab.gouv.fr/wp-content/uploads/2014/05/Open_Licence.pdf>.",
        "",
        "Attribution for `fr_ign_RALD2016.tif` and `fr_ign_RALDW842016.tif`: the same, from IGN",
        "France's `RALD2016.gtx` and `RALDW842016.gtx`.",
        "",
        "Attribution for `dk_kds_dvr90_evrf2019.tif`: Agency for Climate Data (Klimadatastyrelsen),",
        "Denmark, CC BY 4.0, <https://creativecommons.org/licenses/by/4.0/>.",
        "",
        "Attribution for `nz_linz_wellht1953-nzvd2016.tif`: derived from work by Land Information",
        "New Zealand, converted to GeoTIFF by the PROJ project; CC BY 4.0,",
        "<https://creativecommons.org/licenses/by/4.0/>.",
    ]
    (OUT / "README.md").write_text("\n".join(lines) + "\n")
    print(f"{len(real)} grids, {len(variants)} variants, {len(records)} PROJ results -> {OUT.relative_to(ROOT)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
