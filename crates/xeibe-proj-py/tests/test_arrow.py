"""xeibe_proj.arrow: GeoArrow arrays converted in their own buffers
(docs/pyproj-phase3.md, GeoArrow), checked against Transformer.transform on
the same coordinates."""

from __future__ import annotations

import json
import math
import struct
from typing import Any

import pytest

from xeibe_proj import CRS, Proj, Transformer, _native, arrow, datadir
from xeibe_proj.exceptions import ProjError

from .conftest import GRIDS, run_python

pa = pytest.importorskip("pyarrow")

# Easting first: GeoArrow's order.
POLAND_2180 = [(500000.0, 500000.0), (600000.0, 400000.0), (700000.0, 300000.0), (450000.0, 650000.0)]
POLAND_4979 = [(19.0, 52.0, 100.0), (20.5, 51.5, 250.0), (21.0, 50.0, -20.0), (17.0, 54.0, 0.0)]


class Field:
    """An array with a field of our own: GeoArrow's metadata without
    registering pyarrow extension types."""

    def __init__(self, array: Any, metadata: dict[str, str] | None = None, name: str = "geometry") -> None:
        self.array = array
        self.field = pa.field(name, array.type, metadata=metadata)

    def __arrow_c_array__(self, requested_schema: Any = None) -> tuple[object, object]:
        return self.field.__arrow_c_schema__(), self.array.__arrow_c_array__()[1]


def geoarrow(name: str, crs: Any = None, **extra: Any) -> dict[str, str]:
    meta: dict[str, Any] = dict(extra)
    if crs is not None:
        meta["crs"] = crs
    return {"ARROW:extension:name": name, "ARROW:extension:metadata": json.dumps(meta)}


def field_of(obj: Any) -> Any:
    return pa.field(obj)


def extension(obj: Any) -> dict[str, Any]:
    return dict(json.loads(field_of(obj).metadata[b"ARROW:extension:metadata"]))


# Building GeoArrow's native arrays -------------------------------------------------


def coords(points: list[tuple[float, ...]], dims: str, interleaved: bool) -> Any:
    n = len(dims)
    if interleaved:
        flat = pa.array([v for p in points for v in p], pa.float64())
        return pa.FixedSizeListArray.from_arrays(flat, type=pa.list_(pa.field(dims, pa.float64(), nullable=False), n))
    cols = [pa.array([p[i] for p in points], pa.float64()) for i in range(n)]
    return pa.StructArray.from_arrays(cols, fields=[pa.field(c, pa.float64(), nullable=False) for c in dims])


def nest(geoms: list[Any], depth: int, dims: str, interleaved: bool) -> Any:
    if depth == 0:
        return coords(geoms, dims, interleaved)
    offsets = [0]
    children: list[Any] = []
    for g in geoms:
        children.extend(g)
        offsets.append(len(children))
    return pa.ListArray.from_arrays(pa.array(offsets, pa.int32()), nest(children, depth - 1, dims, interleaved))


def leaf(array: Any, depth: int) -> Any:
    for _ in range(depth):
        array = array.flatten()
    return array


def read_coords(array: Any, dims: str) -> list[tuple[float, ...]]:
    if pa.types.is_fixed_size_list(array.type):
        flat = array.flatten().to_pylist()
        n = len(dims)
        return [tuple(flat[i : i + n]) for i in range(0, len(flat), n)]
    cols = [array.field(i).to_pylist() for i in range(len(dims))]
    return list(zip(*cols, strict=True))


def shape(points: list[tuple[float, ...]], depth: int) -> list[Any]:
    """The points grouped into ``depth`` levels of lists: two geometries
    of two parts each, …"""
    out: list[Any] = list(points)
    for _ in range(depth):
        out = [out[i : i + 2] for i in range(0, len(out), 2)]
    return out


def expected(t: Transformer, points: list[tuple[float, ...]], dims: str) -> list[tuple[float, ...]]:
    xs = [p[0] for p in points]
    ys = [p[1] for p in points]
    if "z" in dims:
        ox, oy, oz = t.transform(xs, ys, [p[2] for p in points])
    else:
        ox, oy = t.transform(xs, ys)
        oz = None
    out = []
    for k, p in enumerate(points):
        q = [ox[k], oy[k]]
        if oz is not None:
            q.append(oz[k])
        q.extend(p[len(q) :])  # M passes through
        out.append(tuple(q))
    return out


