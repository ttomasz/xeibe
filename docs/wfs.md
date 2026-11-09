# WFS client

`xeibe-wfs` is a **bulk-read** client: it fetches every feature of one feature type
from a WFS, with paging, and streams the pages into a normal read. It is not a
general WFS SDK, and it stores nothing: no saved pages, no manifest, no resume.

References (PDFs in `ogc_schemas/`):
- WFS 2.0.2 (OGC 09-025r2). §-numbers below refer to this document unless stated otherwise;
- WFS 1.1.0 (OGC 04-094);
- WFS 1.0.0 (OGC 02-058);
- OWS Common 2.0 (OGC 06-121r9);
- Filter Encoding 2.0.2 (OGC 09-026r2).

## Flow

```mermaid
flowchart TD
    caps["GetCapabilities"] -.-> capsout["version, KVP support, output formats,<br/>paging constraints, feature types, CRSs"]
    caps --> dft["DescribeFeatureType<br/>(no schema in the settings)"]
    dft -.-> dftout["declared properties:<br/>a hint for inference"]
    dft --> hits["GetFeature resultType=hits"]
    hits -.-> hitsout["numberMatched (1.1: numberOfFeatures)<br/>for progress"]
    hits --> pages["GetFeature pages"]
    pages --> mem["each page fetched into memory"] --> lazy["Sources::lazy"]
    lazy --> read["read (one layer)"] --> out(["Arrow / Parquet"])
```

