# Type mapping

How observed values and structures become Arrow types. [schema-inference.md](schema-inference.md)
explains how a type is chosen. This document describes what each choice produces.

Principles:
- **Use rich Arrow types.** Store values as typed data, not as strings.
- **No `Decimal128`.** Downstream support is too uneven (Parquet readers, GeoPandas,
  some engines). Losslessness is guaranteed *by value*, not by text (`exact_value`).
- **Keep types stable across files and pages.** For example, integers are `Int64`
  by default, not the smallest type that fits.

**Notation.** Types are written exactly as `arrow-schema` prints and parses them.
The [settings file](architecture.md#settings-file) accepts these strings as well as
PostgreSQL-style aliases (`text`, `bigint`, `timestamptz`, `text[]`, …), which the
scan writes. The alias of each inferred type is given in brackets below.

| Type | Meaning |
|---|---|
| `Utf8View` | String (UTF-8), in Arrow's "view" layout, which DataFusion, Polars and DuckDB handle best. `Utf8` (classic layout, `string_view = false`) and `LargeUtf8` (64-bit offsets) are the other string types. All three become the same string column in Parquet |
| `Int64`, `Float64`, `Int32`, … | Integer / floating-point number, with its size in bits |
| `Date32` | Calendar date (days since 1970-01-01) |
| `Timestamp(µs, "UTC")`, `Timestamp(µs)` | Date and time in microseconds, with a time zone / without one (local time) |
| `List(T)` | Repeated values of type `T` |
| `Map(Utf8View → Utf8View)` | **Shorthand used in these docs** for a string → string map. The full Arrow type is `Map("entries": non-null Struct("key": non-null Utf8View, "value": Utf8View), unsorted)` |

## Scalars

| Inferred type | Arrow type | Notes |
|---|---|---|
| BOOL | `Boolean` (`boolean`) | Only `true`/`false` in lossless modes; `1`/`0` accepted only with `Lossy` |
| INT | `Int64` (`bigint`) | `IntWidth::Smallest` → `Int8…Int64`. Values outside the i64 range → FLOAT (if exact) or STRING |
| FLOAT | `Float64` (`double`) | `gml:max_scale` metadata = largest number of fractional digits seen |
| DATE | `Date32` (`date`) | Dates may carry a time zone (`2021-03-04Z`). See [Dates and times of day with a time zone](#dates-and-times-of-day-with-a-time-zone) |
| DATETIME (all values have a time zone) | `Timestamp(µs, "UTC")` (`timestamptz`) | Values normalized to UTC. See "mixed offsets" below |
| DATETIME (no value has a time zone) | `Timestamp(µs)` (`timestamp`) | Local/unspecified time, as in the source |
| DATETIME (some with a time zone, some without) | `Utf8View` (`text`) | Can't be represented losslessly in one Arrow column |
| TIME | `Time64(µs)` (`time`) | Time of day (`14:30:00`). Same time-zone rules as DATE |
| STRING | `Utf8View` (`text`) | Also `xs:duration` values (`P1Y2M3DT4H`), kept as ISO 8601 text: Arrow's `Interval` type is poorly supported downstream, and `Duration` can't hold months or years. `Utf8` if `string_view = false`. Never `Dictionary`, even for columns with few distinct values: see [Output format notes](#output-format-notes) |
| never seen with a value | `Utf8View` (all null) | Or the `Null` type with `AllNull::Null` |

### Timestamp precision

The default unit is microseconds, which Parquet, DuckDB and pandas all handle well.
If more than 6 fractional-second digits are seen, `exact_value` rejects the
timestamp. It stays STRING unless `timestamps.unit = Nanosecond`.

### Mixed time-zone offsets

Arrow stores one time zone per column. When all values have a time zone but the
offsets differ (for example `+01:00` in winter and `+02:00` in summer), the column
is `Timestamp(µs, "UTC")`. The instant in time is exact; the original offsets are
not kept. They are almost always daylight-saving time.

When the offset is the same for every value, it is stored in field metadata
(`gml:tz_offset = "+02:00"`). PRG's `wersjaId`, for example, is `+02:00`
throughout.

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

Schemas are flat: one column per leaf path, never a `Struct` for XML structure.
Names and paths are described in
[schema-inference.md](schema-inference.md#31-column-names), lists and their
alignment in [Lists and alignment](schema-inference.md#lists-and-alignment).

| Observed shape | Columns (default options) |
|---|---|
| element with text only | one scalar column (above) |
| element with text + attributes (`<area uom="m2">`) | `area` (the text) and `@uom` (path `area/@uom`) |
| element with children | no column of its own; one column per leaf below it |
| element repeated within its parent (`max_occurs > 1`) | every column below it is a list `T[]`, anchored on it |
| property given only as `xlink:href` | `text` (path `<property>/@href`) |
| repeated href-only property | `text[]` (path `<property>[]/@href`) |
| INSPIRE type wrapper | `*` in the path, left out of the name |
| mixed content (text + elements) | `text` containing the raw XML fragment |
| subtree beyond `Limits` / dynamic element names | one `map` column, `Map(Utf8View → Utf8View)` (relative path → text) |
| geometry property | GeoArrow extension type (see [geometry.md](geometry.md)). Below a repeated element: `geometry[]`, a list of WKB |
| `gml:boundedBy` (`BoundedBy::BoxStruct`) | `geoarrow.box`: `Struct("xmin": Float64, "ymin": Float64, "xmax": Float64, "ymax": Float64)` (+ `zmin/zmax`) |

The only `Struct`s in a schema are inside GeoArrow types (native coordinates,
`geoarrow.box`) and `Map`s.

### Nullability

**Every field is nullable**: columns and list items. A scan
or sample can't prove that an element is always present in data it hasn't seen,
and a non-null column would make the next file or WFS page that lacks the element
fail. It also keeps schemas identical across files. How often an element was
present is still recorded and shown by `--explain`.

The only non-null parts are the ones Arrow itself requires: the entries and keys of
a `Map`.

An Arrow schema passed in from code may mark fields non-null. It is used as given,
and a feature without a value for such a field is a feature error (see
[architecture.md](architecture.md#error-handling)).

## Special GML/XLink properties

| Source | Default column |
|---|---|
| `@gml:id` on the feature | `@id: Utf8View` |
| `gml:identifier` + `@codeSpace` | `identifier: text` and `@codeSpace: text` (path `identifier/@codeSpace`). A constant codeSpace goes to metadata |
| `gml:name` (repeatable, `@codeSpace`) | `name: text`, or `text[]` (path `name[]`) depending on observations |
| `gml:description` | `description: text` |
| `gml:boundedBy` | dropped (`BoundedBy::Drop`) |
| `xlink:href` | the href string. `#` is stripped for local references |
| `xlink:href` **and** inline content on one property | Inline content is read, and the href is kept as a `<property>/@href` column. The standard says the link is authoritative and the content is a cached copy (07-036 §7.2.3.4), but we don't resolve links |
| `xlink:title`, `xlink:role`, `xlink:arcrole` | dropped unless `XlinkMode::Full` |
| `xsi:nil` | null |
| `@nilReason` | a `text` column with the path `<property>/@nilReason` when seen (`nil_reason = true`) |
| `gml:metaDataProperty` | raw XML string |

## Field metadata keys

Every Arrow field carries metadata that records where it came from:

| Key | Example | Meaning |
|---|---|---|
| `gml:path` | `idIIP/*/lokalnyId` | Source path relative to the feature, in the [path syntax](architecture.md#settings-file) of the settings file |
| `gml:ns` (schema-level) | JSON object prefix → URI | Namespaces of prefixed path steps. Only present when two siblings differ only by namespace |
| `gml:max_scale` | `2` | Most fractional digits seen (FLOAT) |
| `gml:attr:<name>` | `gml:attr:uom = m2` | Constant attribute moved out of the data |
| `gml:tz_offset` | `+02:00` | Time-zone offset shared by every value of a timestamp, date or time-of-day column |
| `gml:srs_name` | `EPSG:2180` | srsName as written (geometry columns) |
| `gml:axis_swapped` | `true` | The reader swapped axes (geometry columns) |

Schema-level metadata stores the GML version(s) and, optionally, the settings the
read used (`gml:settings`).

A schema given by the user (a settings file or an Arrow schema) needs none of these
keys except `gml:path`, and that only when the column's name isn't its path (see
[architecture.md](architecture.md#settings-file)).

## Output format notes

- **Parquet:**
  - `Utf8View` is written as a regular `BYTE_ARRAY` string.
  - Repeated strings are dictionary-encoded by the Parquet writer on its own (its
    default), so columns with few distinct values stay small without an Arrow
    `Dictionary` type. A `Dictionary` type would depend on what the scan or sample
    happened to see, and not every consumer handles it well.
- **GeoParquet:** geometry columns are written with `geo` metadata. See
  [geometry.md](geometry.md#parquet-and-geoparquet-output) for how curves are handled.
