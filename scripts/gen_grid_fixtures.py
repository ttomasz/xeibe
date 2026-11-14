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
* `cropped/`: grids too large to commit whole, cut down by GDAL to a window
  of a few nodes and kept under PROJ-data's names, so that PROJ and xeibe
  read them in place of the full files: Canada's and the Nordic velocity
  grids (point motion operations) and New Zealand's deformation model, its
  master file reduced to the components that reach the window. Their
  vectors: `scripts/gen_transformation_vectors.py --epochs --grids
  tests/data/reprojection/grids/cropped`. The full files come from
  `target/proj-grids/` (`scripts/fetch_proj_grids.py`) or the CDN.

    scripts/gen_grid_fixtures.py
"""

from __future__ import annotations

import hashlib
import json
import math
import os
import random
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

# Grids cut down to a window: file, sha256 of the full file, window (west,
# south, east, north, in degrees), producer, licence, what it is for.
CROPPED = [
    (
        "ca_nrc_NAD83v70VG.tif",
        "bfa4f958bd8ff287736fd2595bb524d874de88e6fa0670846abdb8394ab7587b",
        (-100.6, 49.6, -99.4, 50.4),
        "Natural Resources Canada",
        "Open Government Licence - Canada",
        "Point motion by velocity grid: NAD83(CSRS)v7 velocity grid, EPSG:9483",
    ),
    (
        "ca_nrc_NAD83v6VG.tif",
        "5f2f41ac873b60187da9ae779bf7106e1b7c4b65a9ddef730744c0e2b04e26c9",
        (-100.6, 49.6, -99.4, 50.4),
        "Natural Resources Canada",
        "Open Government Licence - Canada",
        "Point motion by velocity grid: NAD83(CSRS)v6 velocity grid, EPSG:8676",
    ),
    (
        "eur_nkg_nkgrf17vel.tif",
        "515f68aeb59f659a3f3c340626cd060f58de6f93b4546df67732b906949e0d66",
        (14.6, 59.8, 15.4, 60.2),
        "Nordic Geodetic Commission",
        "CC BY 4.0",
        "Point motion by velocity grid (NKG): NKG_RF17vel, EPSG:10808",
    ),
]
# New Zealand's deformation model: the master file, and the window its
# component grids are cut down to. North of Kaikoura: it crosses the edge of
# one patch and lies in the finest of the 2016 earthquake's nested grids.
MODEL = (
    "nz_linz_nzgd2000-20180701.json",
    "212e3fc3d8f3a2351c18b5663e0d20d5fbed8d9f3e78ce6fa02c871dd78af0ff",
    (172.98, -42.66, 173.04, -42.61),
    "Land Information New Zealand",
    "CC BY 4.0",
    "New Zealand Deformation Model: ITRF96 to NZGD2000 (7), EPSG:9090, and the grids it names",
)

# Run inside the GDAL container: cut every grid of each GeoTIFF (each IFD, a
# file's nested grids) down to the nodes around a window, keeping its
# metadata. A grid the window doesn't reach is left out, and so is a file
# with none left. A grid with children keeps one more node each way: PROJ
# makes a grid the child of the one its metadata names only if that one's
# extent contains it, and two edges cut at the same node differ in their
# last bit.
CROP = r"""
import json, math, os, sys
from osgeo import gdal
gdal.UseExceptions()
spec = json.load(open(sys.argv[1]))
out_dir = sys.argv[2]
report = {}

def extent(ds):
    # Nodes sit at pixel centres: west, south, east, north.
    gt = ds.GetGeoTransform()
    x0, y0 = gt[0] + gt[1] / 2, gt[3] + gt[5] / 2
    return x0, y0 + (ds.RasterYSize - 1) * gt[5], x0 + (ds.RasterXSize - 1) * gt[1], y0

