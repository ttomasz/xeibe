# Schema inference

GML datasets rarely come with a usable application schema (XSD). The schema is
therefore **inferred from the data**, in two separate steps:

```mermaid
flowchart LR
    subgraph scan ["scan (observation)"]
        data(["data"]) --> tree["path tree per layer"] --> merge["merge"]
    end
    subgraph rules ["rules (policy)"]
        options["InferenceOptions"] --> schema(["Arrow Schema per layer"])
    end
    merge --> options
    schema --> settings[("settings file<br/>(like GDAL's .gfs, editable)")]
```

- The **path tree** records *what was seen*. It makes no decisions.
- **`InferenceOptions`** decides *how to represent it*: types, lists, column names.

Within one scan, changing the options needs no new pass: the same tree can produce
a typed schema and an all-strings one. The tree itself is not saved.
What is saved is the resulting schema, in the settings file (see
[section 2.7](#27-scan-results-and-the-settings-file)).

The same inference runs in two places:
- **`scan`**, over the whole input or its first N features, for every layer;
- **`read` without a schema**, over the first features of the requested layer
  ([section 6](#6-sampled-schemas)).

---

## 1. How other tools do it (for context)

| Tool | Approach | Limits |
|---|---|---|
| GDAL GML driver | XSD if one is present and simple enough → `.gfs` cache → otherwise a **full scan**. Flattens nested elements. Supports lists of scalars only. XML attributes become fields only with `GML_ATTRIBUTES_TO_OGR_FIELDS`. Widens types in a line: int → real → string | Nested and repeated complex properties are lost or flattened badly |
| GDAL GMLAS | Driven entirely by the XSD. Nested types become separate joined tables | Requires an XSD. Produces many tables |
| GDAL WFS | Schema from `DescribeFeatureType` | Server-dependent |
| DataFusion CSV/JSON | Samples the first N records (default 1000). arrow-json widening: int + float → float, anything + string → string | Sampling misses rare shapes. `Schema::try_merge` rejects conflicts between files |
| spark-xml | Samples. Attributes prefixed `_`, text stored in `_VALUE`. Nests as Struct/Array | Sampling. Lossy numeric inference |
| xmltodict | No schema. Attributes as `@attr`, text as `#text`. `force_list` option | Not a schema, only a mapping |

This design combines GDAL's full scan and flat columns with lists that stay aligned
(see [Lists and alignment](#lists-and-alignment)) and lossless type rules.

---

## 2. The path tree

### 2.1 Top level

```rust
/// Everything a scan observed. In memory only; the settings file stores schemas, not trees.
pub struct DatasetObservation {
    pub gml_versions: BTreeSet<GmlVersion>,     // V2 | V3_1 | V3_2 (see version detection)
    pub layers: IndexMap<QName, LayerObservation>,
    pub source_context: Vec<SourceContext>,     // producer, WFS request (axis-order evidence)
}

pub struct LayerObservation {
    pub feature_count: u64,
    pub root: ElementNode,                      // the feature element itself
    pub extent: Option<Bbox>,                   // union of geometry bboxes, for listings
}

/// Namespace URI + local name. Never the prefix: prefixes differ between files/WFS pages.
pub struct QName { pub ns: Option<Arc<str>>, pub local: Arc<str> }
```

### 2.2 Element node

```rust
pub struct ElementNode {
    // occurrence → List detection; presence shown by --explain
    pub instances: u64,          // total occurrences of this element
    pub parents_with: u64,       // parent instances that contained it ≥ 1 time
    pub max_occurs: u32,         // max repetitions within ONE parent instance
    pub first_multi: Option<Location>,  // evidence: where max_occurs first exceeded 1

    // content shape
    pub text: Option<ValueStats>,       // non-whitespace text content
    pub mixed: bool,                    // text AND child elements in the same instance
    pub empty: u64,                     // <a/> or <a></a>
    pub nil: NilStats,                  // xsi:nil count + nilReason values (bounded set)

    // structure
    pub attributes: IndexMap<QName, ValueStats>,
    pub children: IndexMap<QName, ElementNode>,  // first-seen order → stable column order

    // GML-specific
    pub geometry: Option<GeometryStats>, // set when this is a geometry property
    pub by_reference: u64,               // instances that only had xlink:href, no content
    pub has_gml_id: u64,                 // instances with their own gml:id (nested object)
    pub name_shape: NameShape,           // UpperCamel (object/type) vs lowerCamel (property)

    pub truncated: bool,                 // depth/width limit hit; subtree not tracked
}
```

**Counting rules**

- `max_occurs` is counted **per parent instance**. The parse stack keeps a small
  counter per child name. When the parent element closes, the counters are folded
  into `max_occurs`.
- `parents_with < parent.instances` means the element is sometimes missing.
  `--explain` shows it; it doesn't affect the schema, because every column is
  nullable (see [type-mapping.md](type-mapping.md#nullability)).
- `first_multi` records `(source, byte_offset, feature_seq, gml:id)`. This is what
  `--explain` shows as the reason a column became a list.

**Geometry nodes are leaves.** When a property contains a GML geometry element, the
**property** node receives `GeometryStats`, and the scan does not go deeper.
Otherwise `exterior/LinearRing/posList` would appear as columns. During the scan,
coordinates are mostly not parsed. Only these are collected: the geometry element
kinds, the presence of curve segments, `srsName`, `srsDimension`, `axisLabels`, the
GML dialect, and the **first position of each geometry**. The positions feed the
axis-order range check (see [geometry.md](geometry.md#auto-evidence-based-decision)).

### 2.3 Value statistics

Each text value reports **every type it can be parsed as**. The statistics keep the
**intersection** across all values. This replaces GDAL's single widening line, which
can't handle types with no natural order (date vs int) and can't express whether a
conversion loses information.

```rust
pub struct ValueStats {
    pub count: u64,
    pub exact_text: TypeSet,     // parses AND prints back identically
    pub exact_value: TypeSet,    // the value survives a round trip through the Arrow type
    pub lossy: TypeSet,          // parses at all
    pub int_range: Option<(i64, i64)>,
    pub float_shape: Option<FloatShape>,   // max significant digits, max fractional scale
    pub temporal: Option<TemporalShape>,   // tz: Absent | Fixed(offset) | Mixed; max fraction digits
    pub max_len: u32,
    pub distinct: BoundedSet<Arc<str>>,    // up to N (default 64) values; dropped on overflow
}

bitflags! { pub struct TypeSet: u16 {
    const BOOL; const INT; const FLOAT; const DATE; const DATETIME;
    const TIME; const STRING;                      // STRING is always set
}}
```

| Value | `exact_text` | `exact_value` | `lossy` |
|---|---|---|---|
| `0012` | STRING | STRING (leading zeros ⇒ identifier) | INT, FLOAT, STRING |
| `1523.40` | STRING | FLOAT, STRING | FLOAT, STRING |
| `1` | INT, FLOAT, STRING | INT, FLOAT, STRING | + BOOL |
| `true` | BOOL, STRING | BOOL, STRING | BOOL, STRING |
| `2021-03-04` | DATE, STRING | DATE, STRING | DATE, DATETIME, STRING |
| `2021-03-04Z` | STRING | DATE, STRING (time zone recorded in `temporal`) | DATE, DATETIME, STRING |
| `2017-04-05T14:53:55+02:00` | DATETIME, STRING | DATETIME, STRING | DATETIME, STRING |
| `12345678901234567.89` | STRING | STRING (too many digits for f64) | FLOAT, STRING |

Rules for `exact_value`:
- **Leading zeros** in the integer part, as in `0012` or `-012` (but not `0` or `0.5`),
  rule out numeric types. Such values are almost always identifiers (codes,
  postcodes, TERYT).
- **Trailing zeros** after the decimal point are allowed (`1523.40` → `1523.4`). The
  maximum scale is kept in field metadata (`gml:max_scale`), so a writer can
  reproduce the original formatting.
- FLOAT requires the value's normalized decimal form to equal the shortest
  round-trip form of the parsed `f64`.
- `1`/`0` are BOOL only in `lossy`. In `exact_*`, only `true`/`false` are BOOL.

Only one of `exact_text` / `exact_value` / `lossy` is used, as set by
`TypeOptions.lossless`. The default is `Value`.

**Speed:** once a `TypeSet` contains only `STRING`, later values are not parsed.
Only `distinct`, `max_len` and `count` are updated.

### 2.4 Geometry statistics

```rust
pub struct GeometryStats {
    pub count: u64,
    pub kinds: BTreeSet<GmlGeomKind>,   // Point, LineString, Curve, Polygon, Surface, MultiSurface, …
    pub has_curves: bool,               // any arc/circle segment seen
    pub has_unsupported: bool,          // e.g. Solid, spline — see support-matrix
    pub dims: BTreeSet<u8>,             // effective srsDimension: 2, 3
    pub srs: BTreeMap<Arc<str>, u64>,   // srsName (as written) → count
    pub axis_evidence: BTreeMap<AxisKey, AxisEvidence>, // per (srsName, dialect):
                                        //   as-written bbox of sampled positions,
                                        //   axisLabels seen, envelope bboxes
    pub by_reference: u64,              // geometry given only as xlink:href
    pub empty: u64,
}
```

### 2.5 Merging

Every structure has a `merge(&mut self, other: Self)` operation that is
**associative and commutative**:

| Field | Merge |
|---|---|
| counts | add |
| `max_occurs`, `max_len` | max |
| `TypeSet`s | intersect |
| `int_range` | min/max |
| `distinct` | union until overflow, then dropped |
| `children`, `attributes` | merge by `QName`, recursively |
| `first_multi` | the earliest location |

This one property allows:
- a **parallel scan**: one tree per chunk, then merged;
- **multi-file inputs and WFS pages**: one tree per file or page, then merged.

Column order is the only non-commutative part. Children are ordered by their
earliest `(source_index, byte_offset)` of first appearance, which is deterministic
whatever the merge order.

### 2.6 Size

The tree grows with the **number of distinct element paths**, not with the file
size.

| Part | Approximate size |
|---|---|
| Node (counters, flags, evidence) | ~100 B |
| `ValueStats` without distinct values | ~80 B |
| `BoundedSet` (N = 64) | 0 – ~2.5 KB (dropped for high-cardinality fields) |

| Dataset shape | Nodes | Tree |
|---|---|---|
| Flat national layer (addresses, parcels) | 50–150 | 20–150 KB |
| Multi-layer package (~80 feature types) | 2k–8k | 0.5–5 MB |
| Dynamic element names | capped by `Limits.max_children` | bounded |

Memory during the scan is one tree per worker plus the parse stack, which is
negligible.

### 2.7 Scan results and the settings file

- A scan returns a `ScanResult`: the observation, the options used, and one
  `LayerSchema` per layer. In the same process it can produce schemas for other
  options (`scan.schema_with(layer, &options)`) without another pass.
- `ScanResult::to_settings()` gives the [settings file](architecture.md#settings-file):
  the options, the axis-order decision (one mode, plus per-srsName overrides only
  where keys were decided differently), and one
  `column → type` map per layer. This is what users keep and pass to later reads.
  The path tree is not saved.
- The settings file has no link to the data it came from: no fingerprints and no
  invalidation. It can be applied to any input with the same feature types (for
  example, one voivodeship's PRG scan for all 16). Content the schema doesn't
  describe is not read (see [6.3](#63-data-that-doesnt-fit)).
- A read can also embed the schema it used in the output's Parquet key-value
  metadata (`gml:settings`), to record where it came from.

### 2.8 GML version detection

GML 2 and GML 3.1 share the namespace `http://www.opengis.net/gml`. GML 3.2 uses
`http://www.opengis.net/gml/3.2`. For the shared namespace, the version is
determined from:

1. `xsi:schemaLocation` pointing to `…/gml/2.x` or `…/gml/3.x`;
2. the WFS version and `outputFormat` for WFS responses;
3. the elements seen: `coordinates`, `outerBoundaryIs`, `coord` → 2;
   `pos`, `posList`, `exterior`, `Curve`, `Surface` → 3.1.

Geometry parsing accepts elements from all versions, so detection affects only
reporting and some defaults (for example, `boundedBy` handling).

---

## 3. `InferenceOptions`

```rust
pub struct InferenceOptions {
    pub structure: StructureOptions,
    pub types: TypeOptions,
    pub gml: GmlOptions,
    pub geometry_encoding: GeomEncoding, // Auto (default) | Wkb, see geometry.md
    pub overrides: Vec<(PathPattern, FieldOverride)>,
    pub layers: Vec<(LayerSelector, InferenceOptionsPatch)>,   // per-layer adjustments
    pub limits: Limits,
}
```

### 3.1 Column names

A schema is flat: **one column per leaf path**. A leaf is an element's text, an
attribute, or a geometry property. XML structure never becomes a `Struct`.

Every column has a path (see [architecture.md](architecture.md#settings-file) for
the syntax) and a name. The read only uses the path. The name is a suggestion
written into the settings file, where users can change it. The scan suggests the
**shortest unique name**:

1. Take the path's steps. Drop type wrappers (`*`, see [3.2](#32-structure)) and
   namespace prefixes. For a property given only by `xlink:href`, drop the `@href`
   step too, since the column holds the href.
2. Start with the last step. While two columns of the layer share a name, each of
   them that has steps left takes one more step from the front. Steps are joined
   with `.`.
3. Two paths that differ only by namespace keep the prefix on the step that differs
   (`gml:name`, `app:name`), and their paths use it as well.

| Path | Name |
|---|---|
| `@id` (the feature's `gml:id`) | `@id` |
| `nazwa` | `nazwa` |
| `idIIP/*/lokalnyId` | `lokalnyId` |
| `idIIP/*/@id` (the wrapper's `gml:id`) | `idIIP.@id` (`@id` is taken by the feature) |
| `inspireId/*/namespace`, `hydroId/*/namespace` | `inspireId.namespace`, `hydroId.namespace` |
| `area`, `area/@uom` | `area`, `@uom` (`area.@uom` if another `@uom` exists) |
| `miejscowosc/@href` (by reference only) | `miejscowosc` |
| `externeReferenz[]/*/datum` | `datum` |

- XML attributes **always** keep the `@` prefix, so an attribute never collides
  with an element of the same name.
- A name depends on the other columns of the layer, so scans of different data can
  suggest different names for the same path. Once written to a settings file, a
  name stays as it is.

### 3.2 Structure

```rust
pub struct StructureOptions {
    pub lists: ListRule,                   // Infer (max_occurs > 1) (default) | Never
    pub force_list: Vec<PathPattern>,      // xmltodict-style
    pub force_scalar: Vec<PathPattern>,
    pub constant_attrs: ConstantAttrs,     // ToFieldMetadata (default) | Keep
    pub collapse_type_wrappers: bool,      // default true
    pub mixed_content: MixedContent,       // RawXml (default) | TextOnly | Drop
    pub xml_attributes: AttrSelect,        // All (default) | None | Only(..) | Except(..)
}
```

**Text and attributes** are separate columns. `<area uom="m2">1523.40</area>`
gives `area: double` (path `area`) and `@uom: text` (path `area/@uom`).

**`constant_attrs = ToFieldMetadata`**: if an attribute has exactly one distinct
value across the dataset (`uom="m2"` everywhere), it is moved into field metadata
(`gml:attr:uom = "m2"`) instead of becoming a column. No information is lost.

**`collapse_type_wrappers`**: INSPIRE-style schemas wrap data types inside
properties:

```xml
<prgad:idIIP>
  <prgad:AD_IdentyfikatorIIP>          ← wrapper: object/type element
    <prgad:lokalnyId>…</prgad:lokalnyId>
```

An element is a type wrapper when **all** of these hold:
- each instance of the property contains exactly one child element;
- every child name seen there has `name_shape == UpperCamel` and `max_occurs == 1`;
- the property has no text and no attributes other than `xlink`/`nil` attributes;
- the wrapper has no attributes other than `@gml:id`.

The wrapper step is written as `*` in paths (`idIIP/*/lokalnyId`) and left out of
names. When several wrapper types occur, as with XPlanung's `XP_ExterneReferenz`
and `XP_SpezExterneReferenz` inside `externeReferenz`, their subtrees are merged:
`externeReferenz[]/*/referenzName` gets the value from either. The wrapper's
`@gml:id`, if present, is kept as a column (`idIIP/*/@id`).

#### Lists and alignment

A column is a **list** (`T[]`) when some element on its path repeats within its
parent (`max_occurs > 1`), or when `force_list` matches. Its **anchor** is the
element whose occurrences the list follows: the innermost repeating element on the
path, marked with `[]` in the path.

Every column under one anchor has **one entry per occurrence of the anchor**, with
null where an occurrence lacks the value. Entry *i* of each of these columns comes
from the anchor's *i*-th occurrence, so the lists can be zipped back into records
(`list_zip`, `arrays_zip`, pandas/Polars `explode` on several columns):

```xml
<adres><ulica>Polna</ulica><numer>1</numer></adres>
<adres><numer>2</numer></adres>
```
```
ulica  (path adres[]/ulica) = ["Polna", null]
numer  (path adres[]/numer) = ["1", "2"]
```

The reader keeps a counter per anchor while it reads a feature:

1. When an anchor element starts, its counter goes up by one (before its attributes
   are read, so `name[]/@codeSpace` sees the new count).
2. Before a value is added, the column is padded with nulls to `count - 1` entries,
   then the value is appended. This gives leading nulls when the first occurrences
   lack the value.
3. If the column already has `count` entries, the value is a second one within the
   same occurrence: the schema doesn't match the data, and the feature fails (see
   [6.3](#63-data-that-doesnt-fit)).
4. When the feature ends, each column is padded with nulls to its anchor's count.
   A feature in which the anchor never occurs gets `null`, not an empty list.

This was checked against the corpus: on 32,333 features with repeated elements (the
first 300 features of 264 files), the streaming padding matched a reference that
groups each anchor occurrence from the whole feature.

- A path has at most one anchor, so there are no lists of lists. When elements
  repeat at two levels (a repeated `spelling` inside a repeated `name`), the scan
  anchors columns below the inner one on the inner one (`name/spelling[]/text`).
  They keep all their values and stay aligned with each other, but not with the
  columns anchored on `name`. The sample of the corpus has no such case.
- A geometry property below a repeating element becomes a list of WKB blobs,
  `geometry[]` (`List(geoarrow.wkb)`), aligned like any other list. It is always
  WKB, because no native GeoArrow type holds several geometries per row in one
  column. GeoParquet metadata covers only top-level geometry columns, so such a
  column is written as a plain list of WKB. The only case in the corpus is GDAL's
  test file `gmlsubfeature.gml`, where GDAL makes one geometry column and keeps
  only the last polygon.
- In the corpus, 44 of 719 feature types have a repeated element. Half are
  repeated references (a single `text[]` of hrefs). In all but one of the rest,
  every occurrence has the same children. Only `OM_Observation` (SWE `field` holds
  `Time`, `Category` or `Quantity`) needs the null padding to stay aligned.

### 3.3 Types

```rust
pub struct TypeOptions {
    pub lossless: Lossless,            // Text | Value (default) | Lossy
    pub enabled: TypeSet,              // STRING only ⇒ GDAL's ALWAYS_STRING
    pub integers: IntWidth,            // Int64 (default) | Smallest
    pub timestamps: TimestampOptions,  // unit (µs default), mixed-tz handling
    pub empty_as_null: bool,           // default true
    pub all_null: AllNull,             // Utf8View (default) | Null type
    pub string_view: bool,             // Utf8View (default) vs Utf8
}
```

Type choice: take the chosen `TypeSet` ∩ `enabled`, then pick the first match in
this order: **BOOL → INT → FLOAT → DATE → DATETIME → TIME → STRING**.
[type-mapping.md](type-mapping.md) lists the resulting Arrow types.

`Int64` is the default width, not the smallest possible type. This keeps schemas
stable across files and WFS pages. Otherwise a column that is Int8 in one file and
Int16 in another has to be widened.

### 3.4 GML-specific

```rust
pub struct GmlOptions {
    pub gml_id: IdMode,                // Column (default; "@id") | Drop
    pub xlink: XlinkMode,              // Href (default) | Full{href,title,role,arcrole} | Drop
    pub strip_local_href_hash: bool,   // "#PL.X.1" → "PL.X.1" (default true)
    pub nil_reason: bool,              // keep nilReason as a `<path>/@nilReason` column (default true when seen)
    pub bounded_by: BoundedBy,         // Drop (default) | BoxStruct | Geometry
    pub standard_props: StdProps,      // gml:name/description/identifier handling
}
```

- Properties given only by reference, such as
  `<prgad:miejsce xlink:href="#…"/>`, become a column `miejsce` with the path
  `miejsce/@href`. Repeated ones (`adres2`) become `text[]` (path
  `adres2[]/@href`). These act as foreign keys to another layer's `@id`. A property
  that sometimes has inline content gets both the `@href` column and the columns of
  its content.
- `xsi:nil="true"` means null. `nilReason` is kept if configured.

### 3.5 Overrides and limits

```rust
pub enum FieldOverride {
    Type(DataType), Drop, AsRawXml, AsMap, List, Scalar, Geometry(GeometryOverride),
}

pub struct Limits {
    pub max_depth: u16,       // default 16 → deeper subtrees become one `map` column
    pub max_children: u16,    // default 512 → element with more distinct child names becomes Map
    pub distinct_values: u16, // BoundedSet capacity (default 64)
}
```

Names are not overridden here: they are edited in the settings file.

`PathPattern` matches local names with globs, optionally namespace-qualified:
`AD_PunktAdresowy/idIIP`, `*/area`, `**/@uom`, `{https://geoportal.gov.pl/schemas/prgad/1.0}*/**`.

### 3.6 Presets

| Preset | Summary |
|---|---|
| `default()` | Lossless by value, `@` attributes, rich types, geometry encoding `Auto` |
| `strings()` | Every scalar as `text` |

---

## 4. The rule engine

The engine walks each layer's tree and emits one column per leaf, then names them:

```
fn columns_for(node, path, anchor, opts, out):
    if override matches                → apply override
    if node.geometry.is_some()         → geometry column at path (geometry.md);
                                         under an anchor: geometry[] (list of WKB, see 3.2)
    if node.truncated                  → one `map` column at path
    if is_list(node)                   → anchor = path[]   (innermost wins)

    match shape(node):
        TextOnly                       → scalar(node.text, opts.types)
        ByReferenceOnly                → text column at path/@href
        Mixed                          → mixed_content rule (raw XML text at path)
        Empty (never had content)      → all_null type
        ElementsOnly                   → (no column of its own)
    for each attribute (xml_attributes, constant_attrs) → column at path/@attr
    for each child                     → columns_for(child, path/step(child), anchor, …)
                                         step(child) = `*` for a type wrapper (collapse_type_wrappers);
                                         several wrapper types under one property are merged first

    a column under an anchor gets T[]; nullable = true (always)
    attach metadata: gml:path, gml:max_scale, gml:attr:*, gml:srs_name, …

then: suggest names (3.1)
```

`is_list(node)` is `(max_occurs > 1 || force_list) && !force_scalar`.

Every field records its source path in metadata (`gml:path`). The reader builds a
lookup tree from the schema's paths and sends each value straight to its builder.
Subtrees that no path leads into are skipped without being parsed.

---

## 5. Worked example: PRG `AD_PunktAdresowy`

The observed tree below is based on a sample of about 410k address points from the
Lubelskie file:

```
prgad:AD_PunktAdresowy           features ~410k        @gml:id exact_value{STRING}
├─ idIIP                         present always, max 1  (elements only)
│  └─ AD_IdentyfikatorIIP        [UpperCamel, only child] → type wrapper
│     ├─ lokalnyId               text {STRING}                       ← UUID
│     ├─ przestrzenNazw          text {STRING} distinct{"PL.PZGIK.200"}
│     └─ wersjaId                text {DATETIME tz=Fixed(+02:00)…, STRING}
├─ poczatekWersjiObiektu         text {DATETIME tz=Absent, STRING}
├─ numerPorzadkowy               text {STRING}                       ← "27a", "33"
├─ georeferencja  [GEOMETRY]     kinds{Point} dims{2} srs{"EPSG:2180"}
├─ kodPocztowy                   text {STRING}                       ← "22-120"
├─ dataNadania                   present ~52%; text {DATE, STRING}
├─ miejscowosc                   by_reference = instances            ← xlink only
└─ ulica2                        present ~13%; by_reference
```

Default schema (`InferenceOptions::default()`; every column nullable), as the
scan writes it into the settings file:

```
"@id":                   "text"
"lokalnyId":             { "type": "text",        "path": "idIIP/*/lokalnyId" }
"przestrzenNazw":        { "type": "text",        "path": "idIIP/*/przestrzenNazw" }
"wersjaId":              { "type": "timestamptz", "path": "idIIP/*/wersjaId" }
"poczatekWersjiObiektu": "timestamp"
"numerPorzadkowy":       "text"                  ("33" alone would be INT; "27a" rules it out)
"georeferencja":         "geometry(Point)"       native point, crs = EPSG:2180
"kodPocztowy":           "text"
"dataNadania":           "date"
"miejscowosc":           { "type": "text", "path": "miejscowosc/@href" }   ('#' stripped) → FK to AD_Miejscowosc.@id
"ulica2":                { "type": "text", "path": "ulica2/@href" }        → FK to AD_UlicaPlac.@id
```

The layer `AD_UlicaPlac` in the same file shows repeated references:
`<prgad:adres2 xlink:href="#…"/>` appears several times per street, so it becomes
`"adres2": { "type": "text[]", "path": "adres2[]/@href" }`.

`przestrzenNazw` has a single constant value but stays a column. Only XML
*attributes* with a constant value are moved to metadata. A text element with a
constant value is still data.

---

## 6. Sampled schemas

A schema inferred from part of the data is a **sampled schema**. There are two
ways to get one:

- `read` without a schema samples the requested layer;
- `xeibe scan --sample N` (`ScanExtent::Sample`) scans only the first N features of
  the input.

### 6.1 Sampling in a read

```rust
pub struct SampleOptions {
    pub features_per_layer: u64,     // default 10_000
    pub max_buffer_bytes: u64,       // default 256 MiB
    pub min_typed_values: u64,       // default 100, see 6.2
    pub conservative: bool,          // default true, see 6.2
}
```

- **The sample is taken from the requested layer, not from the start of the file.**
  PRG stores its layers one after another: tens of thousands of `AD_Miejscowosc`
  come first, then `AD_UlicaPlac`, then millions of `AD_PunktAdresowy`. The
  splitter skips the other layers, so the sample is the first `features_per_layer`
  address points, wherever they start.
- The sampled features are buffered. When the sample is full, or the buffer reaches
  `max_buffer_bytes`, or the input ends, the schema is inferred from the sample's
  path tree and **frozen**. The reader's schema becomes known at that moment. The
  buffered features are then emitted first, followed by the rest of the stream.
- Axis-order evidence ([geometry.md](geometry.md#auto-evidence-based-decision)) and
  the CRS for the column metadata come from the same sample.
- The frozen schema is in the read report, in settings-file form. Saving it makes
  the next read of the same kind of data one-pass with a known schema.

A sampled **scan** is simpler: it stops after the first N features of the input,
whatever their layers. Layers that start later are missing from the result, and
layers with few sampled features get weak type evidence. It is a quick look, not
a substitute for a full scan.

### 6.2 Conservative types

A schema can't change after the first batch is written (a Parquet file has a single
schema), so type choices in a sampled schema must be safe for data that hasn't been
seen yet. With `conservative = true`, these adjustments are applied on top of any
preset:

| Rule | Full scan | Sampled, conservative |
|---|---|---|
| Typed columns (anything but string) | any evidence | at least `min_typed_values` non-null values in the sample; otherwise `Utf8View` |
| Integer width | `integers` option | always `Int64` |
| Date-only values | `Date32` | `Date32` (a later timestamp is a feature error) |
| Geometry encoding `Auto` | native type if the scan saw one simple kind | `geoarrow.wkb` (a `Polygon` sample says nothing about later `MultiPolygon`s) |
| Lists | from `max_occurs` | same (a later repetition of a scalar is a feature error) |

### 6.3 Data that doesn't fit

Any schema used by a read can meet data it doesn't describe: a sampled schema, a
settings file from an older release of the data, or a schema written by hand. The
rule is simple: **what the schema describes is read, everything else is not.**

| Situation | Result |
|---|---|
| Element or attribute whose path isn't in the schema | not read, not reported |
| A second value where the schema has one (a scalar column, or one entry per anchor occurrence) | feature error: the column has the wrong type for the data |
| Value doesn't parse as the column's type | feature error (`OnFeatureError`, see [architecture.md](architecture.md#error-handling)) |
| Geometry kind the native column can't hold | geometry error (`OnFeatureError`, `NullGeometry` applies) |

Content the schema leaves out is a choice; a value that doesn't fit the type of a
column the schema does have is a mismatch, and the read doesn't guess. A new scan
brings new elements into the schema and fixes the types. This may be revisited if silent
skipping turns out to hide too much, for example with an opt-in column that
collects unread content.

---

## 7. `--explain`

`xeibe scan <src> --explain` prints every field with its reason and evidence:

```
adres2        List(Utf8View)      list: max_occurs=3 (first at feature #1204, gml:id=PL.ZIPIN…, byte 91 233 812)
                                  item: by-reference only (xlink:href), '#' stripped
georeferencja geoarrow.point      geometry: kinds={Point}, dims={2}; encoding Auto → native point
                                  crs: "EPSG:2180" short form → x/y axis order (no swap)
kodPocztowy   Utf8View            numeric rejected: values like "22-120" are not numeric
dataNadania   Date32              present in 211 995 / 410 402 parents
lokalnyId     Utf8View            path idIIP/*/lokalnyId: type wrapper AD_IdentyfikatorIIP collapsed
```