def sample(dims: str) -> tuple[Transformer, list[tuple[float, ...]]]:
    if "z" in dims:
        t = Transformer.from_crs(4979, 4978, always_xy=True)
        base = POLAND_4979
    else:
        t = Transformer.from_crs(2180, 4326, always_xy=True)
        base = [(*p, 0.0) for p in POLAND_2180]
    points = []
    for i, p in enumerate(base * 2):
        q = list(p[:2]) + ([p[2]] if "z" in dims else [])
        if "m" in dims:
            q.append(1000.0 + i)
        points.append(tuple(q))
    return t, points


NAMES = {0: "point", 1: "linestring", 2: "polygon", 3: "multipolygon"}


@pytest.mark.parametrize("dims", ["xy", "xyz", "xym", "xyzm"])
@pytest.mark.parametrize("interleaved", [True, False])
@pytest.mark.parametrize("depth", [0, 1, 2, 3])
def test_native_encodings(depth: int, interleaved: bool, dims: str) -> None:
    t, points = sample(dims)
    array = nest(shape(points, depth), depth, dims, interleaved)
    out = arrow.transform(t, array)
    assert isinstance(out, arrow.Array)
    result = pa.array(out)
    assert result.type == array.type
    assert len(result) == len(array)
    assert read_coords(leaf(result, depth), dims) == expected(t, points, dims)
    for level in range(depth):
        assert leaf(result, level).offsets == leaf(array, level).offsets


def test_large_lists_and_metadata() -> None:
    t, points = sample("xy")
    values = coords(points, "xy", True)
    array = pa.LargeListArray.from_arrays(pa.array([0, 3, 8], pa.int64()), values)
    out = arrow.transform(t, Field(array, geoarrow("geoarrow.linestring", "EPSG:2180", extra_key="kept")))
    assert pa.array(out).type == array.type
    assert read_coords(pa.array(out).flatten(), "xy") == expected(t, points, "xy")
    meta = extension(out)
    assert meta["crs_type"] == "projjson"
    assert CRS.from_json_dict(meta["crs"]) == CRS(4326)
    assert meta["extra_key"] == "kept"
    assert field_of(out).metadata[b"ARROW:extension:name"] == b"geoarrow.linestring"
    assert out.extension_name == "geoarrow.linestring"
    assert out.crs == CRS(4326)
    assert "geoarrow.linestring" in repr(out) and "EPSG:4326" in repr(out)


def test_without_metadata_stays_without() -> None:
    t, points = sample("xy")
    out = arrow.transform(t, coords(points, "xy", False))
    assert not field_of(out).metadata
    assert out.crs is None and out.extension_name is None


def test_arro3() -> None:
    a3 = pytest.importorskip("arro3.core")
    t, points = sample("xy")
    array = a3.Array.from_arrow(Field(nest(shape(points, 2), 2, "xy", False), geoarrow("geoarrow.polygon", "EPSG:2180")))
    out = a3.Array.from_arrow(arrow.transform(t, array))
    assert out.field.metadata_str["ARROW:extension:name"] == "geoarrow.polygon"
    assert "projjson" in out.field.metadata_str["ARROW:extension:metadata"]
    result = pa.array(out)
    assert read_coords(leaf(result, 2), "xy") == expected(t, points, "xy")
    # And to_crs from arro3's metadata.
    again = pa.array(arrow.to_crs(array, 4326))
    assert read_coords(leaf(again, 2), "xy") == expected(t, points, "xy")


# CRSs and directions -------------------------------------------------------------


def test_axis_order_is_always_east_first() -> None:
    t, points = sample("xy")
    authority = Transformer.from_crs(2180, 4326)
    a = pa.array(arrow.transform(t, coords(points, "xy", True)))
    b = pa.array(arrow.transform(authority, coords(points, "xy", True)))
    assert a.equals(b)
    # The transformer itself is unchanged.
    assert authority.transform(500000.0, 500000.0) != t.transform(500000.0, 500000.0)


def test_inverse_and_ident() -> None:
    t, points = sample("xy")
    there = arrow.transform(t, Field(coords(points, "xy", True), geoarrow("geoarrow.point", "EPSG:2180")))
    back = arrow.transform(t, there, direction="INVERSE")
    assert back.crs == CRS(2180)
    for p, q in zip(read_coords(pa.array(back), "xy"), points, strict=True):
        assert p == pytest.approx(q, abs=1e-6)
    same = arrow.transform(t, coords(points, "xy", True), direction="IDENT")
    assert read_coords(pa.array(same), "xy") == points


