# xeibe-schema

Arrow schema inference for GML, part of [xeibe](https://github.com/ttomasz/xeibe).

> **Warning: experimental, not for production use.** This crate is
> AI-generated and has not been thoroughly tested yet. Its interfaces are not
> stable and will change in the near future, in breaking ways. Do not use it in
> production.

## What it does

- Scans features into a tree of element paths with value statistics: which
  values parse as integers, decimals, dates or timestamps, how often each
  element repeats, where geometry occurs.
- Turns that observation into a flat Arrow schema, one column per leaf path,
  following explicit, configurable rules (`InferenceOptions`). It can explain
  each decision.
- Binds a schema the caller gives to the element paths it reads from.

Most users want [`xeibe-arrow`](https://crates.io/crates/xeibe-arrow) instead,
which uses this crate for its `scan()` and `read()`.

## Documentation

See the design documents on
[schema inference](https://github.com/ttomasz/xeibe/blob/main/docs/schema-inference.md)
and [type mapping](https://github.com/ttomasz/xeibe/blob/main/docs/type-mapping.md).

## License

CC0-1.0 (public domain dedication).
