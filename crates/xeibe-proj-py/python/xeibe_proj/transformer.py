"""``Transformer``: converts coordinates from one CRS to another.

Differences from pyproj (``docs/pyproj-compat.md``):

- A coordinate that can't be converted raises ``ProjError``, never ``inf``.
  ``errcheck`` and ``inplace`` don't exist; passing either raises
  ``NotImplementedError``. ``direction`` (and the other options after
  pyproj's ``errcheck``) is keyword-only.
- There are no ballpark transformations: without a usable EPSG operation,
  ``from_crs`` raises ``ProjError``.
- Grids are fetched by default and every use is logged (``xeibe_proj.grids``).
"""

from __future__ import annotations

import json
import math
import threading
import warnings
from array import array
from collections.abc import Iterable, Iterator, Sequence
from itertools import chain, islice
from typing import TYPE_CHECKING, Any, overload

from xeibe_proj import _containers, _grids, _native
from xeibe_proj._epsg import OGC_CRS
from xeibe_proj._json import as_dict, get_dict, get_dicts, ident
from xeibe_proj._unsupported import not_yet, out
from xeibe_proj.aoi import AreaOfInterest, AreaOfUse
from xeibe_proj.crs import CRS, CoordinateOperation, Grid
from xeibe_proj.enums import TransformDirection
from xeibe_proj.exceptions import ProjError
from xeibe_proj.network import is_network_enabled

if TYPE_CHECKING:
    from xeibe_proj._typing import ArrayLike, Coordinates, Direction, FloatArray

__all__ = ["AreaOfInterest", "Transformer", "TransformerGroup", "itransform", "transform"]

_FORWARD = TransformDirection.FORWARD


def _reject(method: str, args: tuple[Any, ...], kwargs: dict[str, Any]) -> None:
    """Raise for ``errcheck`` and ``inplace``, by keyword or in pyproj's
    positions (an extra positional argument is where pyproj takes
    ``errcheck``), and for any other unexpected argument."""
    unsupported = [n for n in kwargs if n in ("errcheck", "inplace")]
    if args or unsupported:
        raise out(
            f"{method}({', '.join(unsupported or ['errcheck'])}=…)",
            "a coordinate that can't be converted always raises ProjError, and "
            "results are always new objects; direction is keyword-only",
        )
    if kwargs:
        raise TypeError(f"{method}() got unexpected arguments: {', '.join(kwargs)}")


def _degrees(crs: CRS) -> bool:
    """Whether radians given for this CRS are converted: a geographic CRS
    (or compound with one) whose horizontal axes are in degrees."""
    if not crs.is_geographic:
        return False
    axes = crs.axis_info[:2]
    return bool(axes) and all(a.unit_name == "degree" for a in axes)


def _meridian(axis: dict[str, Any]) -> float | None:
    """The longitude, in degrees, of the meridian a polar axis runs along."""
    meridian = as_dict(axis.get("meridian"))
    if meridian is None:
        return None
    longitude = meridian.get("longitude")
    value = as_dict(longitude)
    return float(value["value"] if value is not None else longitude)  # type: ignore[arg-type]


def _swapped_for_xy(crs: CRS) -> bool:
    """Whether ``always_xy`` swaps the first two values of this CRS: PROJ's
    ``mustAxisOrderBeSwitchedForVisualization`` (src/iso19111/crs.cpp).
    Northing-easting axes are swapped, and of the polar CRSs with both axes
    along meridians only UPS North and South (N,E) and RSPS2000. So
    ``xeibe-reproject``'s own east-first order, which swaps whenever the
    first axis points north, isn't used for this."""
    if crs.is_compound:
        components = crs.sub_crs_list
        return bool(components) and _swapped_for_xy(components[0])
    if not (crs.is_geographic or crs.is_projected):
        return False
    axes = get_dicts(get_dict(crs.to_json_dict(), "coordinate_system"), "axis")
    if len(axes) < 2:
        return False
    d0, d1 = (str(a.get("direction", "")).lower() for a in axes[:2])
    if (d0, d1) == ("north", "east"):
        return True
    m0, m1 = _meridian(axes[0]), _meridian(axes[1])
    if m0 is None or m1 is None:
        return False
    if (d0, d1) == ("south", "south"):
        return abs(m0 - 180) < 1e-10 and abs(m1 - 90) < 1e-10
    if (d0, d1) == ("north", "north"):
        return (abs(m0) < 1e-10 and abs(m1 - 90) < 1e-10) or (abs(m0 - 180) < 1e-10 and abs(m1 + 90) < 1e-10)
    return False


