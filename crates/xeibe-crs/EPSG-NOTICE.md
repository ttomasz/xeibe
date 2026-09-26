# EPSG Dataset notice

`xeibe-crs` contains data extracted from the **EPSG Geodetic Parameter
Dataset** v13.103 (2026-09-11), published by the International Association of
Oil and Gas Producers (IOGP) at <https://epsg.org/>.

**Ownership of the EPSG Dataset by IOGP is hereby acknowledged.**

The EPSG Dataset is used here under the EPSG Dataset Terms of Use,
<https://epsg.org/terms-of-use.html>. Those terms permit extracting subsets of
the data and redistributing them, and they require that:

- IOGP's ownership is acknowledged in any onward publication or transmission,
  including of permitted modifications;
- anyone to whom this data is passed on is informed of the Terms of Use --
  which is what this file is for;
- the data is not distributed for profit, and any commercial packaging derives
  its value from what the provider adds rather than from the Dataset itself;
- data modified other than as the Terms permit is not attributed to the EPSG
  Dataset.

## What was extracted, and how

`scripts/gen_crs_tables.py` reads the official EPSG "PostgreSQL scripts"
release and emits:

- `src/table.rs` -- for each CRS: code, name, kind, first axis direction,
  dimension, axis units and EPSG's published area of use.
- `data/projjson.bin` -- a PROJJSON encoding of each CRS definition.
- `src/aliases.rs` -- EPSG's aliases for CRSs (table `epsg_alias`), lowercased,
  keeping only those that name a single CRS.

Values are reproduced as EPSG publishes them, with two representation changes
that the Terms permit because they preserve numeric equivalence:

1. Angles stored in EPSG's sexagesimal packings (units of measure 9110
   `DDD.MMSSsss` and 9111 `DDD.MMm`) are converted to decimal degrees, and
   relabelled as degrees. For example EPSG:2009 stores a longitude of natural
   origin of `-58.3`, meaning -58°30', which is emitted as `-58.5` degrees.
2. Unit of measure 9122, "degree (supplier to define representation)", is
   emitted as unit of measure 9102, "degree".

No coordinate operation parameters, ellipsoid parameters or areas of use are
otherwise recalculated, re-fitted or reprojected. In particular the area of use
is EPSG's own WGS 84 bounding box, not a reprojection of it.

## Licensing of this crate

The xeibe source code in this crate is under the workspace licence. The EPSG
data it embeds is **not** placed under that licence and is not in the public
domain; it remains IOGP's, under the Terms of Use linked above. The crate's
`license` field records this as a combined expression.
