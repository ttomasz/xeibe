#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Download the boundary datasets used as independent evidence for axis order and build
example_data/reference/axis_reference.gpkg (layers pl_wojewodztwa, pl_powiaty, pl_gminy,
countries; each with `code` and `name`).

    scripts/corpus/reference_data.py [--force]

Sources (recorded with retrieval time in example_data/reference/sources.json):
  * PRG – jednostki administracyjne (GUGiK), official material (not copyrighted; dane.gov.pl: CC BY 4.0),
    https://dane.gov.pl/pl/dataset/726 (resource 2355394): TERYT codes in JPT_KOD_JE
  * Natural Earth 1:10m Admin 0 – Countries, public domain, https://www.naturalearthdata.com/
    (ADM0_A3 codes)

Used by scripts/corpus/axis_evidence.py. Nothing derived from these is committed.
"""
import argparse
import datetime
import json
import subprocess
import sys
import urllib.request
from pathlib import Path

PROJECT = Path(__file__).resolve().parents[2]
REF = PROJECT / "example_data" / "reference"
GPKG = REF / "axis_reference.gpkg"
GDAL = str(PROJECT / "scripts" / "gdal")

SOURCES = {
    "prg": {
        "url": "https://opendata.geoportal.gov.pl/prg/granice/00_jednostki_administracyjne.zip",
        "file": "prg/00_jednostki_administracyjne.zip",
        "dataset_url": "https://dane.gov.pl/pl/dataset/726",
        "publisher": "Główny Urząd Geodezji i Kartografii (GUGiK)",
        "licence": "official material, not subject to copyright (art. 4 pkt 2 ustawy o prawie autorskim); "
                   "dane.gov.pl labels it CC BY 4.0 and asks for the attribution 'Wykorzystano/opracowano na "
                   "podstawie materiałów państwowego zasobu geodezyjnego i kartograficznego'",
    },
    "naturalearth": {
        "url": "https://naciscdn.org/naturalearth/10m/cultural/ne_10m_admin_0_countries.zip",
        "file": "naturalearth/ne_10m_admin_0_countries.zip",
        "dataset_url": "https://www.naturalearthdata.com/downloads/10m-cultural-vectors/10m-admin-0-countries/",
        "publisher": "Natural Earth",
        "licence": "public domain (https://www.naturalearthdata.com/about/terms-of-use/)",
    },
}

LAYERS = [  # (output layer, source dataset, shapefile, code field, name field)
    ("pl_wojewodztwa", "prg", "A01_Granice_wojewodztw", "JPT_KOD_JE", "JPT_NAZWA_"),
    ("pl_powiaty", "prg", "A02_Granice_powiatow", "JPT_KOD_JE", "JPT_NAZWA_"),
    ("pl_gminy", "prg", "A03_Granice_gmin", "JPT_KOD_JE", "JPT_NAZWA_"),
    ("countries", "naturalearth", "ne_10m_admin_0_countries", "ADM0_A3", "NAME"),
]


def download(force: bool) -> dict:
    log_path = REF / "sources.json"
    log = json.loads(log_path.read_text()) if log_path.exists() else {}
    for key, src in SOURCES.items():
        target = REF / src["file"]
        if target.exists() and not force and key in log:
            continue
        target.parent.mkdir(parents=True, exist_ok=True)
        print(f"downloading {src['url']}", file=sys.stderr)
        req = urllib.request.Request(src["url"], headers={"User-Agent": "xeibe-parser-test-corpus"})
        with urllib.request.urlopen(req) as r, target.open("wb") as f:
            while chunk := r.read(1 << 20):
                f.write(chunk)
        log[key] = {**src, "retrieved": datetime.datetime.now(datetime.UTC).isoformat(timespec="seconds"),
                    "bytes": target.stat().st_size}
    log_path.write_text(json.dumps(log, indent=1, ensure_ascii=False) + "\n")
    return log


def build() -> None:
    # example_data/ is read-only inside the GDAL container: build under target/, then move.
    tmp = PROJECT / "target" / "reference" / GPKG.name
    tmp.parent.mkdir(parents=True, exist_ok=True)
    tmp.unlink(missing_ok=True)
    for i, (layer, key, shp, code, name) in enumerate(LAYERS):
        src = f"/vsizip/{(REF / SOURCES[key]['file']).relative_to(PROJECT)}/{shp}.shp"
        cmd = [GDAL, "ogr2ogr", *([] if i == 0 else ["-append"]), "-f", "GPKG", str(tmp.relative_to(PROJECT)), src,
               "-nln", layer, "-nlt", "MULTIPOLYGON", "-sql", f"SELECT {code} AS code, {name} AS name FROM {shp}"]
        subprocess.run(cmd, check=True, cwd=PROJECT)
    tmp.replace(GPKG)
    print(f"-> {GPKG.relative_to(PROJECT)}", file=sys.stderr)


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--force", action="store_true", help="download again and rebuild")
    args = ap.parse_args()
    REF.mkdir(parents=True, exist_ok=True)
    download(args.force)
    if args.force or not GPKG.exists() or any(
            (REF / s["file"]).stat().st_mtime > GPKG.stat().st_mtime for s in SOURCES.values()):
        build()


if __name__ == "__main__":
    main()
