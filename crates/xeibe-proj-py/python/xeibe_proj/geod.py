"""``Geod``: geodesic computations on an ellipsoid, as pyproj's.

The argument handling, the containers and the warnings follow pyproj's
``geod.py`` and ``_geod.pyx`` (MIT, see NOTICE.md); the loops are
``xeibe_reproject::geodesic``, a port of PROJ's ``geodesic.c``, the code
pyproj calls (``docs/pyproj-phase2.md``, Geodesics).

Differences from pyproj (``docs/pyproj-compat.md``): ``inplace`` doesn't
exist on ``fwd`` and ``inv``; passing it raises ``NotImplementedError``,
and the parameters after it are keyword-only.
"""

from __future__ import annotations

import math
import warnings
from array import array
from typing import TYPE_CHECKING, Any, NamedTuple

from xeibe_proj import _containers, _native
from xeibe_proj._unsupported import out
from xeibe_proj.enums import GeodIntermediateFlag
from xeibe_proj.exceptions import GeodError
from xeibe_proj.list import get_ellps_map

__all__ = [
    "Geod",
    "GeodIntermediateFlag",
    "GeodIntermediateReturn",
    "geodesic_version_str",
    "pj_ellps",
    "reverse_azimuth",
]

#: The version of ``geodesic.c`` ported, as pyproj reports PROJ's.
geodesic_version_str = "2.2.0"

pj_ellps = get_ellps_map()

_NPTS_MASK = GeodIntermediateFlag.NPTS_ROUND | GeodIntermediateFlag.NPTS_CEIL | GeodIntermediateFlag.NPTS_TRUNC
_DEL_S_MASK = GeodIntermediateFlag.DEL_S_RECALC | GeodIntermediateFlag.DEL_S_NO_RECALC
_AZIS_MASK = GeodIntermediateFlag.AZIS_DISCARD | GeodIntermediateFlag.AZIS_KEEP


class GeodIntermediateReturn(NamedTuple):
    """What ``inv_intermediate`` and ``fwd_intermediate`` return."""

    npts: int
    del_s: float
    dist: float
    lons: Any
    lats: Any
    azis: Any


def _reject(method: str, args: tuple[Any, ...], kwargs: dict[str, Any]) -> None:
    """Raise for ``inplace``, by keyword or in pyproj's position, and for
    any other unexpected argument."""
    if args or "inplace" in kwargs:
        raise out(
            f"Geod.{method}(inplace=…)",
            "results are always new objects; return_back_azimuth is keyword-only",
        )
    if kwargs:
        raise TypeError(f"Geod.{method}() got unexpected arguments: {', '.join(kwargs)}")


def _params_from_ellps_map(ellps: str) -> tuple[float, float, float, float, bool]:
    ellps_dict = pj_ellps[ellps]
    semi_major_axis, semi_minor_axis, flattening, eccentricity_squared = _params_from_kwargs(ellps_dict)
    sphere = ellps_dict["description"] == "Normal Sphere"
    return semi_major_axis, semi_minor_axis, flattening, eccentricity_squared, sphere


def _params_from_kwargs(kwargs: dict[str, Any]) -> tuple[float, float, float, float]:
    semi_major_axis = kwargs["a"]
    if "b" in kwargs:
        semi_minor_axis = kwargs["b"]
        eccentricity_squared = 1.0 - semi_minor_axis**2 / semi_major_axis**2
        flattening = (semi_major_axis - semi_minor_axis) / semi_major_axis
    elif "rf" in kwargs:
        flattening = 1.0 / kwargs["rf"]
        semi_minor_axis = semi_major_axis * (1.0 - flattening)
        eccentricity_squared = 1.0 - semi_minor_axis**2 / semi_major_axis**2
    elif "f" in kwargs:
        flattening = kwargs["f"]
        semi_minor_axis = semi_major_axis * (1.0 - flattening)
        eccentricity_squared = 1.0 - (semi_minor_axis / semi_major_axis) ** 2
    elif "es" in kwargs:
        eccentricity_squared = kwargs["es"]
        semi_minor_axis = math.sqrt(semi_major_axis**2 - eccentricity_squared * semi_major_axis**2)
        flattening = (semi_major_axis - semi_minor_axis) / semi_major_axis
    elif "e" in kwargs:
        eccentricity_squared = kwargs["e"] ** 2
        semi_minor_axis = math.sqrt(semi_major_axis**2 - eccentricity_squared * semi_major_axis**2)
        flattening = (semi_major_axis - semi_minor_axis) / semi_major_axis
    else:
        semi_minor_axis = semi_major_axis
        flattening = 0.0
        eccentricity_squared = 0.0
    return semi_major_axis, semi_minor_axis, flattening, eccentricity_squared