def test_crs_must_be_the_source() -> None:
    t, points = sample("xy")
    with pytest.raises(ValueError, match="not the transformer's source"):
        arrow.transform(t, Field(coords(points, "xy", True), geoarrow("geoarrow.point", "EPSG:4326")))
    # OGC:CRS84 is EPSG:4326 east first: the same CRS for GeoArrow.
    to_poland = Transformer.from_crs(4326, 2180)
    out = arrow.transform(to_poland, Field(coords([(19.0, 52.0)], "xy", True), geoarrow("geoarrow.point", "OGC:CRS84")))
    assert read_coords(pa.array(out), "xy") == [Transformer.from_crs(4326, 2180, always_xy=True).transform(19.0, 52.0)]


@pytest.mark.parametrize(
    ("crs", "crs_type"),
    [
        (CRS(2180).to_json_dict(), "projjson"),
        (CRS(2180).to_json(), "projjson"),
        (CRS(2180).to_wkt(), "wkt2:2019"),
        ("EPSG:2180", "authority_code"),
        ("EPSG:2180", None),
    ],
)
def test_to_crs_reads_every_crs_form(crs: Any, crs_type: str | None) -> None:
    t, points = sample("xy")
    extra = {"crs_type": crs_type} if crs_type else {}
    out = arrow.to_crs(Field(coords(points, "xy", True), geoarrow("geoarrow.point", crs, **extra)), "EPSG:4326")
    assert read_coords(pa.array(out), "xy") == expected(t, points, "xy")
    assert out.crs == CRS(4326)


def test_metadata_errors() -> None:
    _, points = sample("xy")
    array = coords(points, "xy", True)
    with pytest.raises(ValueError, match="no CRS"):
        arrow.to_crs(array, 4326)
    with pytest.raises(ValueError, match="no CRS"):
        arrow.to_crs(Field(array, geoarrow("geoarrow.point")), 4326)
    with pytest.raises(ValueError, match="SRID"):
        arrow.to_crs(Field(array, geoarrow("geoarrow.point", "2180", crs_type="srid")), 4326)
    with pytest.raises(ValueError, match="spherical edges"):
        arrow.to_crs(Field(array, geoarrow("geoarrow.point", "EPSG:2180", edges="spherical")), 4326)
    with pytest.raises(ValueError, match="geoarrow.wkt"):
        arrow.to_crs(Field(pa.array(["POINT (1 2)"]), geoarrow("geoarrow.wkt", "EPSG:2180")), 4326)
    with pytest.raises(ValueError, match="isn't a GeoArrow encoding"):
        arrow.to_crs(Field(array, geoarrow("geoarrow.geometry", "EPSG:2180")), 4326)
    with pytest.raises(ValueError, match="isn't JSON"):
        arrow.to_crs(Field(array, {"ARROW:extension:name": "geoarrow.point", "ARROW:extension:metadata": "{"}), 4326)


def test_layout_and_type_errors() -> None:
    t = Transformer.from_crs(2180, 4326)
    with pytest.raises(ValueError, match="WKT isn't supported"):
        arrow.transform(t, pa.array(["POINT (1 2)"]))
    with pytest.raises(ValueError, match="not a GeoArrow layout"):
        arrow.transform(t, pa.array([1, 2]))
    with pytest.raises(ValueError, match="neither coordinates"):
        arrow.transform(t, pa.StructArray.from_arrays([pa.array([1.0]), pa.array([2.0])], names=["a", "b"]))
    with pytest.raises(ValueError, match="not double"):
        arrow.transform(t, pa.FixedSizeListArray.from_arrays(pa.array([1, 2], pa.int64()), 2))
    with pytest.raises(TypeError, match="Arrow array"):
        arrow.transform(t, [1.0, 2.0])  # type: ignore[call-overload]
    with pytest.raises(TypeError, match="Transformer"):
        arrow.transform(Proj(2180), pa.array([b""]))


# Nulls, slices and failures ----------------------------------------------------


