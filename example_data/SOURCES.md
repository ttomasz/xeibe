# example_data sources

Raw downloads (git-ignored except this file and `wfs/endpoints-*.txt`; mounted
read-only in `scripts/gdal`). Record every
source here: where it came from, when, and under which licence. Only data with a
clear open licence may be excerpted into `tests/data/` (see tests/data/README.md).

| Path | Source | Retrieved | Licence | Notes |
|---|---|---|---|---|
| `PRG-punkty_adresowe_*/` | GUGiK PRG (Państwowy Rejestr Granic), address points | before 2026-09 (by user) | official material, not copyrighted (art. 4 pkt 2 copyright act); dane.gov.pl label CC BY 4.0 | GML 3.2, 0.6–3.3 GB |
| `A00_*.gml`, `A01_*.gml` | GUGiK PRG boundaries | before 2026-09 (by user) | official material, not copyrighted (art. 4 pkt 2 copyright act) | `gml:Surface` |
| `0808_GML/` | GUGiK BDOT10k package | before 2026-09 (by user) | official material, not copyrighted (art. 4 pkt 2 copyright act) | with XSDs |
| `gdal-autotest-src/` | github.com/OSGeo/gdal @ ec7b4b055f80 (2026-09-18), sparse: `autotest/ogr/data/{gml,nas,wfs,gmlas}` + `autotest/ogr/*.py` (`scripts/corpus/fetch_gdal_autotest.sh`) | 2026-09-19 | Scripts: MIT (file headers). Data files: mixed/unclear provenance | Do not vendor data files; MIT snippets may be copied with notice |
| `_inventory/data_europa_eu.jsonl` | data.europa.eu search API, formats GML + WFS_SRVC (`scripts/corpus/inventory_eu.py`) | 2026-09-19 | metadata only; per-record `license` field | 23,747 distributions, 235 WFS hosts |
| `_inventory/dane_gov_pl.jsonl` | api.dane.gov.pl, formats gml/wfs + zip/xml mentioning gml (`scripts/corpus/inventory_pl.py`) | 2026-09-19 | metadata only | 431 resources |
| `wfs/mapy.geoportal.gov.pl/…` | GUGiK geoportal WFS (40 endpoints from geoportal.gov.pl INSPIRE + WFS pages): capabilities 2.0.0/1.1.0/1.0.0 and GetFeature with 3 features per type/version (`scripts/corpus/fetch_capabilities.py`, `fetch_wfs_samples.py`) | 2026-09-19 | GUGiK services – open use (verify per service) | 217 OK responses, 9.3 MB; `requests.tsv` has every URL |
| `wfs/<host>/…` (non-geoportal) | One WFS endpoint per host from the data.europa.eu inventory (`wfs/endpoints-eu.txt`, 263 endpoints, 212 answered): capabilities 2.0.0/1.1.0/1.0.0 + GetFeature with 3 features × 2 types × 3 versions | 2026-09-19 | per service (see inventory `license`) | ~900 feature responses, 412 MB; axis survey in `wfs/axis_survey.txt` |
| `downloads/<portal>/<host>/…` | Direct GML/ZIP/XML file downloads selected by `scripts/corpus/downloads.py` (≤4 per host and kind, open licences preferred, ≤1.5 GB per file, 18 GB cap) | 2026-09-19 | per file in `downloads/log.jsonl` (`licence`) | selection in `_inventory/selection.jsonl` |
| `reference/prg/00_jednostki_administracyjne.zip` | GUGiK PRG administrative units (SHP), <https://opendata.geoportal.gov.pl/prg/granice/00_jednostki_administracyjne.zip>, dataset <https://dane.gov.pl/pl/dataset/726> | 2026-09-19 | official material, not copyrighted (dane.gov.pl label: CC BY 4.0) | 378 MB; axis-order evidence (`scripts/corpus/reference_data.py`) |
| `reference/naturalearth/ne_10m_admin_0_countries.zip` | Natural Earth 1:10m Admin 0 Countries, <https://naciscdn.org/naturalearth/10m/cultural/ne_10m_admin_0_countries.zip> | 2026-09-19 | public domain | 4.9 MB; axis-order evidence |
| `reference/axis_reference.gpkg` | built from the two above: `pl_wojewodztwa`, `pl_powiaty`, `pl_gminy`, `countries` (`code`, `name`) | 2026-09-19 | as sources | `reference/sources.json` has URLs and retrieval times |