for name, job in spec.items():
    w, s, e, n = job["window"]
    top = gdal.Open(job["path"])
    subs = [sd[0] for sd in top.GetSubDatasets()] or [job["path"]]
    parents = {gdal.Open(sub).GetMetadataItem("parent_grid_name") for sub in subs}
    out = f"{out_dir}/{name}"
    kept = []
    for sub in subs:
        ds = gdal.Open(sub)
        gt = ds.GetGeoTransform()
        W, H = ds.RasterXSize, ds.RasterYSize
        x0, y1, x1, y0 = extent(ds)
        rx, ry = gt[1], -gt[5]
        if e < x0 or w > x1 or n < y1 or s > y0:
            continue
        margin = 1 if ds.GetMetadataItem("grid_name") in parents else 0
        i0 = max(0, math.floor((w - x0) / rx + 1e-9) - margin)
        i1 = min(W - 1, math.ceil((e - x0) / rx - 1e-9) + margin)
        j0 = max(0, math.floor((y0 - n) / ry + 1e-9) - margin)
        j1 = min(H - 1, math.ceil((y0 - s) / ry - 1e-9) + margin)
        # Two nodes at least each way.
        if i1 == i0:
            i0, i1 = (i0 - 1, i1) if i0 > 0 else (i0, i1 + 1)
        if j1 == j0:
            j0, j1 = (j0 - 1, j1) if j0 > 0 else (j0, j1 + 1)
        nx, ny = i1 - i0 + 1, j1 - j0 + 1
        opts = ["COMPRESS=DEFLATE", "PREDICTOR=3", "INTERLEAVE=BAND"] + (["APPEND_SUBDATASET=YES"] if kept else [])
        dst = gdal.GetDriverByName("GTiff").Create(out, nx, ny, ds.RasterCount, ds.GetRasterBand(1).DataType, opts)
        dst.SetGeoTransform((gt[0] + i0 * gt[1], gt[1], 0, gt[3] + j0 * gt[5], 0, gt[5]))
        dst.SetProjection(ds.GetProjection())
        dst.SetMetadata(ds.GetMetadata())
        for b in range(ds.RasterCount):
            sb, db = ds.GetRasterBand(b + 1), dst.GetRasterBand(b + 1)
            db.SetDescription(sb.GetDescription())
            db.SetUnitType(sb.GetUnitType())
            db.SetMetadata(sb.GetMetadata())
            if sb.GetNoDataValue() is not None:
                db.SetNoDataValue(sb.GetNoDataValue())
            db.WriteArray(sb.ReadAsArray(i0, j0, nx, ny))
        dst = None
        kept.append({"nodes": [nx, ny], "extent": [x0 + i0 * rx, y0 - j1 * ry, x0 + i1 * rx, y0 - j0 * ry]})
    report[name] = kept
    if not kept:
        continue
    # As written: every child inside the grid it names, as in the whole file.
    cut = gdal.Open(out)
    cuts = [gdal.Open(sd[0]) for sd in cut.GetSubDatasets()] or [cut]
    by_name = {ds.GetMetadataItem("grid_name"): extent(ds) for ds in cuts}
    for ds in cuts:
        parent = by_name.get(ds.GetMetadataItem("parent_grid_name"))
        if parent is not None:
            c = extent(ds)
            if not (c[0] >= parent[0] and c[1] >= parent[1] and c[2] <= parent[2] and c[3] <= parent[3]):
                sys.exit(f"{name}: {ds.GetMetadataItem('grid_name')} is not inside its parent: {c}, {parent}")