def test_nulls_and_garbage_under_them_are_left_alone() -> None:
    t, points = sample("xy")
    # A null point holding a coordinate that can't be converted.
    bad = (math.inf, 1e300)
    flat = pa.array([v for p in [points[0], bad, points[1]] for v in p], pa.float64())
    array = pa.FixedSizeListArray.from_arrays(flat, 2, mask=pa.array([False, True, False]))
    out = pa.array(arrow.transform(t, array))
    assert out.is_null().to_pylist() == [False, True, False]
    raw = out.values.to_pylist()  # flatten() would skip the null's values
    assert list(zip(raw[::2], raw[1::2], strict=True)) == [*expected(t, [points[0]], "xy"), bad, *expected(t, [points[1]], "xy")]
    # A null linestring over a bad coordinate, and a list that skips one.
    values = coords([points[0], bad, points[1]], "xy", False)
    lines = pa.ListArray.from_arrays(pa.array([0, 1, 2], pa.int32()), values, mask=pa.array([False, True]))
    out = pa.array(arrow.transform(t, lines))
    assert read_coords(out.values, "xy")[1] == bad


def test_sliced() -> None:
    t, points = sample("xy")
    lines = nest(shape(points, 1), 1, "xy", True)
    out = pa.array(arrow.transform(t, lines.slice(1, 2)))
    assert len(out) == 2
    assert read_coords(out.flatten(), "xy") == expected(t, points[2:6], "xy")
    pts = coords(points, "xy", False).slice(3, 2)
    assert read_coords(pa.array(arrow.transform(t, pts)), "xy") == expected(t, points[3:5], "xy")


def test_failure_names_the_geometry() -> None:
    t = Transformer.from_crs(4326, 3857, always_xy=True)
    with pytest.raises(ProjError) as direct:
        t.transform([10.0, 10.0], [0.0, 95.0])
    with pytest.raises(ProjError, match=r"^geometry 2: coordinate \(10, 95\)") as failed:
        arrow.transform(t, coords([(0.0, 0.0), (1.0, 1.0), (10.0, 95.0)], "xy", True))
    assert str(direct.value).split(": ")[-1] == str(failed.value).split(": ")[-1]
    chunks = [coords([(0.0, 0.0)] * 3, "xy", True), coords([(1.0, 1.0), (10.0, 95.0)], "xy", True)]
    with pytest.raises(ProjError, match="^geometry 4: "):
        arrow.transform(t, pa.chunked_array(chunks))
    with pytest.raises(ValueError, match="^geometry 1: WKB"):
        arrow.transform(t, pa.array([wkb(("Point", (1.0, 2.0))), b"\x01\x01\x00\x00\x00"]))


# Chunked arrays ------------------------------------------------------------------


def test_chunked() -> None:
    t, points = sample("xy")
    chunks = [nest(shape(points[:4], 1), 1, "xy", True), nest(shape(points[4:], 1), 1, "xy", True)]
    out = arrow.transform(t, pa.chunked_array(chunks))
    assert isinstance(out, arrow.ChunkedArray)
    assert out.num_chunks == 2 and len(out) == 4
    assert not hasattr(out, "__arrow_c_array__")
    result = pa.chunked_array(out)
    assert result.num_chunks == 2
    assert read_coords(result.combine_chunks().flatten(), "xy") == expected(t, points, "xy")
    empty = arrow.transform(t, pa.chunked_array([], type=chunks[0].type))
    assert len(empty) == 0 and pa.chunked_array(empty).num_chunks == 0


def test_chunked_metadata() -> None:
    # pyarrow's ChunkedArray has a type, not a field: arro3's keeps the
    # field's metadata through the stream.
    a3 = pytest.importorskip("arro3.core")
    t, points = sample("xy")
    array = nest(shape(points, 1), 1, "xy", True)
    field = pa.field("geometry", array.type, metadata=geoarrow("geoarrow.linestring", "EPSG:2180"))
    chunked = a3.ChunkedArray([array.slice(0, 2), array.slice(2, 2)], type=field)
    out = arrow.to_crs(chunked, 4326)
    assert isinstance(out, arrow.ChunkedArray)
    assert out.crs == CRS(4326) and out.extension_name == "geoarrow.linestring"
    back = a3.ChunkedArray.from_arrow(out)
    assert back.num_chunks == 2
    assert back.field.metadata_str["ARROW:extension:name"] == "geoarrow.linestring"
    assert read_coords(pa.chunked_array(back).combine_chunks().flatten(), "xy") == expected(t, points, "xy")