def _divide(x: float, y: float) -> float:
    """x / y as C divides doubles: by zero gives ±inf, or NaN for 0 / 0."""
    if y == 0:
        if x == 0 or math.isnan(x):
            return math.nan
        return math.copysign(math.inf, x) * math.copysign(1.0, y)
    return x / y


def _c_round(x: float) -> float:
    """C's ``round``: half away from zero."""
    r = math.floor(abs(x))
    if abs(x) - r >= 0.5:
        r += 1
    return math.copysign(r, x)


class Geod:
    """Forward and inverse geodesic computations on an ellipsoid.

    The ellipsoid is given by ``ellps`` (one of :data:`pj_ellps`), or by
    ``a`` and one of ``b``, ``rf``, ``f``, ``es`` or ``e``, as keywords or
    as a PROJ-style string (``"+a=6378137 +rf=298.257223563"``). Angles
    are in degrees unless ``radians=True``, distances in metres.
    """

    initstring: str
    sphere: bool
    a: float
    b: float
    es: float
    f: float

    def __init__(self, initstring: str | None = None, **kwargs: Any) -> None:
        ellpsd: dict[str, str | float] = {}
        if initstring is not None:
            for kvpair in initstring.split():
                # Only the ellipsoid's parameters matter; anything without a
                # value is ignored.
                if kvpair.find("=") == -1:
                    continue
                key, val = kvpair.split("=")
                key = key.lstrip("+")
                if key in ["a", "b", "rf", "f", "es", "e"]:
                    ellpsd[key] = float(val)
                else:
                    ellpsd[key] = val
        kwargs = dict(list(kwargs.items()) + list(ellpsd.items()))
        sphere = False
        if "ellps" in kwargs:
            semi_major_axis, semi_minor_axis, flattening, eccentricity_squared, sphere = _params_from_ellps_map(
                kwargs["ellps"]
            )
        else:
            semi_major_axis, semi_minor_axis, flattening, eccentricity_squared = _params_from_kwargs(kwargs)
        if math.fabs(flattening) < 1.0e-8:
            sphere = True
        # Doubles, as pyproj's Cython class takes them.
        self.a = float(semi_major_axis)
        self.f = float(flattening)
        self.initstring = f"+a={self.a!r} +f={self.f!r}"
        self.sphere = bool(sphere)
        self.b = float(semi_minor_axis)
        self.es = float(eccentricity_squared)
        self._geod = _native.Geodesic(self.a, self.f)

    def __reduce__(self) -> tuple[type[Geod], tuple[str]]:
        return self.__class__, (self.initstring,)

    if TYPE_CHECKING:
        # The signatures type checkers see: no inplace, as at runtime.
        def fwd(
            self,
            lons: Any,
            lats: Any,
            az: Any,
            dist: Any,
            radians: bool = False,
            *,
            return_back_azimuth: bool = True,
        ) -> tuple[Any, Any, Any]: ...

        def inv(
            self,
            lons1: Any,
            lats1: Any,
            lons2: Any,
            lats2: Any,
            radians: bool = False,
            *,
            return_back_azimuth: bool = True,
        ) -> tuple[Any, Any, Any]: ...

    else:

        def fwd(
            self,
            lons: Any,
            lats: Any,
            az: Any,
            dist: Any,
            radians: bool = False,
            *args: Any,
            return_back_azimuth: bool = True,
            **kwargs: Any,
        ) -> tuple[Any, Any, Any]:
            """The longitudes, latitudes and back azimuths (forward azimuths
            with ``return_back_azimuth=False``) at the end points of geodesics
            from ``lons``, ``lats`` along ``az`` for ``dist`` metres."""
            if args or kwargs:
                _reject("fwd", args, kwargs)
            inx, x_kind = _containers.to_buffer(lons)
            iny, y_kind = _containers.to_buffer(lats)
            inz, z_kind = _containers.to_buffer(az)
            ind = _containers.to_buffer(dist)[0]
            self._geod.fwd(inx, iny, inz, ind, radians, return_back_azimuth)
            return (
                _containers.from_buffer(x_kind, inx),
                _containers.from_buffer(y_kind, iny),
                _containers.from_buffer(z_kind, inz),
            )

        def inv(
            self,
            lons1: Any,
            lats1: Any,
            lons2: Any,
            lats2: Any,
            radians: bool = False,
            *args: Any,
            return_back_azimuth: bool = True,
            **kwargs: Any,
        ) -> tuple[Any, Any, Any]:
            """The forward azimuths, back azimuths (forward azimuths at the end
            with ``return_back_azimuth=False``) and distances between pairs of
            points."""
            if args or kwargs:
                _reject("inv", args, kwargs)
            inx, x_kind = _containers.to_buffer(lons1)
            iny, y_kind = _containers.to_buffer(lats1)
            inz, z_kind = _containers.to_buffer(lons2)
            ind = _containers.to_buffer(lats2)[0]
            self._geod.inv(inx, iny, inz, ind, radians, return_back_azimuth)
            return (
                _containers.from_buffer(x_kind, inx),
                _containers.from_buffer(y_kind, iny),
                _containers.from_buffer(z_kind, inz),
            )

    def npts(
        self,
        lon1: float,
        lat1: float,
        lon2: float,
        lat2: float,
        npts: int,
        radians: bool = False,
        initial_idx: int = 1,
        terminus_idx: int = 1,
    ) -> list[tuple[float, float]]:
        """``npts`` points equally spaced along the geodesic between two
        points, by default without the end points, as (lon, lat) pairs."""
        res = self._intermediate(
            lon1,
            lat1,
            lon2,
            lat2,
            npts=npts,
            del_s=0,
            radians=radians,
            initial_idx=initial_idx,
            terminus_idx=terminus_idx,
            flags=GeodIntermediateFlag.AZIS_DISCARD,
            out_lons=None,
            out_lats=None,
            out_azis=None,
            return_back_azimuth=False,
            is_fwd=False,
        )
        return list(zip(res.lons, res.lats, strict=True))

    def inv_intermediate(
        self,
        lon1: float,
        lat1: float,
        lon2: float,
        lat2: float,
        npts: int = 0,
        del_s: float = 0,
        initial_idx: int = 1,
        terminus_idx: int = 1,
        radians: bool = False,
        flags: GeodIntermediateFlag = GeodIntermediateFlag.DEFAULT,
        out_lons: Any | None = None,
        out_lats: Any | None = None,
        out_azis: Any | None = None,
        return_back_azimuth: bool | None = None,
    ) -> GeodIntermediateReturn:
        """Points along the geodesic between two points, given their number
        (``npts``) or their spacing (``del_s``), written into ``out_*``
        when given."""
        if return_back_azimuth is None:
            return_back_azimuth = True
            warnings.warn(
                "Back azimuth is being returned by default to be compatible with fwd()"
                "This is a breaking change for pyproj 3.5+."
                "To avoid this warning, set return_back_azimuth=True."
                "Otherwise, to restore old behaviour, set return_back_azimuth=False."
                "This warning will be removed in future version.",
                stacklevel=2,
            )
        return self._intermediate(
            lon1,
            lat1,
            lon2,
            lat2,
            npts=npts,
            del_s=del_s,
            radians=radians,
            initial_idx=initial_idx,
            terminus_idx=terminus_idx,
            flags=int(flags),
            out_lons=out_lons,
            out_lats=out_lats,
            out_azis=out_azis,
            return_back_azimuth=return_back_azimuth,
            is_fwd=False,
        )

    def fwd_intermediate(
        self,
        lon1: float,
        lat1: float,
        azi1: float,
        npts: int,
        del_s: float,
        initial_idx: int = 1,
        terminus_idx: int = 1,
        radians: bool = False,
        flags: GeodIntermediateFlag = GeodIntermediateFlag.DEFAULT,
        out_lons: Any | None = None,
        out_lats: Any | None = None,
        out_azis: Any | None = None,
        return_back_azimuth: bool | None = None,
    ) -> GeodIntermediateReturn:
        """``npts`` points ``del_s`` metres apart along the geodesic from a
        point along an azimuth, written into ``out_*`` when given."""
        if return_back_azimuth is None:
            return_back_azimuth = True
            warnings.warn(
                "Back azimuth is being returned by default to be compatible with inv()"
                "This is a breaking change for pyproj 3.5+."
                "To avoid this warning, set return_back_azimuth=True."
                "Otherwise, to restore old behaviour, set return_back_azimuth=False."
                "This warning will be removed in future version.",
                stacklevel=2,
            )
        return self._intermediate(
            lon1,
            lat1,
            azi1,
            math.nan,
            npts=npts,
            del_s=del_s,
            radians=radians,
            initial_idx=initial_idx,
            terminus_idx=terminus_idx,
            flags=int(flags),
            out_lons=out_lons,
            out_lats=out_lats,
            out_azis=out_azis,
            return_back_azimuth=return_back_azimuth,
            is_fwd=True,
        )

    def _intermediate(
        self,
        lon1: float,
        lat1: float,
        lon2_or_azi1: float,
        lat2: float,
        *,
        npts: int,
        del_s: float,
        radians: bool,
        initial_idx: int,
        terminus_idx: int,
        flags: int,
        out_lons: Any | None,
        out_lats: Any | None,
        out_azis: Any | None,
        return_back_azimuth: bool,
        is_fwd: bool,
    ) -> GeodIntermediateReturn:
        """pyproj's ``_inv_or_fwd_intermediate``."""
        store_az = out_azis is not None or (flags & _AZIS_MASK) == GeodIntermediateFlag.AZIS_KEEP
        if not is_fwd and (del_s == 0) == (npts == 0):
            raise GeodError(
                "inv_intermediate: npts and del_s are mutually exclusive, only one of them must be != 0."
            )
        if radians:
            lon1 = math.degrees(lon1)
            lat1 = math.degrees(lat1)
            lon2_or_azi1 = math.degrees(lon2_or_azi1)
            if not is_fwd:
                lat2 = math.degrees(lat2)
        if is_fwd:
            line = self._geod.line(lat1, lon1, lon2_or_azi1)
            dist = del_s * (npts + initial_idx + terminus_idx - 1)
        else:
            line = self._geod.inverse_line(lat1, lon1, lat2, lon2_or_azi1)
            dist = line.distance
            if npts == 0:
                count = _divide(dist, del_s) - initial_idx - terminus_idx + 1
                if (flags & _NPTS_MASK) == GeodIntermediateFlag.NPTS_ROUND:
                    count = _c_round(count)
                elif (flags & _NPTS_MASK) == GeodIntermediateFlag.NPTS_CEIL:
                    count = math.ceil(count)
                npts = int(count)
            if (flags & _DEL_S_MASK) == GeodIntermediateFlag.DEL_S_RECALC:
                del_s = _divide(dist, npts + initial_idx + terminus_idx - 1)
        if out_lons is None:
            out_lons = array("d", bytes(8 * npts))
        if out_lats is None:
            out_lats = array("d", bytes(8 * npts))
        if out_azis is None and store_az:
            out_azis = array("d", bytes(8 * npts))
        lons, lats = memoryview(out_lons), memoryview(out_lats)
        azis = memoryview(out_azis) if out_azis is not None else None
        if len(lons) < npts or len(lats) < npts or (azis is not None and len(azis) < npts):
            raise GeodError(
                "Arrays are not long enough ("
                f"{len(lons)}, {len(lats)}, {len(azis) if azis is not None else -1}) < {npts}."
            )
        line.fill(
            lons[:npts],
            lats[:npts],
            azis[:npts] if azis is not None else None,
            initial_idx,
            del_s,
            radians,
            return_back_azimuth,
        )
        return GeodIntermediateReturn(npts, del_s, dist, out_lons, out_lats, out_azis)

    def line_length(self, lons: Any, lats: Any, radians: bool = False) -> float:
        """The total length of the line through the points, in metres."""
        inx = _containers.to_buffer(lons)[0]
        iny = _containers.to_buffer(lats)[0]
        return self._geod.line_length(inx, iny, radians)

    def line_lengths(self, lons: Any, lats: Any, radians: bool = False) -> Any:
        """The length of each segment of the line through the points."""
        inx, x_kind = _containers.to_buffer(lons)
        iny = _containers.to_buffer(lats)[0]
        self._geod.line_length(inx, iny, radians)
        line_lengths = _containers.from_buffer(x_kind, inx)
        return line_lengths if x_kind is _containers.Kind.FLOAT else line_lengths[:-1]

    def polygon_area_perimeter(self, lons: Any, lats: Any, radians: bool = False) -> tuple[float, float]:
        """The area (positive counter-clockwise), in square metres, and the
        perimeter of the polygon with these vertices, which needn't repeat
        the first one."""
        return self._geod.polygon_area_perimeter(
            _containers.to_buffer(lons)[0], _containers.to_buffer(lats)[0], radians
        )

    def geometry_length(self, geometry: Any, radians: bool = False) -> float:
        """The length of a shapely geometry (or anything with its ``xy``,
        ``exterior`` and ``geoms``); a polygon's is its exterior's."""
        try:
            lons, lats = geometry.xy
            return self.line_length(lons, lats, radians=radians)
        except (AttributeError, NotImplementedError):
            pass
        if hasattr(geometry, "exterior"):
            return self.geometry_length(geometry.exterior, radians=radians)
        if hasattr(geometry, "geoms"):
            total_length = 0.0
            for geom in geometry.geoms:
                total_length += self.geometry_length(geom, radians=radians)
            return total_length
        raise GeodError("Invalid geometry provided.")

    def geometry_area_perimeter(self, geometry: Any, radians: bool = False) -> tuple[float, float]:
        """The area and perimeter of a shapely geometry; holes subtract
        from a polygon's area (they turn the other way) and don't count in
        its perimeter."""
        try:
            lons, lats = geometry.xy
            return self.polygon_area_perimeter(lons, lats, radians=radians)
        except (AttributeError, NotImplementedError):
            pass
        if hasattr(geometry, "exterior"):
            total_area, total_perimeter = self.geometry_area_perimeter(geometry.exterior, radians=radians)
            for hole in geometry.interiors:
                area, _ = self.geometry_area_perimeter(hole, radians=radians)
                total_area += area
            return total_area, total_perimeter
        if hasattr(geometry, "geoms"):
            total_area = 0.0
            total_perimeter = 0.0
            for geom in geometry.geoms:
                area, perimeter = self.geometry_area_perimeter(geom, radians=radians)
                total_area += area
                total_perimeter += perimeter
            return total_area, total_perimeter
        raise GeodError("Invalid geometry provided.")

    def __repr__(self) -> str:
        # The name of a named ellipsoid when it is one.
        for ellps, vals in pj_ellps.items():
            if self.a == vals["a"]:
                if self.b == vals.get("b") or (not self.sphere and 1.0 / self.f == vals.get("rf")):
                    return f"{self.__class__.__name__}(ellps={ellps!r})"
        return f"{self.__class__.__name__}({self.initstring!r})"

    def __eq__(self, other: object) -> bool:
        if not isinstance(other, Geod):
            return False
        return self.__repr__() == other.__repr__()

    def __hash__(self) -> int:
        return hash(self.__repr__())


def reverse_azimuth(azi: Any, radians: bool = False) -> Any:
    """The azimuths turned by a half turn."""
    inazi, kind = _containers.to_buffer(azi)
    _native.reverse_azimuth(inazi, radians)
    return _containers.from_buffer(kind, inazi)
