#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Fetch the same few features from a WFS once per srsName request form.

    scripts/corpus/fetch_srs_variants.py [--count 3]

Which srsName a server writes back, and in which axis order, depends on the
form it was asked for: GeoServer answers a request for CRS84 or a short
`EPSG:4326` with `http://www.opengis.net/gml/srs/epsg.xml#4326` and
longitude first, and a URN with latitude first (docs/geometry.md, "Observed
in real services"). fetch_wfs_samples.py asks for no srsName, so it can't
show this. Saves
example_data/wfs/<host>/<path>/<type>/getfeature-<version>-srs-<form>.xml,
next to that script's responses, plus request URLs in requests.tsv.
"""
import argparse
import re
import sys
import time
import urllib.parse
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2] / "example_data" / "wfs"

# GeoServers with points at known places, so that the order can be told.
ENDPOINTS = [
    ("https://data.aad.gov.au/geoserver/ows", "aadc:SCAR_CGA_PLACE_NAMES"),  # Antarctica
    ("https://data.wien.gv.at/daten/geo", "ogdwien:SCOOTERABSTELLOGD"),  # Vienna
    ("https://unionepratomagno.ldpgis.it/services/civici_r1/wfs", "unionepratomagno_public:civici_r1"),  # Tuscany
]

# The request forms, by the name used in the file name.
FORMS = {
    "epsg-4326": "EPSG:4326",
    "urn-4326": "urn:ogc:def:crs:EPSG::4326",
    "urnx-4326": "urn:x-ogc:def:crs:EPSG:4326",
    "uri-4326": "http://www.opengis.net/def/crs/EPSG/0/4326",
    "legacy-url-4326": "http://www.opengis.net/gml/srs/epsg.xml#4326",
    "urn-crs84": "urn:ogc:def:crs:OGC:1.3:CRS84",
    "uri-crs84": "http://www.opengis.net/def/crs/OGC/1.3/CRS84",
}


def safe(s: str) -> str:
    return re.sub(r"[^A-Za-z0-9._-]", "_", s)


def main() -> None:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--count", type=int, default=3)
    args = p.parse_args()
    with (ROOT / "requests.tsv").open("a") as log:
        for endpoint, ft in ENDPOINTS:
            u = urllib.parse.urlparse(endpoint)
            base = ROOT / u.netloc / re.sub(r"[^A-Za-z0-9._/-]", "_", u.path.strip("/"))
            for version in ("2.0.0", "1.1.0"):
                for name, srs in FORMS.items():
                    params = {"SERVICE": "WFS", "REQUEST": "GetFeature", "VERSION": version, "SRSNAME": srs}
                    if version == "2.0.0":
                        params.update(TYPENAMES=ft, COUNT=args.count)
                    else:
                        params.update(TYPENAME=ft, MAXFEATURES=args.count)
                    url = endpoint + "?" + urllib.parse.urlencode(params)
                    target = base / safe(ft) / f"getfeature-{version}-srs-{name}.xml"
                    if target.exists():
                        continue
                    target.parent.mkdir(parents=True, exist_ok=True)
                    status = "ok"
                    try:
                        req = urllib.request.Request(url, headers={"User-Agent": "xeibe-corpus"})
                        with urllib.request.urlopen(req, timeout=120) as r:
                            body = r.read(50_000_000)
                        target.write_bytes(body)
                        if b"ExceptionReport" in body[:2000]:
                            status = "exception"
                    except Exception as e:  # noqa: BLE001
                        status = f"error: {str(e)[:120]}"
                    log.write(f"{time.strftime('%Y-%m-%dT%H:%M:%S')}\t{status}\t{target.relative_to(ROOT)}\t{url}\n")
                    log.flush()
                    print(f"{status:10} {version} {name:16} {ft} ({u.netloc})", file=sys.stderr, flush=True)
                    time.sleep(0.5)


if __name__ == "__main__":
    main()
