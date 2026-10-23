# Esri data notice

`xeibe-esri` contains data derived from Esri's **projection engine database
documentation**, https://github.com/Esri/projection-engine-db-doc, at commit `06dc50b556a2fa49224453ce8cbcf1833b3b64f7`
(ArcGIS Pro 3.7, with EPSG's entries from EPSG 12.049). It is Copyright (c)
2010-2018 Environmental Systems Research Institute, Inc., and licensed
under the Apache License, Version 2.0, a copy of which is in
`LICENSE-APACHE`. The repository has no `NOTICE` file.

## What was taken, and how it was changed

`scripts/gen_esri_tables.py` reads `csv/pe_list_geogcs.csv`,
`pe_list_projcs.csv`, `pe_list_vertcs.csv`, `pe_list_datum.csv` and
`pe_list_vdatum.csv` and writes `src/tables.rs`:

- Esri's names of the CRSs and datums that come from the EPSG Dataset (the
  rows whose authority is EPSG, and Esri's rows whose code changed to an
  EPSG code), lowercased, each with its EPSG code;
- the Esri codes that changed to EPSG codes.

Everything else in the CSVs (definitions, areas of use, objects only Esri
defines) is left out.

The rest of this crate, its code, is CC0-1.0 like the rest of the xeibe
workspace. EPSG codes are identifiers of the EPSG Dataset, which IOGP owns;
`xeibe-crs` carries its notice.
