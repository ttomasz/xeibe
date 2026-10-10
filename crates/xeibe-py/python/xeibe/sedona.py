"""GML as a SedonaDB data source.

    import sedonadb
    from xeibe.sedona import GmlFormat, read_gml

    sd = sedonadb.connect()

    # One call: picks the layer, infers one schema across all files.
    df = read_gml(sd, "prg/*.gml", "AD_PunktAdresowy")

    # The format itself, for `sd.read()`; registered, for `.gml` paths.
    df = sd.read("bdot/", format=GmlFormat(), options={"layer": "OT_BUBD_A"})
    sd.register(GmlFormat(extension="gml"))
    df = sd.read("prg/*.gml", options={"layer": "AD_PunktAdresowy"})

A table is one layer. Options (`options=` of `sd.read()`, or `with_options()`):

- `layer`: the layer's name. It may be left out only where a sampled scan
  reads the whole input and finds exactly one layer.
- `schema`: the layer's schema: an Arrow schema (e.g. `Scan.schema()`),
  a `xeibe.Scan`, or its columns as a dict or JSON text
  (`{"lokalnyId": "text", "geom": {"type": "geometry", "path": "pozycja"}}`).
- `settings`: a settings file (path, dict or JSON text): its read options,
  and the layer's schema if it has one.
- `preset`, `axis_order`, `crs`, `sample_features`, `batch_size`, `threads`:
  read options, as `key=value` pairs (`{"preset": "strings"}`).

Without a schema, each file's schema is sampled when the table is created,
and SedonaDB merges them; files whose types differ make that merge fail.
`read_gml()` avoids that: it infers one schema across all the files first.

Every file of a directory or glob is read. Files without features (XML
schemas, ISO metadata, anything that isn't GML) are skipped, as `xeibe.read()`
skips them, and each one skipped is a `xeibe.SkippedSourceWarning` when it is
read; `warnings.simplefilter("error", xeibe.SkippedSourceWarning)` makes that
an error instead. If no file holds the layer, the query fails. `extension`
(e.g. `"gml"`) makes SedonaDB list only files with it, in directories and globs
alike, and `sd.register()` needs it.

Geometry columns are always `geoarrow.wkb`, the one geometry type SedonaDB
reads. Paths and URLs are opened by xeibe, so object stores use xeibe's
credentials (from the environment), not stores registered in SedonaDB.
"""

from __future__ import annotations

import copy
import os
from collections.abc import Iterable, Mapping
from typing import Any

from arro3.core import DataType, RecordBatchReader, Schema
from sedonadb.datasource import ExternalFormatSpec

import xeibe
from xeibe._xeibe import layer_schema, read_options, read_or_empty, to_wkb

__all__ = ["GmlFormat", "read_gml"]

# Schema metadata: the layer chosen for a file when no `layer` was given.
# Files that chose different layers can't be merged into one table.
_LAYER_KEY = b"gml:layer"

# The metadata a read needs (`docs/type-mapping.md`). The rest records what a
# sample saw (GML versions, srsNames, axis decisions, …); it differs between
# files, and SedonaDB won't merge schemas whose metadata differs.
_FIELD_KEYS = {b"gml:path", b"gml:content", b"ARROW:extension:name", b"ARROW:extension:metadata"}
_SCHEMA_KEYS = {b"gml:ns"}


