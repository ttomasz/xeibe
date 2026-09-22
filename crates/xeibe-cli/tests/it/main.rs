//! Tests for the `xeibe` binary (`docs/architecture.md`, "CLI"): `scan` and its
//! settings file, `convert` to Parquet (native `GEOMETRY` type + GeoParquet 1.1
//! `geo` metadata) and Arrow IPC, curves, and `wfs` against a local scripted
//! server. Nothing here touches a real service.

mod convert;
mod scan;
mod support;
mod wfs;