# WKB -----------------------------------------------------------------------------

KINDS = {
    "Point": 1,
    "LineString": 2,
    "Polygon": 3,
    "MultiPoint": 4,
    "MultiLineString": 5,
    "MultiPolygon": 6,
    "GeometryCollection": 7,
    "Triangle": 17,
}


def wkb(geom: tuple[str, Any], dims: str = "xy", *, le: bool = True, flavor: str = "iso", srid: int | None = None) -> bytes:
    """``geom`` as WKB: ``("Point", (x, y[, z][, m]))``, ``("LineString",
    [coords])``, ``("Polygon", [[coords]])``, multi-geometries and
    collections of such tuples. ``flavor`` is ``"iso"`` or ``"ewkb"``; only
    the outer geometry carries an EWKB SRID."""
    kind, body = geom
    order = "<" if le else ">"
    z, m = "z" in dims, "m" in dims
    code = KINDS[kind]
    if flavor == "iso":
        code += 1000 * z + 2000 * m
    else:
        code |= 0x80000000 * z | 0x40000000 * m | (0x20000000 if srid is not None else 0)
    out = bytes([le]) + struct.pack(order + "I", code)
    if flavor == "ewkb" and srid is not None:
        out += struct.pack(order + "I", srid)

    def points(ps: list[tuple[float, ...]]) -> bytes:
        return b"".join(struct.pack(order + "d" * len(dims), *p) for p in ps)

    if kind == "Point":
        return out + points([body])
    if kind == "LineString":
        return out + struct.pack(order + "I", len(body)) + points(body)
    if kind in ("Polygon", "Triangle"):
        out += struct.pack(order + "I", len(body))
        return out + b"".join(struct.pack(order + "I", len(r)) + points(r) for r in body)
    parts = b"".join(wkb(g, dims, le=le, flavor=flavor) for g in body)
    return out + struct.pack(order + "I", len(body)) + parts


def mapped(geom: tuple[str, Any], f: Any) -> tuple[str, Any]:
    kind, body = geom
    if kind == "Point":
        return kind, f(body)
    if kind == "LineString":
        return kind, [f(p) for p in body]
    if kind in ("Polygon", "Triangle"):
        return kind, [[f(p) for p in r] for r in body]
    return kind, [mapped(g, f) for g in body]


def converter(t: Transformer, dims: str) -> Any:
    def f(p: tuple[float, ...]) -> tuple[float, ...]:
        n = 3 if "z" in dims else 2
        if n == 3:
            q = t.transform(p[0], p[1], p[2])
        else:
            q = t.transform(p[0], p[1])
        return (*q, *p[n:])

    return f


def wkb_samples(dims: str) -> list[tuple[str, Any]]:
    _, points = sample(dims)
    a, b, c, d = points[:4]
    return [
        ("Point", a),
        ("LineString", [a, b, c]),
        ("Polygon", [[a, b, c, a], [b, c, d, b]]),
        ("Triangle", [[a, b, c, a]]),
        ("MultiPoint", [("Point", a), ("Point", d)]),
        ("MultiLineString", [("LineString", [a, b]), ("LineString", [c, d])]),
        ("MultiPolygon", [("Polygon", [[a, b, c, a]]), ("Polygon", [[b, c, d, b]])]),
        ("GeometryCollection", [("Point", a), ("GeometryCollection", [("LineString", [b, c])])]),
    ]


@pytest.mark.parametrize("storage", ["binary", "large_binary", "binary_view"])
@pytest.mark.parametrize("dims", ["xy", "xyz", "xym", "xyzm"])
@pytest.mark.parametrize(("le", "flavor", "srid"), [(True, "iso", None), (False, "iso", None), (True, "ewkb", 2180), (False, "ewkb", None)])
def test_wkb(storage: str, dims: str, le: bool, flavor: str, srid: int | None) -> None:
    t, _ = sample(dims)
    geoms = wkb_samples(dims)
    array = pa.array([*(wkb(g, dims, le=le, flavor=flavor, srid=srid) for g in geoms), None], getattr(pa, storage)())
    out = pa.array(arrow.transform(t, Field(array, geoarrow("geoarrow.wkb"))))
    assert out.type == array.type
    f = converter(t, dims)
    want = [wkb(mapped(g, f), dims, le=le, flavor=flavor, srid=srid) for g in geoms]
    assert out.to_pylist() == [*want, None]


