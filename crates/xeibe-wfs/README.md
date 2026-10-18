# xeibe-wfs

A bulk-read client for OGC **WFS** (Web Feature Service) 1.0, 1.1 and 2.0,
part of [xeibe](https://github.com/ttomasz/xeibe).

> **Warning: experimental, not for production use.** This crate is
> AI-generated and has not been thoroughly tested yet. Its interfaces are not
> stable and will change in the near future, in breaking ways. Do not use it in
> production.

## What it does

- Parses capabilities documents and exception reports.
- Chooses a paging strategy the server supports and requests pages until every
  feature of a type has been fetched.
- Streams the pages as `xeibe_core::Sources`, so a whole feature type can be
  read into Arrow by [`xeibe-arrow`](https://crates.io/crates/xeibe-arrow)
  as one dataset.

Without the default `http` feature, only capabilities and response parsing and
request building are available.

## Documentation

See the [WFS design document](https://github.com/ttomasz/xeibe/blob/main/docs/wfs.md).

## License

CC0-1.0 (public domain dedication).
