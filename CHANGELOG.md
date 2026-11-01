# Changelog

The workspace crates, the `xeibe` CLI and the `xeibe` Python package, released
together under one version by a `v*` tag ([docs/releasing.md](docs/releasing.md)).
`xeibe-crs` has its own changelog of EPSG releases
([crates/xeibe-crs/CHANGELOG.md](crates/xeibe-crs/CHANGELOG.md)), and
`xeibe-proj` its own ([crates/xeibe-proj-py/CHANGELOG.md](crates/xeibe-proj-py/CHANGELOG.md)).

## Unreleased

- First release.
- `xeibe-crs` 0.4.1: the embedded EPSG data is LZ4-compressed (pure-Rust
  `lz4_flex`) instead of zstd. The first lookup of each blob is up to a third
  faster (`operations_json()` 1.2 ms rather than 1.9 ms), and `xeibe-crs`
  no longer needs zstd's C code, so the `xeibe-proj` wheels and the reproject
  page build without it. The data is 1.5 MB rather than 1.0 MB; artifacts
  that link zstd anyway (the CLI, the `xeibe` wheels, the GML page) grow by
  about 540 KB. Every lookup returns the same bytes as before.
