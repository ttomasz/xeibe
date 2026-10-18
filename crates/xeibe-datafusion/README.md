# xeibe-datafusion

Query GML with SQL in [Apache DataFusion](https://datafusion.apache.org/)
(and SedonaDB), part of [xeibe](https://github.com/ttomasz/xeibe).

> **Warning: experimental, not for production use.** This crate is
> AI-generated and has not been thoroughly tested yet. Its interfaces are not
> stable and will change in the near future, in breaking ways. Do not use it in
> production.

## What it does

- `GmlTable`: a `TableProvider` for one GML layer, with a given schema or one
  sampled when the table is created. Each source is one partition.
- `read_gml()`: a table function, registered with `xeibe_datafusion::register`.

```sql
SELECT count(*) FROM read_gml('data/*.gml', 'Parcel');
```

Remote inputs go through DataFusion's own `object_store` registry.

## Documentation

See the [architecture document](https://github.com/ttomasz/xeibe/blob/main/docs/architecture.md).

## License

CC0-1.0 (public domain dedication).
