# Type mapping

How observed values and structures become Arrow types. [schema-inference.md](schema-inference.md)
explains how a type is chosen. This document describes what each choice produces.

Principles:
- **Use rich Arrow types.** Store values as typed data, not as strings.
- **No `Decimal128`.** Downstream support is too uneven (Parquet readers, GeoPandas,
  some engines). Losslessness is guaranteed *by value*, not by text (`exact_value`).
- **Keep types stable across files and pages.** For example, integers are `Int64`
  by default, not the smallest type that fits.

**Notation.** Types are written exactly as `arrow-schema` prints and parses them,
which is also the format of the [settings file](architecture.md#settings-file), so
any type here can be copied into one:

| Type | Meaning |
|---|---|
| `Utf8View` | String (UTF-8), in Arrow's "view" layout, which DataFusion, Polars and DuckDB handle best. `Utf8` (classic layout, `string_view = false`) and `LargeUtf8` (64-bit offsets) are the other string types. All three become the same string column in Parquet |
| `Int64`, `Float64`, `Int32`, … | Integer / floating-point number, with its size in bits |
| `Date32` | Calendar date (days since 1970-01-01) |
| `Timestamp(µs, "UTC")`, `Timestamp(µs)` | Date and time in microseconds, with a time zone / without one (local time) |
| `List(T)` | Repeated values of type `T` |
| `Struct("a": T, "b": U)` | Nested record with named fields. Arrow prints `non-null` before the type of a required field; inferred schemas have none (see [Nullability](#nullability)) |
| `Map(Utf8View → Utf8View)` | **Shorthand used in these docs** for a string → string map. The full Arrow type is `Map("entries": non-null Struct("key": non-null Utf8View, "value": Utf8View), unsorted)` |

## Scalars

| Inferred type | Arrow type | Notes |
|---|---|---|
| BOOL | `Boolean` | Only `true`/`false` in lossless modes; `1`/`0` accepted only with `Lossy` |
| INT | `Int64` | `IntWidth::Smallest` → `Int8…Int64`. Values outside the i64 range → FLOAT (if exact) or STRING |
| FLOAT | `Float64` | `gml:max_scale` metadata = largest number of fractional digits seen |
| DATE | `Date32` | Dates may carry a time zone (`2021-03-04Z`). See [Dates and times of day with a time zone](#dates-and-times-of-day-with-a-time-zone) |
| DATETIME (all values have a time zone) | `Timestamp(µs, "UTC")` | Values normalized to UTC. See "mixed offsets" below |
| DATETIME (no value has a time zone) | `Timestamp(µs)` | Local/unspecified time, as in the source |
| DATETIME (some with a time zone, some without) | `Utf8View` | Can't be represented losslessly in one Arrow column |
| TIME | `Time64(µs)` | Time of day (`14:30:00`). Same time-zone rules as DATE |
| STRING | `Utf8View` | Also `xs:duration` values (`P1Y2M3DT4H`), kept as ISO 8601 text: Arrow's `Interval` type is poorly supported downstream, and `Duration` can't hold months or years. `Utf8` if `string_view = false`. Never `Dictionary`, even for columns with few distinct values: see [Output format notes](#output-format-notes) |
| never seen with a value | `Utf8View` (all null) | Or the `Null` type with `AllNull::Null` |

### Timestamp precision

The default unit is microseconds, which Parquet, DuckDB and pandas all handle well.
If more than 6 fractional-second digits are seen, `exact_value` rejects the
timestamp. It stays STRING unless `timestamps.unit = Nanosecond`.

### Mixed time-zone offsets

Arrow stores one time zone per column. When all values have a time zone but the
offsets differ (for example `+01:00` in winter and `+02:00` in summer):

- Default: `Timestamp(µs, "UTC")` plus an `Int16` sibling column `<name>.@offset_min`
  holding the original offset in minutes. This is lossless.
- `timestamps.keep_offset = false`: UTC only. The offset is lost, but the instant
  in time is preserved.

When the offset is the same for every value, no offset column is added. The offset
is stored in field metadata (`gml:tz_offset = "+02:00"`).

### Dates and times of day with a time zone

In XML a date or a time of day may end with a time zone: `2021-03-04Z` (UTC),
`2021-03-04+02:00`, `14:30:00+01:00`. Arrow's `Date32` (a day count) and
`Time64` (time since midnight) have no place to store one. It is common in real
data: 25 WFS responses in the corpus, from 10 services, write every date with a
trailing `Z` (e.g. `<eintragung>2019-05-12Z</eintragung>`).

The column's time zones decide the type (`TzShape` in the observation):

| Values in the column | Arrow type | Time zone kept in |
|---|---|---|
| none has a time zone | `Date32` / `Time64(µs)` | — |
| all have the **same** time zone (`Z` and `+00:00` count as the same) | `Date32` / `Time64(µs)` | field metadata, `gml:tz_offset = "+00:00"`. Lossless, because it's the same for every row |
| all have one, but **different** ones | `Utf8View` | the text itself |
| some have one, some don't | `Utf8View` | the text itself |

With `lossless = Lossy`, the last two become `Date32` / `Time64(µs)` too, and the
time zones are dropped.

Unlike a timestamp, a date isn't converted to UTC: `2021-03-04+02:00` stays
2021-03-04.

## Structure

| Observed shape | Arrow type (default options) |
|---|---|
| element with text only | scalar (above) |
| element with text + attributes | `Struct("#text": T, "@attr": …)` (`SimpleContent::Struct`) |
| element with children | `Struct("child": …, …)` |
| repeated element (`max_occurs > 1`) | `List(T)` |
| property given only as `xlink:href` | `Utf8View` (href) |
| repeated href-only property | `List(Utf8View)` |
| INSPIRE type wrapper | collapsed. The wrapper's children are placed directly in the property's struct |
| mixed content (text + elements) | `Utf8View` containing the raw XML fragment |
| subtree beyond `Limits` / dynamic element names | `Map(Utf8View → Utf8View)` (path → text) |
| streaming overflow | `_overflow: Map(Utf8View → Utf8View)` |
| geometry property | GeoArrow extension type (see [geometry.md](geometry.md)) |
| `gml:boundedBy` (`BoundedBy::BoxStruct`) | `Struct("xmin": Float64, "ymin": Float64, "xmax": Float64, "ymax": Float64)` (+ `zmin/zmax`), geoarrow.box |

### Nullability

**Every field is nullable**: top-level columns, struct fields and list items. A scan
or sample can't prove that an element is always present in data it hasn't seen,
and a non-null column would make the next file or WFS page that lacks the element
fail. It also keeps schemas identical across files. How often an element was
present is still recorded and shown by `--explain`.

The only non-null parts are the ones Arrow itself requires: the entries and keys of
a `Map`, such as `_overflow`.

An Arrow schema passed in from code may mark fields non-null. It is used as given,
and a feature without a value for such a field is a feature error (see
[architecture.md](architecture.md#error-handling)).

### Flattening (with `Nesting::FlattenSingleOnly` or `Flatten`)

Names are joined with `flatten_separator` (default `.`):
`idIIP.lokalnyId`, `area.@uom`. Repeated elements are never flattened, because
flattening can't represent more than one value losslessly.

## Special GML/XLink properties

| Source | Default column |
|---|---|
| `@gml:id` on the feature | `@id: Utf8View` |
| `gml:identifier` + `@codeSpace` | `identifier: Struct("#text": Utf8View, "@codeSpace": Utf8View)`. A constant codeSpace goes to metadata |
| `gml:name` (repeatable, `@codeSpace`) | `name: Utf8View` or `List(Utf8View)` depending on observations |
| `gml:description` | `description: Utf8View` |
| `gml:boundedBy` | dropped (`BoundedBy::Drop`) |
| `xlink:href` | the href string. `#` is stripped for local references |
| `xlink:href` **and** inline content on one property | Inline content is used, and the href is kept as `@href` in the struct. The standard says the link is authoritative and the content is a cached copy (07-036 §7.2.3.4), but we don't resolve links |
| `xlink:title`, `xlink:role`, `xlink:arcrole` | dropped unless `XlinkMode::Full` |
| `xsi:nil` | null |
| `@nilReason` | `<field>.@nilReason: Utf8View` when seen (`nil_reason = true`) |
| `gml:metaDataProperty` | raw XML string |

## Field metadata keys

Every Arrow field carries metadata that records where it came from:

| Key | Example | Meaning |
|---|---|---|
| `gml:path` | `prgad:idIIP/prgad:AD_IdentyfikatorIIP/prgad:lokalnyId` | Source path relative to the feature (prefixes as first seen; namespace URIs in `gml:ns`) |
| `gml:ns` | JSON object prefix → URI | Namespaces used by `gml:path` |
| `gml:max_scale` | `2` | Most fractional digits seen (FLOAT) |
| `gml:attr:<name>` | `gml:attr:uom = m2` | Constant attribute moved out of the data |
| `gml:tz_offset` | `+02:00` | Time-zone offset shared by every value of a timestamp, date or time-of-day column |
| `gml:srs_name` | `EPSG:2180` | srsName as written (geometry columns) |
| `gml:axis_swapped` | `true` | The reader swapped axes (geometry columns) |

Schema-level metadata stores the GML version(s) and, optionally, the settings the
read used (`gml:settings`).

A schema given by the user (a settings file or an Arrow schema) needs none of these
keys. Columns are matched to XML by name (see
[architecture.md](architecture.md#settings-file)). `gml:path` is only needed for
renamed columns.

## Output format notes

- **Parquet:**
  - `Utf8View` is written as a regular `BYTE_ARRAY` string.
  - Repeated strings are dictionary-encoded by the Parquet writer on its own (its
    default), so columns with few distinct values stay small without an Arrow
    `Dictionary` type. A `Dictionary` type would depend on what the scan or sample
    happened to see, and not every consumer handles it well.
- **GeoParquet:** geometry columns are written with `geo` metadata. See
  [geometry.md](geometry.md#parquet-and-geoparquet-output) for how curves are handled.