def _native_json(crs: CRS) -> str:
    """What Rust reads the CRS from: an OGC CRS as its EPSG twin with its
    own axes, so that EPSG's transformations apply to it, and a CRS made
    from WKT as that WKT (whose WKT1 axes may be unstated)."""
    projjson = crs.to_json_dict()
    auth, code = ident(projjson) or ("", "")
    twin = OGC_CRS.get(code.upper()) if auth == "OGC" else None
    if twin is not None:
        projjson["id"] = {"authority": "EPSG", "code": twin[0]}
        return json.dumps(projjson)
    return crs._native_input()  # pyright: ignore[reportPrivateUsage]


def _last_value(buffer: Any) -> float | None:
    """The last value of a C-contiguous float64 buffer, or ``None``."""
    view = memoryview(buffer).cast("B").cast("d")
    return float(view[-1]) if len(view) else None


def _operation(info: _native.OperationInfo, available: bool) -> CoordinateOperation:
    """A candidate operation as pyproj describes one."""
    projjson: dict[str, Any] = {
        "type": "Transformation" if len(info.steps) <= 1 else "ConcatenatedOperation",
        "name": info.name,
    }
    if info.accuracy is not None:
        projjson["accuracy"] = str(info.accuracy)
    if info.scope is not None:
        projjson["scope"] = info.scope
    if info.code is not None:
        remarks = _native.operation_remarks(info.code[0])
        if remarks is not None:
            projjson["remarks"] = remarks
        if not info.code[1]:
            projjson["id"] = {"authority": "EPSG", "code": info.code[0]}
    area = None if info.area is None else AreaOfUse(*info.area, name=info.area_name)
    grids = [
        Grid(
            short_name=g,
            full_name="",
            package_name="",
            url=_grids.CDN + g,
            direct_download=True,
            open_license=True,
            available=available,
        )
        for g in info.grids
    ]
    return CoordinateOperation(projjson, area, grids)


def _available(info: _native.OperationInfo, network: bool, local: set[str]) -> bool:
    """Whether an operation's grids can be had: from the CDN with the
    network on, else from the data directories."""
    return network or all(g in local for g in info.grids)