class GmlFormat(ExternalFormatSpec):
    """GML files as a SedonaDB data source, one layer per table.

    `extension` (default: none) restricts the files SedonaDB lists to those
    with it, and is the one `sd.register()` registers the format under, for
    `sd.read()` to guess the format from. Registered without one, the format
    can't be found that way.
    """

    def __init__(self, *, extension: str = "") -> None:
        self._extension = extension
        self._layer: str | None = None
        self._schema: Any = None
        self._settings: Any = None
        self._pairs: dict[str, str] = {}
        self._options: dict = read_options()

    @property
    def extension(self) -> str:
        return self._extension

    @property
    def supports_concurrent_file_reads(self) -> bool:
        return True

    def with_options(self, options: Mapping[str, Any]) -> GmlFormat:
        clone = copy.copy(self)
        clone._pairs = dict(self._pairs)
        for key, value in options.items():
            if key == "layer":
                clone._layer = None if value is None else str(value)
            elif key == "schema":
                clone._schema = value
            elif key == "settings":
                clone._settings = value
            else:
                clone._pairs[key] = str(value)
        # Checks the options now, not when the first file is read.
        clone._options = read_options(clone._settings, clone._pairs)
        if clone._layer is not None:
            clone._given_schema(clone._layer)
        return clone

    def infer_schema(self, src: Any) -> Schema:
        url = _url(src)
        try:
            layer = self._layer or self._only_layer([url])
            schema = self._given_schema(layer)
            if schema is None:
                sample = xeibe.read([url], layer, options=self._options)
                schema = _portable(to_wkb(sample.schema))
        except (xeibe.UnknownLayerError, xeibe.NoFeaturesError):
            # Not GML, or not this layer: the other files decide the schema.
            # Reading the file warns if it isn't GML.
            return Schema([])
        if self._layer is None:
            schema = schema.with_metadata({**(schema.metadata or {}), _LAYER_KEY: layer.encode()})
        return schema

    def open_reader(self, args: Any) -> Any:
        url = _url(args.src)
        if args.file_schema is None:
            schema = self.infer_schema(args.src)
        else:
            schema = Schema.from_arrow(args.file_schema)
        if len(schema) == 0:
            # Every file's schema was empty: none has the layer, or none is GML.
            if self._layer is None:
                raise xeibe.NoFeaturesError("GML: no file holds features: none is GML")
            raise xeibe.UnknownLayerError(
                f"GML: no file holds the layer {self._layer!r} (files that aren't GML are skipped)"
            )
        metadata = schema.metadata or {}
        layer = self._layer
        if layer is None and _LAYER_KEY in metadata:
            layer = metadata[_LAYER_KEY].decode()
        if layer is None:
            try:
                layer = self._only_layer([url])
            except xeibe.NoFeaturesError:
                layer = ""

        columns = range(len(schema)) if args.file_projection is None else args.file_projection
        fields = [schema.field(i) for i in columns]
        options = self._options
        if args.batch_size is not None:
            options = {**options, "batch_size": args.batch_size}
        if not fields:
            # No columns (`count(*)`). SedonaDB 0.4.1 can't import batches
            # without columns, and takes one column in their place.
            fields = [schema.field(_cheapest_column(schema))]
        projected = Schema(fields, metadata=metadata)
        if not layer:
            return RecordBatchReader.from_batches(projected, [])
        # The given schema is the projection: other columns aren't built. A
        # file that isn't GML reads as no rows, with a SkippedSourceWarning.
        return read_or_empty([url], layer, projected, options)

    def _given_schema(self, layer: str) -> Schema | None:
        """The layer's schema from `schema` or `settings`, as WKB."""
        schema = self._schema
        if schema is None and self._settings is not None:
            schema = layer_schema(self._settings, layer)
        elif isinstance(schema, xeibe.Scan):
            schema = schema.schema(layer)
        elif schema is not None and not hasattr(schema, "__arrow_c_schema__"):
            schema = layer_schema(schema, layer)
        return None if schema is None else to_wkb(schema)

    def _only_layer(self, paths: list[str]) -> str:
        """The layer when none was given: the only one a sampled scan finds,
        if that scan reads all the input."""
        sample = self._options["sample"]["features_per_layer"]
        return _only_layer(xeibe.scan(paths, sample=sample, options=self._options))


def read_gml(
    sd: Any,
    paths: str | os.PathLike | Iterable[str | os.PathLike],
    layer: str | None = None,
    *,
    schema: Any = None,
    settings: Any = None,
    options: Mapping[str, Any] | None = None,
    extension: str = "",
) -> Any:
    """One layer of GML files as a SedonaDB DataFrame.

    Unlike `sd.read(paths, format=GmlFormat(), …)`, the layer and the schema
    are decided once for all the files: the layer by a sampled scan (see
    `GmlFormat`), the schema by sampling the layer across the files, as
    `xeibe.read()` does. `options` are the `key=value` read options.

    `extension`, if given, selects the files of directories and globs.
    """
    if isinstance(paths, (str, os.PathLike)):
        paths = [paths]
    paths = [os.fspath(path) for path in paths]
    gml = GmlFormat(extension=extension).with_options(
        {"layer": layer, "schema": schema, "settings": settings, **(options or {})}
    )
    if layer is None:
        layer = gml._only_layer(paths)
    if gml._given_schema(layer) is None:
        schema = xeibe.read(paths, layer, options=gml._options).schema
    gml = gml.with_options({"layer": layer, "schema": schema})
    return sd.read(paths, format=gml)


def _only_layer(scan: xeibe.Scan) -> str:
    names = [layer["name"] for layer in scan.layers]
    if scan.is_complete and len(names) == 1:
        return names[0]
    if not names:
        raise xeibe.XeibeError("GML: no features found")
    seen = "layers" if scan.is_complete else "layers seen in a sample"
    raise xeibe.XeibeError(f"GML: pass the layer option; {seen}: {', '.join(names)}")


def _portable(schema: Schema) -> Schema:
    """The schema with only the metadata a read needs."""
    fields = [
        field.with_metadata({k: v for k, v in (field.metadata or {}).items() if k in _FIELD_KEYS})
        for field in schema
    ]
    metadata = {k: v for k, v in (schema.metadata or {}).items() if k in _SCHEMA_KEYS}
    return Schema(fields, metadata=metadata)


def _cheapest_column(schema: Schema) -> int:
    """A column that is cheap to build: not geometry, not a list."""
    for index, field in enumerate(schema):
        extension = (field.metadata or {}).get(b"ARROW:extension:name")
        if extension is None and not DataType.is_list(field.type):
            return index
    return 0


def _url(src: Any) -> str:
    url = src.to_url()
    if url is None:
        raise xeibe.XeibeError(f"GML: can't open {src!r}: it has no URL")
    return url
