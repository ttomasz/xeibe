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
from array import array
from collections.abc import Iterable, Iterator, Sequence
from itertools import chain, islice
from typing import TYPE_CHECKING, Any, overload

from xeibe_proj import _containers, _grids, _native
from xeibe_proj._epsg import OGC_CRS
from xeibe_proj._json import ident
from xeibe_proj._unsupported import not_yet, out
from xeibe_proj.aoi import AreaOfInterest, AreaOfUse
from xeibe_proj.crs import CRS
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


def _native_json(crs: CRS) -> str:
    """The PROJJSON handed to Rust: an OGC CRS as its EPSG twin with its
    own axes, so that EPSG's transformations apply to it."""
    projjson = crs.to_json_dict()
    auth, code = ident(projjson) or ("", "")
    twin = OGC_CRS.get(code.upper()) if auth == "OGC" else None
    if twin is not None:
        projjson["id"] = {"authority": "EPSG", "code": twin[0]}
    return json.dumps(projjson)


class Transformer:
    """A conversion from one CRS to another, made with ``Transformer.from_crs``."""

    _source: CRS
    _target: CRS
    _always_xy: bool
    _network: bool
    _forward: _native.Transform
    _backward: _native.Transform | None
    # The directions (True: forward) whose grid use has been logged.
    _logged: set[bool]

    def __init__(self, *args: Any, **kwargs: Any) -> None:
        raise ProjError("use Transformer.from_crs() to make a Transformer")

    @classmethod
    def _make(cls, source: CRS, target: CRS, always_xy: bool) -> Transformer:
        self = cls.__new__(cls)
        self._source = source
        self._target = target
        self._always_xy = always_xy
        self._network = is_network_enabled()
        self._forward = _native.Transform(_native_json(source), _native_json(target), always_xy, self._network)
        self._backward = None
        self._logged = set()
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
        raise not_yet("Transformer.accuracy (needs operation metadata from xeibe-reproject)", 1)

    @property
    def area_of_use(self) -> AreaOfUse:
        raise not_yet("Transformer.area_of_use (needs operation metadata from xeibe-reproject)", 1)

    @property
    def remarks(self) -> str:
        raise not_yet("Transformer.remarks (needs operation metadata from xeibe-reproject)", 1)

    @property
    def scope(self) -> str:
        raise not_yet("Transformer.scope (needs operation metadata from xeibe-reproject)", 1)

    @property
    def operations(self) -> tuple[Any, ...] | None:
        raise not_yet("Transformer.operations (needs operation metadata from xeibe-reproject)", 1)

    def get_last_used_operation(self) -> Transformer:
        raise not_yet("Transformer.get_last_used_operation (needs operation metadata from xeibe-reproject)", 1)

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

    def _plan(self, direction: Direction) -> tuple[_native.Transform | None, CRS, CRS]:
        """The native transform for ``direction`` (``None`` for IDENT), and
        the CRSs coordinates come from and go to."""
        d = TransformDirection.create(direction)
        if d is TransformDirection.IDENT:
            return None, self._source, self._source
        if d is TransformDirection.FORWARD:
            return self._forward, self._source, self._target
        if self._backward is None:
            self._backward = self._forward.inverse()
        return self._backward, self._target, self._source

    def _load_grids(self, native: _native.Transform, forward: bool, bounds: tuple[float, float, float, float] | None) -> None:
        if not self._network:
            return
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
    ) -> None:
        extent = _native.extent(x, y)
        if extent is not None and radians_in:
            extent = (
                math.degrees(extent[0]),
                math.degrees(extent[1]),
                math.degrees(extent[2]),
                math.degrees(extent[3]),
            )
        self._load_grids(native, forward, extent)
        native.convert(x, y, z, radians_in, radians_out)

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
        native, source, target = self._plan(direction)
        bx, kx = _containers.to_buffer(xx)
        by, ky = _containers.to_buffer(yy)
        bz = kz = None
        if zz is not None:
            bz, kz = _containers.to_buffer(zz)
        if native is not None:
            forward = TransformDirection.create(direction) is TransformDirection.FORWARD
            self._convert(native, forward, bx, by, bz, radians and _degrees(source), radians and _degrees(target))
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
        native, source, target = self._plan(direction)
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
                self._convert(native, forward, x, y, z, radians and _degrees(source), radians and _degrees(target))
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
        native, source, target = self._plan(direction)
        bounds = (float(left), float(bottom), float(right), float(top))
        if native is None:
            return bounds
        if radians and _degrees(source):
            bounds = (math.degrees(bounds[0]), math.degrees(bounds[1]), math.degrees(bounds[2]), math.degrees(bounds[3]))
        forward = TransformDirection.create(direction) is TransformDirection.FORWARD
        self._load_grids(native, forward, bounds)
        result = native.convert_bounds(bounds, densify_pts)
        if radians and _degrees(target):
            result = (math.radians(result[0]), math.radians(result[1]), math.radians(result[2]), math.radians(result[3]))
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

    def __reduce__(self) -> tuple[Any, tuple[CRS, CRS, bool]]:
        return Transformer._make, (self._source, self._target, self._always_xy)

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
    """Every transformer between two CRSs (phase 1, not implemented yet:
    it needs operation metadata from ``xeibe-reproject``)."""

    def __init__(self, crs_from: Any, crs_to: Any, always_xy: bool = False, **kwargs: Any) -> None:
        raise not_yet("TransformerGroup (needs operation metadata from xeibe-reproject)", 1)


def transform(*args: Any, **kwargs: Any) -> Any:
    """pyproj's deprecated module-level ``transform``: not supported."""
    raise out("pyproj.transform", "pyproj deprecated it; use Transformer.from_crs().transform")


def itransform(*args: Any, **kwargs: Any) -> Any:
    """pyproj's deprecated module-level ``itransform``: not supported."""
    raise out("pyproj.itransform", "pyproj deprecated it; use Transformer.from_crs().itransform")
