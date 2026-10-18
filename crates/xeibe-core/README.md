# xeibe-core

Streaming GML reading primitives for [xeibe](https://github.com/ttomasz/xeibe),
shared by its other crates. No Arrow dependency.

> **Warning: experimental, not for production use.** This crate is
> AI-generated and has not been thoroughly tested yet. Its interfaces are not
> stable and will change in the near future, in breaking ways. Do not use it in
> production.

## What it does

- Byte sources: files, readers, zip archive members (Deflate and Deflate64),
  gzip and zstd decompression, decoding to UTF-8 from the document's declared
  encoding.
- Namespaces and qualified names, GML version and dialect detection.
- Splitting a document into one chunk per feature, without building a DOM, so
  that features can be parsed on several threads.

Most users want [`xeibe-arrow`](https://crates.io/crates/xeibe-arrow) instead,
which reads GML into Arrow record batches.

## Documentation

See the [architecture document](https://github.com/ttomasz/xeibe/blob/main/docs/architecture.md)
for how the crates fit together.

## License

CC0-1.0 (public domain dedication).