- `WfsClient::new(endpoint, options)?.pages(type_name, progress)` returns the
  pages as a lazy sequence of sources (see
  [architecture.md](architecture.md#sources-and-remote-input)). `progress` is
  called after every page with the counts so far and any new warnings.
  `read` pulls the next page when it needs more features. A scan pulls them too.
- **A page is fetched completely into memory before it is parsed.** Pages are
  bounded by the page size (typically a few MB to tens of MB), and a whole page
  can be retried, or re-requested with half the page size after a truncated
  response, without emitting any feature twice.
- The schema is given (a settings file) or sampled from the first pages, like any
  other read. A scan followed by a read fetches every page twice. That is the
  user's choice.
- A failure that retries can't fix fails the read. There is no resume. The user
  reruns it.

HTTP (retries, backoff, auth) comes from `xeibe-io` and is shared with plain file
reads.

## Capabilities

What we read from `GetCapabilities`:

| Item | WFS 2.0 | WFS 1.1 | WFS 1.0 | Used for |
|---|---|---|---|---|
| Feature types | `FeatureTypeList/FeatureType`: `Name`, `Title`, `DefaultCRS`, `OtherCRS`, `OutputFormats`, `ows:WGS84BoundingBox` | same, but `DefaultSRS`/`OtherSRS` | `Name`, `Title`, `SRS`, `LatLongBoundingBox` | `xeibe wfs layers`, default CRS, bbox checks |
| KVP support | `KVPEncoding` constraint (Table 13) | assumed | assumed | Read, not used yet: requests are always `GET` (`POST` is planned, P2) |
| Paging support | `ImplementsResultPaging` (Table 13) | — | — | Paging strategy |
| Server page limit | `CountDefault` (Table 14) | — | — | Page size cap |
| Paging consistency | `PagingIsTransactionSafe` (Table 14, default FALSE) | — | — | Warns that duplicates or skipped features are possible |
| Sorting support | FES 2.0 `ImplementsSorting` (`fes:Filter_Capabilities/fes:Conformance`) | — | — | A warning when `--sort-by` is given and the server declares FALSE |
| Page cache lifetime | `ResponseCacheTimeout` (Table 14, seconds) | — | — | Read, not used yet. Resuming after `ResponseCacheExpired` doesn't depend on it |
| Output formats | `GetFeature` `outputFormat` parameter domain | same | `ResultFormat` (`GML2`) | Format negotiation |

## Schema hint from `DescribeFeatureType`

A WFS read without a settings file samples its schema from the first pages.
That leaves two gaps: a property null in every sampled feature becomes `text`,
and a property that never appears in the sample has no column, so it is not
read at all (the schema is the projection). The type's `DescribeFeatureType`
XSD fills them, **as a hint only**. `xeibe wfs convert` asks for it whenever
the settings file has no schema for the layer, after `GetCapabilities` and
before the first page (`WfsClient::describe`, then
`InferenceOptions::hint`).

- The request is `DescribeFeatureType` with the type name and namespace as
  `GetFeature` sends them (`TYPENAMES`/`NAMESPACES` in 2.0, `TYPENAME`/`NAMESPACE`
  in 1.1, `TYPENAME` in 1.0) and no `OUTPUTFORMAT`, so the server's default
  schema language: an XSD.
- Only the feature type's **direct properties** are read: the elements of its
  content model (named or inline complex type, `sequence`/`choice`/`all`), after
  those of a base type the same document defines (a `complexContent`
  extension). A simple type the document defines, inline or named, is
  followed to the built-in type it restricts. `maxOccurs > 1` on the element
  or on a group around it makes a list.
- Declared types become column types:

  | Declared type | Column |
  |---|---|
  | `xs:boolean` | `bool` |
  | `xs:integer`, `long`, `int`, `short`, `byte` and the other integer types | `int64` (`xs:unsignedLong`: `uint64`) |
  | `xs:double`, `float`, `decimal` | `double` |
  | `xs:date` | `date` |
  | `xs:time` | `time` in the timestamp unit |
  | `xs:dateTime` | `text`: a timestamp column takes values either with a time zone or without one, and the declaration doesn't say which, so a guess could fail the read |
  | other built-in simple types (`xs:string`, `token`, `anyURI`, `gYear`, …) | `text` |
  | GML geometry property types (`gml:PointPropertyType`, …, `gml:GeometryPropertyType`, GML 2 to 3.2) | geometry, WKB |
  | anything else (complex types, `gml:CodeType`, `gml:MeasureType`, `xs:anyType`) | ignored |

  A type the type options leave out (`types.enabled`, e.g. `--preset strings`)
  is `text`.
- It **types a sampled column that never had a value** with the declared type,
  and **adds a column** for a declared property the sample never saw, after the
  sampled columns in XSD order. The settings' overrides still apply to both:
  a dropped path stays out, a typed one takes the settings' type.
- It **never overrides evidence**: a column the sample typed keeps its type,
  whatever the XSD says. Declared types are not trusted over data.
- Anything else is ignored: nested complex types, references (`ref`, e.g.
  `gml:boundedBy`), substitution groups, wildcards (`xs:any`), groups, and
  imports: nothing is fetched beyond the response itself. A
  `DescribeFeatureType` that fails, is no schema, or doesn't declare the type
  is a warning, and the read goes on with the sample alone.
- The reasons of every column it typed or added name the XSD (what
  `--explain` prints). The settings a read can save keep the columns
  themselves, not the hint.

This is not XSD-driven mapping, which stays a non-goal
([architecture.md](architecture.md#non-goals-and-deliberate-limits)): the data
decides, and the XSD only fills what the data couldn't show.

## Paging

### What the standard says (WFS 2.0.2 §7.7.4.4)

- Paging happens only when the request sets `count`, **or** the server has a
  `CountDefault`.
- Each response may carry `next` and `previous` attributes: **URIs generated by the
  server**, whose format is up to the implementation. The last page has no `next`.
- The server may drop the cached result set after `ResponseCacheTimeout`. Following a
  `next` link after that returns a **`ResponseCacheExpired`** exception.
- `PagingIsTransactionSafe = FALSE`, the default, means data may change between
  pages: "it is possible to have a result set element be skipped or duplicated
  between iterations".
- `startIndex` (§7.6.3.4) is the index in the result set where the response starts.
  It is 0-based: the default is 0 (§7.6.3.2).
- `count` (§7.6.3.5) counts **only features of the requested types**. Nested
  features inside properties, and features referenced by xlink, are not counted.
  In a join, each tuple counts once.

### Strategies

Chosen automatically, or forced with `WfsOptions::strategy`. Before the first
request, a WFS 2.0 server with `ImplementsResultPaging = TRUE` gets `startIndex`
paging and any other server a single request; when the first response carries a
`next` link, the read follows `next` links from then on
(`xeibe_wfs::paging::choose`). Without a page size from the user or a
`CountDefault`, pages hold 5,000 features.

| Strategy | Condition | Notes |
|---|---|---|
| **`next` link** | WFS 2.0 response has `next` | Follow it until there is none. On `ResponseCacheExpired`, switch to `startIndex` at the number of features fetched so far, if supported. Otherwise the read fails |
| **`startIndex` + `count`** | WFS 2.0 with `ImplementsResultPaging = TRUE`, or forced | Page size = `min(user page size, CountDefault)`. Stops when `numberReturned < count` or the total reaches `numberMatched` |
| **Vendor `startIndex` on 1.x** | WFS 1.1/1.0 on servers known to support it (GeoServer, some MapServer) | `maxFeatures` + `startIndex`. Enabled with an explicit flag |
| **Spatial tiling** | No paging support | Split the bbox into tiles until each tile's `hits` count is below the server limit. Duplicates across tile edges are removed by `gml:id` within the run (🤔 Considering; forcing it is an `Unsupported` error) |
| **Single request** | Small layers, or forced | Warns if `numberReturned < numberMatched` |

### Stable ordering and duplicates

Offset-based paging is only correct when the order is stable. The standard itself
warns that pages may overlap or skip features without transactional consistency.

- **`SORTBY` is the user's choice** (`WfsOptions::sort_by`, `--sort-by`). Nothing
  is sorted unless it is given, and no property is guessed: an id that looks
  unique may not be, a property may not be sortable on the server (or slow to
  sort without an index), and `gml:id` is an attribute, not a property, so not every
  server can sort by it.
  A wrong guess would look safe while it isn't.
- The keys are `property [ASC|DESC]`, comma-separated, ascending by default
  (`name DESC,id`; 1.1's `A`/`D` are read too). They are written in each version's
  spelling:
  - 2.0: `SORTBY=prop [ASC|DESC],…` (Table 8);
  - 1.1: `SORTBY=prop [A|D],…` (04-094 Table for GetFeature KVP).
- **Every `GetFeature` of the read carries them**: the first request, whatever the
  paging strategy (a single request, `startIndex` pages, or the request whose
  response starts `next` links, which the server builds from it), and the
  `startIndex` pages after `ResponseCacheExpired`. Sending them on offset pages
  only would let the order change partway: a `next`-link read would never be
  sorted, and offsets after the switch would count positions in a different
  order than the pages before it. `resultType=hits` gets none.
- Warnings, never errors:
  - offset paging without `SORTBY`, once, when a second offset page is about to be
    requested: unless the server keeps one order across requests, features may be
    skipped or repeated. It suggests sorting by an identifier property and names
    none;
  - `SORTBY` given to a server that declares `ImplementsSorting = FALSE`: it may
    ignore it;
  - `SORTBY` given for WFS 1.0, which has no `SORTBY`: not sent;
  - `xeibe wfs convert`: a sort property the type's `DescribeFeatureType`
    schema doesn't declare (by the local name of its first step; `gml:` and
    `@` names, from base types outside the schema, are not checked). The server
    may reject the sort with an exception, which ends the read, or ignore it.
- `xeibe wfs convert` writes the rows in the order the server sends them
  (`ReadOptions::preserve_order`): pages are fetched one at a time, so keeping
  the order costs nothing.
- Always: check `gml:id` for duplicates across pages and **report** them. Each page
  is compared with the previous one. Removing duplicates (`WfsOptions::dedupe`)
  would need a set of ids and memory proportional to their count. It is not
  implemented and will be if the need appears; until then, setting it fails
  the read.
- A server that doesn't declare `PagingIsTransactionSafe = TRUE` gets a warning
  under offset paging.
- Compare `numberMatched` at the start and at the end. A changed count means the
  data changed during the read, and a warning is raised.

## Response structure

| Case | Structure | Handling |
|---|---|---|
| Single query (2.0) | `wfs:FeatureCollection` → `wfs:member` → feature (§11.3.3.4) | Normal |
| Several queries (2.0) | `wfs:FeatureCollection` → `wfs:member` → `wfs:FeatureCollection` per query, in query order. Inner collections may be inline **or referenced by `xlink:href`**. A feature matching several queries appears in one collection and is referenced from the others (§11.3.3.5) | Inner members become layers as usual; a layer spans every query's collection. Referenced members are counted and reported, not fetched |
| Join (2.0) | `wfs:member` → `wfs:Tuple` (§11.3.3.6) | Not supported |
| `resolve` used (2.0) | Referenced objects are placed in `wfs:additionalObjects`, and references are changed to point to them (§11.3.4) | Their features become layers of their own, like any member; the references hold the `@id` they point to (`#` stripped) |
| `GetFeatureById` stored query | The bare feature, without any collection element (§11.3.5) | "Single feature as document root" |
| `resultType=hits` | Empty collection with `numberMatched` (2.0), or `numberOfFeatures` (1.1) holding the **total** | Count only |
| WFS 1.1 results | `wfs:FeatureCollection` with `numberOfFeatures` = features **in this response** (04-094 §9.3) | Progress only. 1.1 has no total count in results |
| WFS 1.0 results | `wfs:FeatureCollection` → `gml:featureMember`, GML 2 | Normal |

`numberMatched` may be `"unknown"` (§7.7.4.2). If `count` isn't set and the server
has no `CountDefault`, `numberReturned` equals `numberMatched` (§7.7.4.3).

## Request encoding (KVP)

| Parameter | 2.0 | 1.1 | 1.0 | Notes |
|---|---|---|---|---|
| Service/version | `SERVICE=WFS&VERSION=2.0.2` (or `2.0.0`) | `1.1.0` | `1.0.0` | |
| Feature type | `TYPENAMES` | `TYPENAME` | `TYPENAME` | Mandatory unless `RESOURCEID` is used (Table 8) |
| Namespaces | `NAMESPACES=xmlns(prefix,uri)` | `NAMESPACE=xmlns(prefix=uri)` | — | Sent when the type name has a prefix. Some servers require it |
| Page size | `COUNT` | `MAXFEATURES` | `MAXFEATURES` | For 1.x, `MAXFEATURES` counts only requested types; a nested collection counts as one (04-094 §9.2) |
| Offset | `STARTINDEX` | vendor | vendor | |
| CRS | `SRSNAME` | `SRSNAME` | `SRSNAME` | Must be `DefaultCRS` or an `OtherCRS` of the type, otherwise `InvalidParameterValue`. WFS 1.1 servers must accept `EPSG:<code>`, `http://www.opengis.net/gml/srs/epsg.xml#<code>` and `urn:EPSG:geographicCRS:<code>` (04-094 §9.2) |
| Output format | `OUTPUTFORMAT` | `OUTPUTFORMAT` | `OUTPUTFORMAT` | Preference: `application/gml+xml; version=3.2` → `text/xml; subtype=gml/3.1.1` (1.1 default) → `text/xml; subtype=gml/2.1.2` / `GML2` |
| Bounding box | `BBOX=minx,miny,maxx,maxy[,crs]` | same | same | Values in **the CRS's axis order**, lower corner then upper corner (06-121r9 §10.2.3). Default CRS is the type's `DefaultCRS`. A single bbox applies to all queries |
| Filter | `FILTER` (FES 2.0 XML) | `FILTER` (Filter 1.1) | `FILTER` (Filter 1.0) | Passed through as-is. Mutually exclusive with `BBOX`/`RESOURCEID` |
| Properties | `PROPERTYNAME` | `PROPERTYNAME` | `PROPERTYNAME` | |
| Sorting | `SORTBY` | `SORTBY` | — | See above |
| Result type | `RESULTTYPE=hits` | `RESULTTYPE=hits` | — | |
| Resolution | `RESOLVE`, `RESOLVEDEPTH`, `RESOLVETIMEOUT` | `TRAVERSEXLINKDEPTH`, `TRAVERSEXLINKEXPIRY` | — | 🤔 Considering |
| Vendor | anything (`CQL_FILTER`, …) | same | same | `--param key=value` |

**Several queries in one KVP request** (2.0): each parameter's values are grouped in
parentheses, one group per query, and every query must use the same optional
parameters (Table 8 notes). We send one query per request instead. It's simpler,
and paging then applies to one type at a time. Multi-query responses from other
tools are still read correctly.

`POST` with an XML body is planned (P2) for filters too long for a URL.

## Errors and robustness

| Condition | Detection | Action |
|---|---|---|
| OWS exception | `ows:ExceptionReport` (1.1/2.0) or `ServiceExceptionReport` (1.0), **also inside HTTP 200 responses** | Error with `exceptionCode`, `locator` and the message |
| `ResponseCacheExpired` | exception code | Resume via `startIndex` (see above) |
| `OperationProcessingFailed` / `OperationParsingFailed` | exception code | No retry. Report the request |
| Truncated response | `wfs:truncatedResponse` (2.0), or no closing collection element | Retry with half the page size (up to N times) |
| HTTP 5xx, 429, network errors | status | Exponential backoff. Respects `Retry-After` |
| Changed `numberMatched` | start vs. end | Warning |
| Duplicate `gml:id` | compared with the previous page | Reported. Removal is planned |

Also:
- gzip/deflate `Content-Encoding`.
- `WfsOptions::concurrency` (default 1) is the number of pages held at once:
  the one being read plus the ones fetched ahead of it, in order. They are
  fetched one request at a time. Parallel requests (possible only with
  `startIndex` paging; `next` links can only be followed one after another) and
  a rate limit are not planned for now.
- Authentication: HTTP basic, bearer token, custom headers (`WfsOptions::auth`,
  `headers`; library only, the CLI has no flags for them yet).

## Out of scope

- WFS-T (Transaction), LockFeature, GetFeatureWithLock.
- GetPropertyValue.
- Stored-query management (Create/Drop). `GetFeatureById`, `ListStoredQueries` and
  `DescribeStoredQueries` may come later.
- Joins (`wfs:Tuple`).
- `previous` links (not needed to page forward).
- OGC API – Features (WFS 3). It is GeoJSON/JSON-based and belongs in a different
  tool. Its GML output encoding is not planned.
