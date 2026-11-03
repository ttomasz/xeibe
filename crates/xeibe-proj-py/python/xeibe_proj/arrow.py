"""GeoArrow arrays converted between CRSs. Not in pyproj.

``transform`` and ``to_crs`` take anything with ``__arrow_c_array__`` or,
for chunked data, ``__arrow_c_stream__`` (pyarrow, arro3, polars, nanoarrow,
``xeibe.read_gml``'s columns) and give an ``Array`` or ``ChunkedArray`` that
any of those libraries reads back (``pyarrow.array(out)``,
``pyarrow.chunked_array(out)``). No Arrow library is imported.

The encodings are GeoArrow's native ones (points, linestrings, polygons and
their multi- forms, interleaved or separated, in XY, XYZ, XYM or XYZM; M
passes through), WKB (``Binary``, ``LargeBinary`` or ``BinaryView``, either
byte order, ISO or EWKB) and boxes, converted as ``transform_bounds``
converts one. The layout decides how an array is read, so arrays without
GeoArrow's extension metadata work too. Coordinates are always easting or
longitude first, as GeoArrow has them, whatever the transformer's
``always_xy``. Offsets and validity buffers are shared with the input; the
coordinates are new. The conversion runs without the GIL.

When the field has GeoArrow's extension metadata, its CRS must be the
transformer's source CRS (axis order aside), and the result's metadata keeps
its other keys and gets the target CRS as PROJJSON. Arrays with edges other
than planar raise ``ValueError``: a geodesic edge between two converted
points is not the converted edge. See ``docs/pyproj-phase3.md`` (GeoArrow).
"""

from __future__ import annotations

import json
from typing import TYPE_CHECKING, Any, Protocol, overload

from xeibe_proj import _native
from xeibe_proj.crs import CRS
from xeibe_proj.enums import TransformDirection
from xeibe_proj.proj import Proj
from xeibe_proj.transformer import Transformer

if TYPE_CHECKING:
    from xeibe_proj._typing import Direction

__all__ = ["Array", "ArrowArrayExportable", "ArrowStreamExportable", "ChunkedArray", "to_crs", "transform"]

_NAME = "ARROW:extension:name"
_METADATA = "ARROW:extension:metadata"

# GeoArrow's extension names this reads. The others (geoarrow.wkt,
# geoarrow.geometry, geoarrow.geometrycollection) raise.
_ENCODINGS = frozenset(
    {
        "geoarrow.point",
        "geoarrow.linestring",
        "geoarrow.polygon",
        "geoarrow.multipoint",
        "geoarrow.multilinestring",
        "geoarrow.multipolygon",
        "geoarrow.wkb",
        "geoarrow.box",
    }
)


class ArrowArrayExportable(Protocol):
    """An array with the Arrow PyCapsule interface."""

    def __arrow_c_array__(self, requested_schema: Any = None) -> tuple[object, object]: ...


class ArrowStreamExportable(Protocol):
    """A stream of arrays with the Arrow PyCapsule interface."""

    def __arrow_c_stream__(self, requested_schema: Any = None) -> object: ...


class _Converted:
    __slots__ = ("_data",)

    def __init__(self, data: _native.ArrowData) -> None:
        self._data = data

    def __len__(self) -> int:
        return len(self._data)

    def __arrow_c_schema__(self) -> object:
        return self._data.__arrow_c_schema__()

    def __arrow_c_stream__(self, requested_schema: Any = None) -> object:
        return self._data.__arrow_c_stream__(requested_schema)

    @property
    def extension_name(self) -> str | None:
        """GeoArrow's extension name, e.g. ``"geoarrow.wkb"``, or ``None``."""
        return self._data.metadata.get(_NAME)

    @property
    def crs(self) -> CRS | None:
        """The CRS in GeoArrow's extension metadata, or ``None``."""
        return _crs_of(_extension_metadata(self._data.metadata))

    def __repr__(self) -> str:
        crs = self.crs
        return (
            f"<xeibe_proj.arrow.{type(self).__name__}: {len(self)} geometries, "
            f"{self.extension_name or 'no extension type'}, "
            f"{crs.to_string() if crs is not None else 'no CRS'}>"
        )


class Array(_Converted):
    """A converted array, for ``pyarrow.array(out)`` or any other library
    that takes ``__arrow_c_array__``."""

    __slots__ = ()

    def __arrow_c_array__(self, requested_schema: Any = None) -> tuple[object, object]:
        return self._data.__arrow_c_array__(requested_schema)


class ChunkedArray(_Converted):
    """A converted chunked array, for ``pyarrow.chunked_array(out)`` or any
    other library that takes ``__arrow_c_stream__``. Each chunk is read once:
    export it once."""

    __slots__ = ()

    @property
    def num_chunks(self) -> int:
        return self._data.num_chunks


@overload
def transform(
    transformer: Transformer, array: ArrowArrayExportable, *, direction: Direction = ...
) -> Array: ...
@overload
def transform(
    transformer: Transformer, array: ArrowStreamExportable, *, direction: Direction = ...
) -> ChunkedArray: ...
def transform(
    transformer: Transformer,
    array: ArrowArrayExportable | ArrowStreamExportable,
    *,
    direction: Direction = TransformDirection.FORWARD,
) -> Array | ChunkedArray:
    """``array``'s geometries converted by ``transformer``, easting or
    longitude first whatever its ``always_xy``.

    A ``ValueError`` when the array's GeoArrow metadata names another CRS
    than the transformer's source, for edges that aren't planar, and for
    layouts that aren't GeoArrow's (WKT among them); a ``ProjError`` naming
    the geometry when a coordinate fails to convert."""
    # Checked at runtime too: a Proj has no CRSs to check the array's against.
    if isinstance(transformer, Proj) or not isinstance(transformer, Transformer):  # pyright: ignore[reportUnnecessaryIsInstance]
        raise TypeError("transform() takes a Transformer made by Transformer.from_crs")
    data, chunked = _import(array)
    data = _transform(transformer, data, TransformDirection.create(direction))
    return ChunkedArray(data) if chunked else Array(data)


