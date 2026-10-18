# xeibe-io

Remote inputs for [xeibe](https://github.com/ttomasz/xeibe): GML over HTTP(S)
and from object stores, each read as one sequential stream.

> **Warning: experimental, not for production use.** This crate is
> AI-generated and has not been thoroughly tested yet. Its interfaces are not
> stable and will change in the near future, in breaking ways. Do not use it in
> production.

## What it does

- `http` feature (default): `http://` and `https://` inputs, with
  authentication options.
- `object-store` feature: `s3://`, `gs://` and `az://` inputs through
  [`object_store`](https://crates.io/crates/object_store), globs included.
- `resolve_sources()` turns command-line style inputs (paths, directories,
  globs, zip archives, `-`, URLs) into a list of `xeibe_core::Source`s.

Nothing is downloaded to disk or cached.

## Documentation

See [sources and remote input](https://github.com/ttomasz/xeibe/blob/main/docs/architecture.md#sources-and-remote-input)
in the architecture document.

## License

CC0-1.0 (public domain dedication).