json.dump(report, open(f"{out_dir}/report.json", "w"))
"""


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


def full_file(name: str, digest: str, algorithm: str = "sha256") -> Path:
    """A whole PROJ-data file: `target/proj-grids/`'s, or the CDN's (kept in
    `target/grid-fixture-sources/`), with the checksum it must have."""
    cache = ROOT / "target" / "grid-fixture-sources" / name
    for path in (ROOT / "target" / "proj-grids" / name, cache):
        if path.exists() and hashlib.new(algorithm, path.read_bytes()).hexdigest() == digest:
            return path
    print(f"downloading {CDN}{name}")
    cache.parent.mkdir(parents=True, exist_ok=True)
    with urllib.request.urlopen(CDN + name) as response:
        cache.write_bytes(response.read())
    got = hashlib.new(algorithm, cache.read_bytes()).hexdigest()
    if got != digest:
        sys.exit(f"{name}: {algorithm} {got}, expected {digest} (the CDN file changed?)")
    return cache


def crop() -> list[dict]:
    """`cropped/`: the velocity grids and New Zealand's deformation model,
    cut down to their windows. Returns the README's rows."""
    out = OUT / "cropped"
    shutil.rmtree(out, ignore_errors=True)
    out.mkdir(parents=True)
    work = ROOT / "target" / "grid-fixtures" / "cropped"
    work.mkdir(parents=True)
    spec = {
        name: {"path": str(full_file(name, digest)), "window": window} for name, digest, window, *_ in CROPPED
    }
    model_name, model_digest, window, producer, licence, purpose = MODEL
    model = json.loads(full_file(model_name, model_digest).read_text())
    for component in model["components"]:
        spatial = component["spatial_model"]
        spec[spatial["filename"]] = {
            "path": str(full_file(spatial["filename"], spatial["md5_checksum"], "md5")),
            "window": window,
        }
    (work / "spec.json").write_text(json.dumps(spec))
    (work / "crop.py").write_text(CROP)
    result = subprocess.run(
        [str(ROOT / "scripts" / "gdal"), "python3", str(work / "crop.py"), str(work / "spec.json"), str(work)],
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode != 0:
        sys.exit(result.stderr)
    report = json.loads((work / "report.json").read_text())
    for name, grids in report.items():
        if grids:
            shutil.copyfile(work / name, out / name)
    # The master file with the components that are left, for the window only.
    components = []
    for component in model["components"]:
        spatial = component["spatial_model"]
        if report[spatial["filename"]]:
            spatial["md5_checksum"] = hashlib.md5((out / spatial["filename"]).read_bytes()).hexdigest()
            components.append(component)
    dropped = len(model["components"]) - len(components)
    model["components"] = components
    model["extent"] = {"type": "bbox", "parameters": {"bbox": list(window)}}
    model["description"] += (
        f"\nReduced for xeibe's tests by scripts/gen_grid_fixtures.py: valid only within {list(window)}, "
        f"with the grids cut down to it and without the {dropped} components that don't reach it.\n"
    )
    (out / model_name).write_text(json.dumps(model, indent=1) + "\n")
    check_cropped(out, model_name, model["time_extent"])
    # Where the vectors' points go: inside every window, clear of its edges.
    windows = {name: list(window) for name, _, window, *_ in CROPPED} | {model_name: list(window)}
    (out / "windows.json").write_text(json.dumps(windows, indent=1) + "\n")
    rows = [
        {"file": name, "window": window, "producer": producer, "licence": licence, "purpose": purpose,
         "bytes": (out / name).stat().st_size}
        for name, _, window, producer, licence, purpose in CROPPED
    ]  # fmt: skip
    grids = [name for name in report if name not in windows and report[name]]
    rows.append(
        {"file": f"{model_name}` and {len(grids)} `nz_linz_nzgd2000-*.tif", "window": window, "producer": producer,
         "licence": licence, "purpose": purpose,
         "bytes": sum((out / name).stat().st_size for name in [model_name, *grids])}
    )  # fmt: skip
    return rows


def check_cropped(out: Path, model_name: str, time_extent: dict) -> None:
    """PROJ gives on the cropped files what it gives on the whole ones, to
    the last bits, everywhere in their windows."""
    whole = [str(ROOT / "target" / "proj-grids"), str(ROOT / "target" / "grid-fixture-sources")]
    degrees = "+step +proj=unitconvert +xy_in=deg +xy_out=rad"
    back = "+step +proj=unitconvert +xy_in=rad +xy_out=deg"
    first, last = (int(time_extent[k][:4]) for k in ("first", "last"))
    rng = random.Random(1)
    worst = 0.0
    for name, window in [(n, w) for n, _, w, *_ in CROPPED] + [(model_name, MODEL[2])]:
        if name == model_name:
            step = f"+step +proj=defmodel +model={name}"
        else:
            step = (
                f"+step +proj=cart +ellps=GRS80 +step +proj=deformation +dt=10 +grids={name} +ellps=GRS80 "
                "+step +inv +proj=cart +ellps=GRS80"
            )
        results = []
        w, s, e, n = window
        points = [
            (rng.uniform(w, e), rng.uniform(s, n), rng.uniform(0, 500), rng.uniform(first + 1, last - 1))
            for _ in range(400)
        ]
        for dirs in (whole, [str(out)]):
            pyproj.datadir.set_data_dir(os.pathsep.join([pyproj.datadir.get_data_dir().split(os.pathsep)[0], *dirs]))
            t = pyproj.Transformer.from_pipeline(f"+proj=pipeline {degrees} {step} {back}")
            results.append([t.transform(*p, errcheck=False) for p in points])
        for p, a, b in zip(points, *results):
            if not all(math.isfinite(v) for v in a + b):
                sys.exit(f"{name}: PROJ has no result at {p}: {a}, {b}")
            worst = max(worst, abs(a[0] - b[0]) * 1e5, abs(a[1] - b[1]) * 1e5, abs(a[2] - b[2]))
            if worst > 1e-8:
                sys.exit(f"{name}: at {p} PROJ gives {b} on the cropped files, {a} on the whole ones")
    print(f"cropped grids: PROJ's results on them differ from the whole files' by {worst:.1e} m at most")


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
    cropped = crop()
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
        "",
        "## `cropped/`",
        "",
        "PROJ-data grids too large to commit whole, cut down by GDAL to the nodes around a",
        "window (west, south, east, north) and kept under their own names: a directory to",
        "give PROJ and xeibe in place of the full files, valid only within the windows",
        "(`windows.json`). Every grid in a file (its nested grids) is cut down, with its",
        "metadata, and a grid with children keeps one node more each way, so that PROJ still",
        "nests them. The values are the whole file's: the script checks that PROJ gives the",
        "same results on both, everywhere in the windows.",
        "",
        "| File | Window | Producer | Licence | Size | For |",
        "|---|---|---|---|---|---|",
    ]
    for r in cropped:
        window = ", ".join(f"{v:g}" for v in r["window"])
        lines.append(
            f"| `{r['file']}` | {window} | {r['producer']} | {r['licence']} | {r['bytes'] / 1024:.0f} KiB | {r['purpose']} |"
        )
    lines += [
        "",
        "Attribution for `ca_nrc_NAD83v70VG.tif` and `ca_nrc_NAD83v6VG.tif`: derived from work by",
        "Natural Resources Canada (`NAD83v70VG.gvb`, `NAD83v6VG.gvb`), converted to GeoTIFF by the",
        "PROJ project and cut down here; Open Government Licence - Canada,",
        "<https://open.canada.ca/en/open-government-licence-canada>.",
        "",
        "Attribution for `eur_nkg_nkgrf17vel.tif`: the Nordic Geodetic Commission's NKG_RF17vel",
        "model, cut down here; CC BY 4.0, <https://creativecommons.org/licenses/by/4.0/>.",
        "",
        "Attribution for `nz_linz_nzgd2000-*`: Land Information New Zealand's NZGD2000 deformation",
        "model (version 20180701), converted by the PROJ project; the grids are cut down here and",
        "the master file lists only the components that reach the window, with the window as its",
        "extent and the cut grids' checksums. CC BY 4.0,",
        "<https://creativecommons.org/licenses/by/4.0/>.",
    ]
    (OUT / "README.md").write_text("\n".join(lines) + "\n")
    size = sum(r["bytes"] for r in cropped) / 1024
    print(
        f"{len(real)} grids, {len(variants)} variants, {len(records)} PROJ results, "
        f"{len(cropped)} cropped sets ({size:.0f} KiB) -> {OUT.relative_to(ROOT)}"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