@overload
def to_crs(array: ArrowArrayExportable, crs: Any) -> Array: ...
@overload
def to_crs(array: ArrowStreamExportable, crs: Any) -> ChunkedArray: ...
def to_crs(array: ArrowArrayExportable | ArrowStreamExportable, crs: Any) -> Array | ChunkedArray:
    """``array``'s geometries converted from the CRS in its GeoArrow
    metadata to ``crs`` (anything ``CRS.from_user_input`` takes). A
    ``ValueError`` when the array has no CRS: then use ``transform``."""
    data, chunked = _import(array)
    source = _crs_of(_extension_metadata(data.metadata))
    if source is None:
        raise ValueError("the array has no CRS in GeoArrow's extension metadata: use transform() with a Transformer")
    transformer = Transformer.from_crs(source, crs, always_xy=True)
    data = _transform(transformer, data, TransformDirection.FORWARD)
    return ChunkedArray(data) if chunked else Array(data)


# Internals --------------------------------------------------------------------


def _import(array: object) -> tuple[_native.ArrowData, bool]:
    """The native data, and whether it came in as a stream."""
    export_array = getattr(array, "__arrow_c_array__", None)
    if export_array is not None:
        schema, capsule = export_array()
        return _native.ArrowData.from_array(schema, capsule), False
    export_stream = getattr(array, "__arrow_c_stream__", None)
    if export_stream is not None:
        return _native.ArrowData.from_stream(export_stream()), True
    raise TypeError(
        f"expected an Arrow array (__arrow_c_array__ or __arrow_c_stream__), not {type(array).__name__}"
    )


def _extension_metadata(metadata: dict[str, str]) -> dict[str, Any] | None:
    """GeoArrow's extension metadata of a field, ``None`` without an
    extension name; a ``ValueError`` for what this doesn't read."""
    name = metadata.get(_NAME)
    if name is None:
        return None
    if name == "geoarrow.wkt":
        raise ValueError("geoarrow.wkt isn't supported: only GeoArrow's native encodings and WKB are")
    if name not in _ENCODINGS:
        raise ValueError(f"{name} isn't a GeoArrow encoding this converts")
    text = metadata.get(_METADATA) or "{}"
    try:
        extension = json.loads(text)
    except ValueError as e:
        raise ValueError(f"the GeoArrow metadata isn't JSON: {e}") from None
    if not isinstance(extension, dict):
        raise ValueError("the GeoArrow metadata isn't a JSON object")
    edges = extension.get("edges", "planar")  # pyright: ignore[reportUnknownMemberType, reportUnknownVariableType]
    if edges != "planar":
        raise ValueError(f"{edges} edges aren't converted: the edge between two converted points is another line")
    return extension  # pyright: ignore[reportUnknownVariableType]


def _crs_of(extension: dict[str, Any] | None) -> CRS | None:
    """The CRS GeoArrow's extension metadata names, or ``None``."""
    if extension is None:
        return None
    crs = extension.get("crs")
    if crs is None or crs == "":
        return None
    if extension.get("crs_type") == "srid":
        raise ValueError(f"the array's CRS is SRID {crs}, which names a CRS only in its producer's database")
    if isinstance(crs, dict):
        return CRS.from_json_dict(crs)  # pyright: ignore[reportUnknownArgumentType]
    if isinstance(crs, str) and crs.lstrip().startswith("{"):
        return CRS.from_json(crs)
    return CRS.from_user_input(crs)


def _transform(transformer: Transformer, data: _native.ArrowData, d: TransformDirection) -> _native.ArrowData:
    """``data`` converted in direction ``d``, its GeoArrow metadata checked
    and given the target CRS."""
    xy = transformer._east_first()  # pyright: ignore[reportPrivateUsage]
    native = xy._native_for(d)  # pyright: ignore[reportPrivateUsage]
    metadata = data.metadata
    extension = _extension_metadata(metadata)
    if native is None:  # TransformDirection.IDENT
        return data
    if not isinstance(native, _native.Transform):
        raise TypeError("transform() takes a Transformer made by Transformer.from_crs")
    forward = d is TransformDirection.FORWARD
    source, target = (xy.source_crs, xy.target_crs) if forward else (xy.target_crs, xy.source_crs)
    crs = _crs_of(extension)
    if crs is not None and not crs.equals(source, ignore_axis_order=True):
        raise ValueError(f"the array's CRS is {crs.to_string()}, not the transformer's source {source.to_string()}")
    converted = _convert(xy, native, data)
    if extension is None:
        return converted
    extension = {k: v for k, v in extension.items() if k not in ("crs", "crs_type")}
    extension["crs"] = target.to_json_dict()
    extension["crs_type"] = "projjson"
    metadata[_METADATA] = json.dumps(extension, separators=(",", ":"))
    return converted.with_metadata(metadata)


def _convert(transformer: Transformer, native: _native.Transform, data: _native.ArrowData) -> _native.ArrowData:
    """Convert, loading the grid data the coordinates' extent needs."""
    try:
        return data.convert(native)
    except _native.GridsNeeded as needed:
        transformer._fetch_grids(native, needed)  # pyright: ignore[reportPrivateUsage]
        return data.convert(native, False)