class Transformer:
    """A conversion from one CRS to another, made with ``Transformer.from_crs``."""

    _source: CRS
    _target: CRS
    _always_xy: bool
    _network: bool
    # Whether always_xy swaps the source's and the target's first two values.
    _swap: tuple[bool, bool]
    # Both in the CRSs' own axis order: always_xy is applied in Python.
    _forward: _native.Transform
    _backward: _native.Transform | None
    # The directions (True: forward) whose grid use has been logged.
    _logged: set[bool]
    # For a transformer through one operation (a TransformerGroup member,
    # get_last_used_operation): that operation's name.
    _member: str | None
    # Per thread: the last coordinate converted, for get_last_used_operation.
    _last: threading.local

    def __init__(self, *args: Any, **kwargs: Any) -> None:
        raise ProjError("use Transformer.from_crs() to make a Transformer")

    @classmethod
    def _make(cls, source: CRS, target: CRS, always_xy: bool, member: str | None = None) -> Transformer:
        """The transformer between two CRSs: choosing among every candidate,
        or with ``member`` through the candidate of that name only."""
        network = is_network_enabled()
        if member is None:
            # With the network off, the grids in the data directories.
            local = None if network else _grids.local_files()
            native = _native.Transform(_native_json(source), _native_json(target), False, network, local)
        else:
            everything = _native.Transform(_native_json(source), _native_json(target), False, True)
            names = [o.name for o in everything.operation_info()]
            only = everything.with_operation_index(names.index(member)) if member in names else None
            if only is None:
                raise ProjError(f"no operation {member!r} from {source.to_string()} to {target.to_string()}")
            native = only
        return cls._from_native(source, target, always_xy, native, member, network)

    @classmethod
    def _from_native(
        cls,
        source: CRS,
        target: CRS,
        always_xy: bool,
        native: _native.Transform,
        member: str | None,
        network: bool,
    ) -> Transformer:
        self = cls.__new__(cls)
        self._source = source
        self._target = target
        self._always_xy = always_xy
        self._network = network
        self._swap = (_swapped_for_xy(source), _swapped_for_xy(target)) if always_xy else (False, False)
        self._forward = native
        self._member = member
        self._backward = None
        self._logged = set()
        self._last = threading.local()
        return self

    @staticmethod
    def from_crs(
        crs_from: Any,
        crs_to: Any,
        always_xy: bool = False,
        area_of_interest: AreaOfInterest | None = None,
        authority: str | None = None,
        accuracy: float | None = None,
        allow_ballpark: bool | None = None,
        force_over: bool = False,
        only_best: bool | None = None,
    ) -> Transformer:
        """The transformer from ``crs_from`` to ``crs_to`` (anything
        ``CRS.from_user_input`` takes).

        ``always_xy=True`` takes and returns longitude or easting first.
        Between datums, EPSG's operations are chosen per coordinate as PROJ
        chooses them. ``allow_ballpark`` is accepted, but no ballpark
        transformation is ever used: without a usable EPSG operation this
        raises ``ProjError``.
        """
        if area_of_interest is not None or accuracy is not None or only_best is not None:
            raise not_yet("from_crs(area_of_interest=, accuracy=, only_best=)", 2)
        if authority is not None and authority.upper() not in ("EPSG", "ANY"):
            raise out(f"from_crs(authority={authority!r})", "only EPSG's operations are known")
        if force_over:
            raise out("from_crs(force_over=True)", "it is a PROJ pipeline option")
        return Transformer._make(CRS.from_user_input(crs_from), CRS.from_user_input(crs_to), always_xy)

    @staticmethod
    def from_proj(proj_from: Any, proj_to: Any, always_xy: bool = False, area_of_interest: AreaOfInterest | None = None) -> Transformer:
        raise out("Transformer.from_proj", "pyproj deprecated it; use Transformer.from_crs")

    @staticmethod
    def from_pipeline(proj_pipeline: str, always_xy: bool = False) -> Transformer:
        raise out("Transformer.from_pipeline", "PROJ pipelines need PROJ")

    # Properties ---------------------------------------------------------------

    @property
    def source_crs(self) -> CRS:
        return self._source

    @property
    def target_crs(self) -> CRS:
        return self._target

    @property
    def is_network_enabled(self) -> bool:
        """Whether grids may be fetched, as when the transformer was made."""
        return self._network

    @property
    def name(self) -> str:
        """``"noop"`` within one CRS, ``"conversion"`` on one datum, else ``"transformation"``."""
        if self._source == self._target:
            return "noop"
        return "transformation" if self._forward.operations() else "conversion"

    @property
    def description(self) -> str:
        """The best candidate operation's name, or the conversion between the CRSs."""
        operations = self._forward.operations()
        if operations:
            return operations[0]
        return f"Conversion from {self._source.name} to {self._target.name}"

    @property
    def has_inverse(self) -> bool:
        return True

    @property
    def accuracy(self) -> float:
        """In metres: the operation's, 0 for a conversion on one datum, and
        -1 when unknown or when the operation is chosen per coordinate
        among several, as PROJ reports."""
        infos = self._forward.operation_info()
        if not infos:
            return 0.0
        if len(infos) == 1 and infos[0].accuracy is not None:
            return infos[0].accuracy
        return -1.0

    @property
    def area_of_use(self) -> AreaOfUse | None:
        """The operation's area of use; for a conversion on one datum, where
        both CRSs' areas meet; ``None`` when chosen per coordinate."""
        infos = self._forward.operation_info()
        if len(infos) == 1:
            area = infos[0].area
            return None if area is None else AreaOfUse(*area, name=infos[0].area_name)
        if infos:
            return None
        a, b = self._source.area_of_use, self._target.area_of_use
        if a is None or b is None:
            return a or b
        return AreaOfUse(max(a.west, b.west), max(a.south, b.south), min(a.east, b.east), min(a.north, b.north))

    def _single(self) -> _native.OperationInfo | None:
        """The operation, when there is exactly one."""
        infos = self._forward.operation_info()
        return infos[0] if len(infos) == 1 else None

    @property
    def remarks(self) -> str | None:
        """EPSG's remarks on the operation, when it is one EPSG operation."""
        info = self._single()
        if info is None or info.code is None:
            return None
        return _native.operation_remarks(info.code[0])

    @property
    def scope(self) -> str | None:
        """EPSG's scope of the operation, when it is one EPSG operation."""
        info = self._single()
        return None if info is None else info.scope

    @property
    def operations(self) -> tuple[CoordinateOperation, ...] | None:
        """The operation, when there is one: none on one datum or when it is
        chosen per coordinate. pyproj lists a concatenated operation's
        steps; this gives the operation itself."""
        infos = self._forward.operation_info()
        if len(infos) != 1:
            return ()
        return (_operation(infos[0], True),)

    def get_last_used_operation(self) -> Transformer:
        """The transformer through the operation the last coordinate
        converted in this thread went through."""
        last = getattr(self._last, "value", None)
        if last is None:
            raise ProjError("Last used operation not found. This is likely due to not initiating a transform.")
        native, forward, x, y, z = last
        source, target = (self._source, self._target) if forward else (self._target, self._source)
        if not native.operation_info():
            return self if forward else Transformer._make(source, target, self._always_xy)
        index = native.operation_index_for(x, y, z)
        only = None if index is None else native.with_operation_index(index)
        if index is None or only is None:
            raise ProjError("the last coordinate went through no operation")
        name = native.operation_info()[index].name
        return Transformer._from_native(source, target, self._always_xy, only, name, self._network)

    @property
    def definition(self) -> str:
        raise out("Transformer.definition", "it is a PROJ pipeline string; see Transformer.description")

    def to_proj4(self, version: Any = 5, pretty: bool = False) -> str:
        raise out("Transformer.to_proj4", "it is a PROJ pipeline string")

    def to_wkt(self, version: Any = "WKT2_2019", pretty: bool = False) -> str:
        raise not_yet("Transformer.to_wkt", 2)

    def to_json(self, pretty: bool = False, indentation: int = 2) -> str:
        raise not_yet("Transformer.to_json", 2)

    def to_json_dict(self) -> dict[str, Any]:
        raise not_yet("Transformer.to_json_dict", 2)

    # Conversion ---------------------------------------------------------------

    def _plan(self, direction: Direction) -> tuple[_native.Transform | None, CRS, CRS, tuple[bool, bool]]:
        """The native transform for ``direction`` (``None`` for IDENT), the
        CRSs coordinates come from and go to, and whether always_xy swaps
        the values going in and coming out."""
        d = TransformDirection.create(direction)
        if d is TransformDirection.IDENT:
            return None, self._source, self._source, (False, False)
        if d is TransformDirection.FORWARD:
            return self._forward, self._source, self._target, self._swap
        if self._backward is None:
            self._backward = self._forward.inverse()
        return self._backward, self._target, self._source, (self._swap[1], self._swap[0])

    def _load_grids(self, native: _native.Transform, forward: bool, bounds: tuple[float, float, float, float] | None) -> None:
        files = native.grids()
        if not files:
            return
        if forward not in self._logged:
            self._logged.add(forward)
            source, target = (self._source, self._target) if forward else (self._target, self._source)
            _grids.LOGGER.info(
                "%s to %s may use grids: %s",
                source.to_string(),
                target.to_string(),
                ", ".join(files),
                extra={"grids": files, "source": source.to_string(), "target": target.to_string()},
            )
        _grids.load(native, bounds)

    def _convert(
        self,
        native: _native.Transform,
        forward: bool,
        x: Any,
        y: Any,
        z: Any,
        radians_in: bool,
        radians_out: bool,
        swap: tuple[bool, bool],
    ) -> tuple[Any, Any]:
        """Convert in place; the buffers holding the first and second values
        out. A swap only exchanges which buffer is which."""
        if swap[0]:
            x, y = y, x
        extent = _native.extent(x, y)
        if extent is not None and radians_in:
            extent = (
                math.degrees(extent[0]),
                math.degrees(extent[1]),
                math.degrees(extent[2]),
                math.degrees(extent[3]),
            )
        self._load_grids(native, forward, extent)
        last = _last_value(x), _last_value(y), None if z is None else _last_value(z)
        native.convert(x, y, z, radians_in, radians_out)
        if last[0] is not None and last[1] is not None:
            # In degrees, as the coordinates went in.
            scale = (180 / math.pi) if radians_in else 1.0
            self._last.value = (native, forward, last[0] * scale, last[1] * scale, last[2])
        return (y, x) if swap[1] else (x, y)

    @overload
    def transform(
        self,
        xx: Coordinates,
        yy: Coordinates,
        zz: None = None,
        tt: None = None,
        radians: bool = False,
        *,
        direction: Direction = _FORWARD,
    ) -> tuple[Coordinates, Coordinates]: ...
    @overload
    def transform(
        self,
        xx: Coordinates,
        yy: Coordinates,
        zz: Coordinates,
        tt: None = None,
        radians: bool = False,
        *,
        direction: Direction = _FORWARD,
    ) -> tuple[Coordinates, Coordinates, Coordinates]: ...
    @overload
    def transform(
        self,
        xx: Coordinates,
        yy: Coordinates,
        zz: Coordinates,
        tt: Coordinates,
        radians: bool = False,
        *,
        direction: Direction = _FORWARD,
    ) -> tuple[Coordinates, Coordinates, Coordinates, Coordinates]: ...
    @overload
    def transform(
        self,
        xx: ArrayLike,
        yy: ArrayLike,
        zz: None = None,
        tt: None = None,
        radians: bool = False,
        *,
        direction: Direction = _FORWARD,
    ) -> tuple[FloatArray, FloatArray]: ...
    @overload
    def transform(
        self,
        xx: ArrayLike,
        yy: ArrayLike,
        zz: ArrayLike,
        tt: ArrayLike | None = None,
        radians: bool = False,
        *,
        direction: Direction = _FORWARD,
    ) -> tuple[FloatArray, ...]: ...
    def transform(
        self,
        xx: Any,
        yy: Any,
        zz: Any = None,
        tt: Any = None,
        radians: bool = False,
        *args: Any,
        direction: Direction = _FORWARD,
        **kwargs: Any,
    ) -> tuple[Any, ...]:
        """Convert coordinates: scalars, lists, tuples, ``array.array``,
        numpy arrays, or anything with ``__array__`` (which gives numpy
        arrays back). ``zz`` is the height or third axis; ``tt`` passes
        through unchanged. ``radians=True`` takes and returns geographic
        coordinates in radians. Raises ``ProjError`` at the first coordinate
        that can't be converted."""
        if args or kwargs:
            _reject("transform", args, kwargs)
        native, source, target, swap = self._plan(direction)
        bx, kx = _containers.to_buffer(xx)
        by, ky = _containers.to_buffer(yy)
        bz = kz = None
        if zz is not None:
            bz, kz = _containers.to_buffer(zz)
        if native is not None:
            forward = TransformDirection.create(direction) is TransformDirection.FORWARD
            bx, by = self._convert(
                native, forward, bx, by, bz, radians and _degrees(source), radians and _degrees(target), swap
            )
        result: tuple[Any, ...] = (_containers.from_buffer(kx, bx), _containers.from_buffer(ky, by))
        if bz is not None and kz is not None:
            result += (_containers.from_buffer(kz, bz),)
        if tt is not None:
            bt, kt = _containers.to_buffer(tt)
            result += (_containers.from_buffer(kt, bt),)
        return result

    def itransform(
        self,
        points: Iterable[Sequence[float]],
        switch: bool = False,
        time_3rd: bool = False,
        radians: bool = False,
        *args: Any,
        direction: Direction = _FORWARD,
        **kwargs: Any,
    ) -> Iterator[tuple[float, ...]]:
        """Convert points given as tuples of 2 to 4 values, 64 at a time.
        ``switch`` swaps the first two values in and out; ``time_3rd`` reads
        a third value as a time, which passes through."""
        if args or kwargs:
            _reject("itransform", args, kwargs)
        native, source, target, swap = self._plan(direction)
        forward = TransformDirection.create(direction) is TransformDirection.FORWARD
        it = iter(points)
        try:
            first = next(it)
        except StopIteration:
            raise ValueError("iterable must contain at least one point") from None
        stride = len(first)
        if stride not in (2, 3, 4):
            raise ValueError("points can contain up to 4 coordinates")
        if time_3rd and stride != 3:
            raise ValueError("'time_3rd' is only valid for 3 coordinates.")
        has_z = stride >= 3 and not time_3rd
        values = chain(first, (p[i] for p in it for i in range(stride)))
        while True:
            chunk = array("d", islice(values, 64 * stride))
            if not chunk:
                return
            a, b = (1, 0) if switch else (0, 1)
            x, y = chunk[a::stride], chunk[b::stride]
            z = chunk[2::stride] if has_z else None
            if native is not None:
                x, y = self._convert(
                    native, forward, x, y, z, radians and _degrees(source), radians and _degrees(target), swap
                )
            chunk[a::stride], chunk[b::stride] = x, y
            if z is not None:
                chunk[2::stride] = z
            yield from zip(*([iter(chunk)] * stride))

    def transform_bounds(
        self,
        left: float,
        bottom: float,
        right: float,
        top: float,
        densify_pts: int = 21,
        radians: bool = False,
        *args: Any,
        direction: Direction = _FORWARD,
        **kwargs: Any,
    ) -> tuple[float, float, float, float]:
        """The box that holds a box's converted, densified edges. A
        geographic result with ``right < left`` crosses the antimeridian."""
        if args or kwargs:
            _reject("transform_bounds", args, kwargs)
        if densify_pts < 0:
            raise ProjError("densify_pts must be positive")
        native, source, target, swap = self._plan(direction)
        bounds = (float(left), float(bottom), float(right), float(top))
        if native is None:
            return bounds
        if swap[0]:
            bounds = (bounds[1], bounds[0], bounds[3], bounds[2])
        if radians and _degrees(source):
            bounds = (math.degrees(bounds[0]), math.degrees(bounds[1]), math.degrees(bounds[2]), math.degrees(bounds[3]))
        forward = TransformDirection.create(direction) is TransformDirection.FORWARD
        self._load_grids(native, forward, bounds)
        result = native.convert_bounds(bounds, densify_pts)
        if radians and _degrees(target):
            result = (math.radians(result[0]), math.radians(result[1]), math.radians(result[2]), math.radians(result[3]))
        if swap[1]:
            result = (result[1], result[0], result[3], result[2])
        return result

    # Identity -----------------------------------------------------------------

    def __eq__(self, other: object) -> bool:
        return (
            isinstance(other, Transformer)
            and self._source == other._source
            and self._target == other._target
            and self._always_xy == other._always_xy
        )

    def __hash__(self) -> int:
        return hash((self._source, self._target, self._always_xy))

    def is_exact_same(self, other: Any) -> bool:
        return (
            isinstance(other, Transformer)
            and self._source.is_exact_same(other._source)
            and self._target.is_exact_same(other._target)
            and self._always_xy == other._always_xy
        )

    def __reduce__(self) -> tuple[Any, tuple[CRS, CRS, bool, str | None]]:
        return Transformer._make, (self._source, self._target, self._always_xy, self._member)

    def __str__(self) -> str:
        return self.description

    def __repr__(self) -> str:
        operations = self._forward.operations()
        listed = "".join(f"- {o}\n" for o in operations) or "- none: on one datum\n"
        return (
            f"<Transformer: {self._source.to_string()} to {self._target.to_string()}>\n"
            f"Description: {self.description}\n"
            f"Candidate operations:\n{listed}"
        )