def test_wkb_mixed_dimensions_and_empty() -> None:
    t = Transformer.from_crs(4979, 4978, always_xy=True)
    flat = wkb(("Point", (19.0, 52.0)))
    deep = wkb(("Point", (19.0, 52.0, 100.0)), "xyz")
    empty = wkb(("Point", (math.nan, math.nan)))
    nothing = struct.pack("<BII", 1, 7, 0)  # an empty collection: inline in a view
    for storage in (pa.binary(), pa.binary_view()):
        out = pa.array(arrow.transform(t, pa.array([flat, deep, empty, nothing], storage))).to_pylist()
        assert out[0] == wkb(("Point", t.transform(19.0, 52.0)))
        assert out[1] == wkb(("Point", t.transform(19.0, 52.0, 100.0)), "xyz")
        assert out[2] == empty and out[3] == nothing


@pytest.mark.parametrize(
    ("value", "message"),
    [
        (b"\x01\x01\x00\x00\x00\x00", "ends inside"),
        (b"\x02\x01\x00\x00\x00", "byte order 2"),
        (struct.pack("<BI", 1, 8) + b"\x00" * 4, "curved"),
        (struct.pack("<BI", 1, 99), "geometry type 99"),
        (wkb(("Point", (1.0, 2.0))) + b"\x00", "left over"),
        (struct.pack("<BII", 1, 2, 0xFFFFFFFF), "ends inside"),
    ],
)
def test_wkb_errors(value: bytes, message: str) -> None:
    t = Transformer.from_crs(2180, 4326)
    with pytest.raises(ValueError, match=message):
        arrow.transform(t, pa.array([value]))


# Boxes ---------------------------------------------------------------------------


def test_boxes() -> None:
    t = Transformer.from_crs(2180, 4326, always_xy=True)
    rows = [(400000.0, 300000.0, 600000.0, 500000.0), (500000.0, 500000.0, 700000.0, 700000.0)]
    names = ["xmin", "ymin", "xmax", "ymax"]
    array = pa.StructArray.from_arrays([pa.array([r[i] for r in rows] + [0.0]) for i in range(4)], names=names, mask=pa.array([False, False, True]))
    out = pa.array(arrow.transform(t, Field(array, geoarrow("geoarrow.box", "EPSG:2180"))))
    got = [tuple(out.field(n)[i].as_py() for n in names) for i in range(2)]
    assert got == [t.transform_bounds(*r) for r in rows]
    assert out.is_null().to_pylist() == [False, False, True]
    # Z bounds pass through.
    with_z = pa.StructArray.from_arrays(
        [pa.array([v]) for v in (*rows[0][:2], 5.0, *rows[0][2:], 9.0)], names=["xmin", "ymin", "zmin", "xmax", "ymax", "zmax"]
    )
    out = pa.array(arrow.transform(t, with_z))
    assert out.field("zmin").to_pylist() == [5.0] and out.field("zmax").to_pylist() == [9.0]
    assert out.field("xmin").to_pylist() == [t.transform_bounds(*rows[0])[0]]


# Grids and imports ---------------------------------------------------------------


def test_grids_are_loaded_for_the_extent(monkeypatch: pytest.MonkeyPatch) -> None:
    from .test_grids import _vectors

    _native.clear_grids()
    monkeypatch.setattr(datadir, "_user_dirs", None)
    datadir.set_data_dir(GRIDS)
    t = Transformer.from_crs(4138, 4269, always_xy=True)
    lats, lons, want = _vectors()
    pairs = [(lon, lat, w) for lat, lon, w in zip(lats, lons, want, strict=True) if w is not None]
    out = pa.array(arrow.transform(t, coords([(p[0], p[1]) for p in pairs], "xy", True)))
    for (_, _, w), q in zip(pairs, read_coords(out, "xy"), strict=True):
        # The vectors are latitude first.
        assert q == pytest.approx((w[1], w[0]), abs=1e-9)
    _native.clear_grids()


def test_imports_no_arrow_library() -> None:
    run_python(
        "import sys, xeibe_proj.arrow\n"
        "loaded = {'pyarrow', 'arro3', 'polars', 'nanoarrow', 'numpy'} & {m.partition('.')[0] for m in sys.modules}\n"
        "assert not loaded, loaded\n"
    )