class TransformerGroup:
    """Every operation between two CRSs, best first: a transformer for each
    whose grids can be had, and the others as unavailable operations.

    The options PROJ uses to widen or narrow the search
    (``area_of_interest``, ``accuracy``, ``crs_extent_use``, ``pivot_crs``,
    ``grid_check``) are accepted at their defaults only.
    """

    def __init__(
        self,
        crs_from: Any,
        crs_to: Any,
        always_xy: bool = False,
        area_of_interest: AreaOfInterest | None = None,
        authority: str | None = None,
        accuracy: float | None = None,
        allow_ballpark: bool = True,
        allow_superseded: bool = False,
        crs_extent_use: Any = None,
        pivot_crs: Any = None,
        grid_check: Any = None,
    ) -> None:
        if area_of_interest is not None or accuracy is not None:
            raise not_yet("TransformerGroup(area_of_interest=, accuracy=)", 2)
        if authority is not None and authority.upper() not in ("EPSG", "ANY"):
            raise out(f"TransformerGroup(authority={authority!r})", "only EPSG's operations are known")
        if allow_superseded or crs_extent_use is not None or pivot_crs is not None or grid_check is not None:
            raise out(
                "TransformerGroup(allow_superseded=, crs_extent_use=, pivot_crs=, grid_check=)",
                "xeibe-reproject searches as PROJ does by default",
            )
        source, target = CRS.from_user_input(crs_from), CRS.from_user_input(crs_to)
        network = is_network_enabled()
        local = set(_grids.local_files())
        # Every candidate PROJ-data has grids for; availability is decided here.
        everything = _native.Transform(_native_json(source), _native_json(target), False, True)
        infos = everything.operation_info()
        self._transformers: list[Transformer] = []
        self._unavailable: list[CoordinateOperation] = []
        if not infos:
            # Same module: TransformerGroup makes Transformers as from_crs does.
            self._transformers.append(Transformer._make(source, target, always_xy))  # pyright: ignore[reportPrivateUsage]
        for index, info in enumerate(infos):
            only = everything.with_operation_index(index)
            if only is not None and _available(info, network, local):
                self._transformers.append(
                    Transformer._from_native(  # pyright: ignore[reportPrivateUsage]
                        source, target, always_xy, only, info.name, network
                    )
                )
            else:
                self._unavailable.append(_operation(info, False))
        self._best_available = not infos or _available(infos[0], network, local)
        if not self._best_available:
            missing = ", ".join(infos[0].grids)
            warnings.warn(f"Best transformation is not available due to missing grids: {missing}", stacklevel=2)

    @property
    def transformers(self) -> list[Transformer]:
        """The transformers whose grids can be had, best first."""
        return list(self._transformers)

    @property
    def unavailable_operations(self) -> list[CoordinateOperation]:
        """The operations whose grids are neither local nor, with the network
        off, to be fetched."""
        return list(self._unavailable)

    @property
    def best_available(self) -> bool:
        """Whether the best operation is among the transformers."""
        return self._best_available

    def download_grids(self, directory: Any = None, open_license: bool = True, verbose: bool = False) -> None:
        raise not_yet("TransformerGroup.download_grids", 2)

    def __repr__(self) -> str:
        return (
            f"<TransformerGroup: best_available={self.best_available}>\n"
            f"- transformers: {len(self._transformers)}\n"
            f"- unavailable_operations: {len(self._unavailable)}"
        )



def transform(*args: Any, **kwargs: Any) -> Any:
    """pyproj's deprecated module-level ``transform``: not supported."""
    raise out("pyproj.transform", "pyproj deprecated it; use Transformer.from_crs().transform")


def itransform(*args: Any, **kwargs: Any) -> Any:
    """pyproj's deprecated module-level ``itransform``: not supported."""
    raise out("pyproj.itransform", "pyproj deprecated it; use Transformer.from_crs().itransform")
