"""The CF conventions' grid mapping attributes (CF 1.8) of conversions,
datums and coordinate systems, as ``pyproj.crs._cf1x8``, for ``CRS.to_cf``,
``from_cf`` and ``cs_to_cf``: pyproj 3.8.0's attributes and choices (MIT,
see NOTICE.md), from one table that maps each grid mapping to its
conversion both ways (``docs/pyproj-phase3.md``, "CF")."""

from __future__ import annotations

import math
import warnings
from collections.abc import Callable
from dataclasses import dataclass
from typing import Any

from xeibe_proj._json import JsonDict
from xeibe_proj.crs._objects import CoordinateOperation, Datum, Ellipsoid, Param, PrimeMeridian
from xeibe_proj.crs.coordinate_operation import (
    GeostationarySatelliteConversion,
    LambertCylindricalEqualAreaScaleConversion,
    PoleRotationNetCDFCFConversion,
)
from xeibe_proj.crs._conversion import conversion
from xeibe_proj.crs.datum import CustomDatum, CustomEllipsoid, CustomPrimeMeridian
from xeibe_proj.exceptions import CRSError

__all__ = ["_try_list_if_string", "try_list_if_string"]

_UNKNOWN = ("undefined", "unknown")
# An attribute a grid mapping needs.
_REQUIRED = math.nan


@dataclass(frozen=True)
class _Mapping:
    """A grid mapping as one method's conversion: each CF attribute with
    the EPSG parameter it is, in the method's order, and its default
    (``_REQUIRED`` for none). ``standard_parallel`` with two values is
    ``standard_parallel[0]`` and ``[1]``."""

    grid_mapping: str
    method: int | str
    attributes: tuple[tuple[str, int, float], ...]
    # Whether the CF attributes are of this method's conversion.
    applies: Callable[[dict[str, Any]], bool] = lambda cf: True


_FALSE = (("false_easting", 8806, 0.0), ("false_northing", 8807, 0.0))
_ORIGIN = (("latitude_of_projection_origin", 8801, 0.0), ("longitude_of_projection_origin", 8802, 0.0))


def _parallels(cf: dict[str, Any]) -> list[float]:
    value = try_list_if_string(cf.get("standard_parallel"))
    if value is None:
        return []
    try:
        return [float(value)]
    except TypeError:
        return [float(v) for v in value]


def _one_parallel_at_origin(cf: dict[str, Any]) -> bool:
    """Lambert Conic Conformal with one standard parallel, which is its
    latitude of origin (CF's tangent cone, as pyproj reads it)."""
    parallels = _parallels(cf)
    origin = cf.get("latitude_of_projection_origin")
    return len(parallels) == 1 and (origin is None or float(origin) == parallels[0])


# In the order `from_cf` tries them for a grid mapping.
_MAPPINGS = (
    _Mapping(
        "albers_conical_equal_area",
        9822,
        (
            ("latitude_of_projection_origin", 8821, 0.0),
            ("longitude_of_central_meridian", 8822, 0.0),
            ("standard_parallel[0]", 8823, _REQUIRED),
            ("standard_parallel[1]", 8824, 0.0),
            ("false_easting", 8826, 0.0),
            ("false_northing", 8827, 0.0),
        ),
    ),
    _Mapping("azimuthal_equidistant", 9832, (*_ORIGIN, *_FALSE)),
    _Mapping("lambert_azimuthal_equal_area", 9820, (*_ORIGIN, *_FALSE)),
    _Mapping(
        "lambert_conformal_conic",
        9801,
        (
            ("standard_parallel[0]", 8801, _REQUIRED),
            ("longitude_of_central_meridian", 8802, 0.0),
            ("", 8805, 1.0),
            *_FALSE,
        ),
        _one_parallel_at_origin,
    ),
    # Two parallels, or one away from the origin: the cone through it twice.
    _Mapping(
        "lambert_conformal_conic",
        9802,
        (
            ("standard_parallel[0]", 8823, _REQUIRED),
            ("standard_parallel[1]", 8824, _REQUIRED),
            ("latitude_of_projection_origin", 8821, 0.0),
            ("longitude_of_central_meridian", 8822, 0.0),
            ("false_easting", 8826, 0.0),
            ("false_northing", 8827, 0.0),
        ),
    ),
    _Mapping(
        "lambert_cylindrical_equal_area",
        9835,
        (("standard_parallel", 8823, 0.0), ("longitude_of_central_meridian", 8802, 0.0), *_FALSE),
        lambda cf: "scale_factor_at_projection_origin" not in cf,
    ),
    _Mapping(
        "mercator",
        9804,
        (
            ("standard_parallel", 8801, 0.0),
            ("longitude_of_projection_origin", 8802, 0.0),
            ("scale_factor_at_projection_origin", 8805, 1.0),
            *_FALSE,
        ),
        lambda cf: "scale_factor_at_projection_origin" in cf,
    ),
    _Mapping(
        "mercator",
        9805,
        (("standard_parallel", 8823, 0.0), ("longitude_of_projection_origin", 8802, 0.0), *_FALSE),
    ),
    _Mapping(
        "oblique_mercator",
        9815,
        (
            ("latitude_of_projection_origin", 8811, _REQUIRED),
            ("longitude_of_projection_origin", 8812, _REQUIRED),
            ("azimuth_of_central_line", 8813, _REQUIRED),
            # CF's oblique Mercator has its grid rectified at a right angle.
            ("", 8814, 90.0),
            ("scale_factor_at_projection_origin", 8815, 1.0),
            ("false_easting", 8816, 0.0),
            ("false_northing", 8817, 0.0),
        ),
    ),
    _Mapping("orthographic", 9840, (*_ORIGIN, *_FALSE)),
    _Mapping(
        "polar_stereographic",
        9829,
        (
            ("standard_parallel", 8832, _REQUIRED),
            ("straight_vertical_longitude_from_pole", 8833, _REQUIRED),
            *_FALSE,
        ),
        lambda cf: "standard_parallel" in cf,
    ),
    _Mapping(
        "polar_stereographic",
        9810,
        (
            ("latitude_of_projection_origin", 8801, _REQUIRED),
            ("straight_vertical_longitude_from_pole", 8802, _REQUIRED),
            ("scale_factor_at_projection_origin", 8805, 1.0),
            *_FALSE,
        ),
    ),
    _Mapping("sinusoidal", "Sinusoidal", (("longitude_of_projection_origin", 8802, 0.0), *_FALSE)),
    _Mapping(
        "stereographic",
        "Stereographic",
        (*_ORIGIN, ("scale_factor_at_projection_origin", 8805, 1.0), *_FALSE),
    ),
    _Mapping(
        "transverse_mercator",
        9807,
        (
            ("latitude_of_projection_origin", 8801, 0.0),
            ("longitude_of_central_meridian", 8802, 0.0),
            ("scale_factor_at_central_meridian", 8805, 1.0),
            *_FALSE,
        ),
    ),
    _Mapping(
        "vertical_perspective",
        9838,
        (
            ("latitude_of_projection_origin", 8834, 0.0),
            ("longitude_of_projection_origin", 8835, 0.0),
            ("", 8836, 0.0),
            ("perspective_point_height", 8840, _REQUIRED),
            *_FALSE,
        ),
    ),
)
# EPSG's Azimuthal Equidistant is CF's too.
_METHOD_ALIASES = {1125: 9832}


def try_list_if_string(input_str: Any) -> Any:
    """``"25., 30."`` as ``[25.0, 30.0]``; anything else as it is."""
    if not isinstance(input_str, str):
        return input_str
    parts = input_str.split(",")
    if len(parts) > 1:
        return [float(p.strip()) for p in parts]
    return input_str


def _cf_value(cf: dict[str, Any], attribute: str, default: float, grid_mapping: str) -> float:
    if attribute.startswith("standard_parallel["):
        parallels = _parallels(cf)
        index = int(attribute[-2])
        if index < len(parallels):
            return parallels[index]
        # One parallel of a cone through it twice.
        if parallels and default is _REQUIRED:
            return parallels[0]
        value = None
    else:
        value = cf.get(attribute) if attribute else None
    if value is None:
        if default is _REQUIRED:
            raise CRSError(f"CF grid mapping {grid_mapping} needs {attribute.split('[')[0]}")
        return default
    return float(value)


def conversion_from_cf(cf: dict[str, Any], grid_mapping: str) -> CoordinateOperation:
    if grid_mapping == "geostationary":
        sweep = cf.get("sweep_angle_axis")
        if sweep is None:
            sweep = {"x": "y", "y": "x"}[str(cf["fixed_angle_axis"]).lower()]
        return GeostationarySatelliteConversion(
            sweep_angle_axis=str(sweep),
            satellite_height=_cf_value(cf, "perspective_point_height", _REQUIRED, grid_mapping),
            latitude_natural_origin=_cf_value(cf, "latitude_of_projection_origin", 0.0, grid_mapping),
            longitude_natural_origin=_cf_value(cf, "longitude_of_projection_origin", 0.0, grid_mapping),
            false_easting=_cf_value(cf, "false_easting", 0.0, grid_mapping),
            false_northing=_cf_value(cf, "false_northing", 0.0, grid_mapping),
        )
    if grid_mapping == "lambert_cylindrical_equal_area" and "scale_factor_at_projection_origin" in cf:
        return LambertCylindricalEqualAreaScaleConversion(
            longitude_natural_origin=_cf_value(cf, "longitude_of_central_meridian", 0.0, grid_mapping),
            false_easting=_cf_value(cf, "false_easting", 0.0, grid_mapping),
            false_northing=_cf_value(cf, "false_northing", 0.0, grid_mapping),
            scale_factor_natural_origin=_cf_value(cf, "scale_factor_at_projection_origin", 1.0, grid_mapping),
        )
    mapping = next((m for m in _MAPPINGS if m.grid_mapping == grid_mapping and m.applies(cf)), None)
    if mapping is None:
        raise CRSError(f"Unsupported grid mapping name: {grid_mapping}")
    if mapping.method == 9804 and _cf_value(cf, "standard_parallel", 0.0, grid_mapping) != 0:
        raise CRSError("This conversion is defined for only latitude_natural_origin = 0.")
    values = [(code, _cf_value(cf, attribute, default, grid_mapping)) for attribute, code, default in mapping.attributes]
    return CoordinateOperation.from_json_dict(conversion(mapping.method, values))


def _secant_parallels(lat0: float, k0: float, ellipsoid: Ellipsoid) -> tuple[float, float] | None:
    """The two parallels of the secant cone a Lambert Conic Conformal (1SP)
    with scale ``k0`` < 1 at ``lat0`` is: where its scale is 1, on either
    side of ``lat0``, in degrees. ``None`` for ``k0`` > 1, which no cone
    through two parallels is."""
    if not 0 < k0 < 1 or abs(lat0) >= 90:
        return None
    e = math.sqrt(max(0.0, 1 - (ellipsoid.semi_minor_metre / ellipsoid.semi_major_metre) ** 2))

    def log_m(phi: float) -> float:
        return math.log(math.cos(phi) / math.sqrt(1 - (e * math.sin(phi)) ** 2))

    def log_t(phi: float) -> float:
        es = e * math.sin(phi)
        return math.log(math.tan(math.pi / 4 - phi / 2)) - e / 2 * math.log((1 - es) / (1 + es))

    phi0 = math.radians(lat0)
    n = math.sin(phi0)

    # The cone's log scale at phi: 0 at the parallels sought, log k0 at phi0.
    def log_scale(phi: float) -> float:
        return math.log(k0) + log_m(phi0) - log_m(phi) + n * (log_t(phi) - log_t(phi0))

    def root(lo: float, hi: float) -> float:
        # log_scale(lo) and log_scale(hi) have opposite signs.
        for _ in range(200):
            mid = (lo + hi) / 2
            if (log_scale(mid) < 0) == (log_scale(lo) < 0):
                lo = mid
            else:
                hi = mid
        return math.degrees((lo + hi) / 2)

    edge = math.pi / 2 - 1e-9
    try:
        if log_scale(edge) <= 0 or log_scale(-edge) <= 0:
            return None
        return root(-edge, phi0), root(phi0, edge)
    except (ValueError, ZeroDivisionError):
        return None


def conversion_to_cf(conversion: CoordinateOperation, ellipsoid: Ellipsoid | None) -> JsonDict | None:
    """The grid mapping attributes of a projected CRS's conversion, or
    ``None`` for a conversion CF has none for."""
    method = conversion.method_name
    by_code: dict[int, Any] = {int(p.code): _cf_number(p) for p in conversion.params if p.code.isdigit()}
    by_name: dict[str, Any] = {p.name.lower(): _cf_number(p) for p in conversion.params}
    if method.lower().startswith("geostationary satellite (sweep "):
        return {
            "grid_mapping_name": "geostationary",
            "sweep_angle_axis": "x" if method.lower().endswith("(sweep x)") else "y",
            "perspective_point_height": by_name["satellite height"],
            "latitude_of_projection_origin": by_code.get(8801, 0.0),
            "longitude_of_projection_origin": by_code[8802],
            "false_easting": by_code[8806],
            "false_northing": by_code[8807],
        }
    code = conversion.method_code
    key: int | str = _METHOD_ALIASES.get(int(code), int(code)) if code.isdigit() else method
    mapping = next((m for m in _MAPPINGS if m.method == key), None)
    if mapping is None:
        return None
    if key == 9801 and by_code.get(8805, 1.0) != 1:
        # CF's cone with one parallel has no scale factor: one with a scale
        # under 1 is the secant cone through two, which CF has. (pyproj
        # writes the parallel and leaves the scale out: another projection.)
        secant = None if ellipsoid is None else _secant_parallels(by_code[8801], by_code[8805], ellipsoid)
        if secant is None:
            return None
        return {
            "grid_mapping_name": "lambert_conformal_conic",
            "standard_parallel": secant,
            "latitude_of_projection_origin": by_code[8801],
            "longitude_of_central_meridian": by_code[8802],
            "false_easting": by_code[8806],
            "false_northing": by_code[8807],
        }
    if key == 9815 and by_code.get(8814, 90.0) != 90:
        warnings.warn("angle from rectified to skew grid parameter lost in conversion to CF", stacklevel=3)
    out: JsonDict = {"grid_mapping_name": mapping.grid_mapping}
    parallels: list[Any] = []
    for attribute, pcode, _ in mapping.attributes:
        if pcode not in by_code or not attribute:
            continue
        if attribute.startswith("standard_parallel["):
            parallels.append(by_code[pcode])
        else:
            out[attribute] = by_code[pcode]
    if parallels:
        out["standard_parallel"] = tuple(parallels) if len(parallels) > 1 else parallels[0]
    return out


def _cf_number(p: Param) -> Any:
    """A parameter's value as CF has it: an angle in degrees (EPSG gives
    NTF's in grads, which pyproj writes as they are), else as it is."""
    if p.unit_category == "angular" and isinstance(p.value, (int, float)) and p.unit_conversion_factor:
        degrees = p.value * p.unit_conversion_factor / (math.pi / 180)
        # Degrees, or their binary rounding, as they are.
        return p.value if math.isclose(degrees, p.value, rel_tol=1e-12, abs_tol=1e-15) else degrees
    return p.value


def rotated_pole_to_cf(conversion: CoordinateOperation) -> JsonDict | None:
    """A rotated pole's attributes, from PROJ's ``ob_tran`` or CF's own."""
    method = conversion.method_name.lower()
    params = {p.name.lower(): _cf_number(p) for p in conversion.params}
    if method.startswith("proj ob_tran o_proj=") and method[len("proj ob_tran o_proj=") :] in (
        "longlat",
        "lonlat",
        "latlon",
        "latlong",
    ):
        return {
            "grid_mapping_name": "rotated_latitude_longitude",
            "grid_north_pole_latitude": params["o_lat_p"],
            # https://github.com/pyproj4/pyproj/issues/927
            "grid_north_pole_longitude": float(params["lon_0"]) - 180,
            "north_pole_grid_longitude": params["o_lon_p"],
        }
    if method == "pole rotation (netcdf cf convention)":
        return {
            "grid_mapping_name": "rotated_latitude_longitude",
            "grid_north_pole_latitude": params["grid north pole latitude (netcdf cf convention)"],
            "grid_north_pole_longitude": params["grid north pole longitude (netcdf cf convention)"],
            "north_pole_grid_longitude": params["north pole grid longitude (netcdf cf convention)"],
        }
    return None


def horizontal_datum(cf: dict[str, Any]) -> Datum | None:
    """The datum CF's attributes name or define, or ``None``."""
    name = cf.get("horizontal_datum_name")
    if name and name not in _UNKNOWN:
        try:
            return Datum.from_name(name)
        except CRSError:
            pass
    ellipsoid: Any = None
    ellipsoid_name = cf.get("reference_ellipsoid_name")
    try:
        ellipsoid = CustomEllipsoid(
            name=ellipsoid_name or "undefined",
            semi_major_axis=cf.get("semi_major_axis"),
            semi_minor_axis=cf.get("semi_minor_axis"),
            inverse_flattening=cf.get("inverse_flattening"),
            radius=cf.get("earth_radius"),
        )
    except CRSError:
        if ellipsoid_name and ellipsoid_name not in _UNKNOWN:
            ellipsoid = Ellipsoid.from_name(ellipsoid_name)
    meridian: Any = None
    meridian_name = cf.get("prime_meridian_name")
    if "longitude_of_prime_meridian" in cf:
        meridian = CustomPrimeMeridian(name=meridian_name or "undefined", longitude=cf["longitude_of_prime_meridian"])
    elif meridian_name and meridian_name not in _UNKNOWN:
        meridian = PrimeMeridian.from_name(meridian_name)
    if ellipsoid is None and meridian is None:
        return None
    return CustomDatum(name=name or "undefined", ellipsoid=ellipsoid or "WGS 84", prime_meridian=meridian or "Greenwich")


def pole_rotation(cf: dict[str, Any]) -> CoordinateOperation:
    """CF's rotated pole as its conversion."""
    grid = "rotated_latitude_longitude"
    return PoleRotationNetCDFCFConversion(
        grid_north_pole_latitude=_cf_value(cf, "grid_north_pole_latitude", _REQUIRED, grid),
        grid_north_pole_longitude=_cf_value(cf, "grid_north_pole_longitude", _REQUIRED, grid),
        north_pole_grid_longitude=_cf_value(cf, "north_pole_grid_longitude", 0.0, grid),
    )


def coordinate_system_to_cf(kind: str, axes: list[JsonDict], rotated_pole: bool) -> list[JsonDict]:
    """``CoordinateSystem.to_cf``: CF's attributes of each axis of a
    coordinate system of this kind (its lowercase subtype)."""
    out: list[JsonDict] = []

    def linear_unit(axis: JsonDict) -> Any:
        unit = axis.get("unit")
        return f"{unit['conversion_factor']} metre" if isinstance(unit, dict) else unit

    if kind == "cartesian":
        for axis in axes:
            letter = "X" if str(axis["name"]).lower() == "easting" else "Y"
            out.append(
                {
                    "axis": letter,
                    "long_name": axis["name"],
                    "standard_name": f"projection_{letter.lower()}_coordinate",
                    "units": linear_unit(axis),
                }
            )
    elif kind == "ellipsoidal":
        for axis in axes:
            if str(axis.get("abbreviation", "")).upper() in ("D", "H"):
                out.append(
                    {
                        "standard_name": "height_above_reference_ellipsoid",
                        "long_name": axis["name"],
                        "units": axis["unit"],
                        "positive": axis["direction"],
                        "axis": "Z",
                    }
                )
                continue
            letter, name = ("X", "longitude") if "longitude" in str(axis["name"]).lower() else ("Y", "latitude")
            if rotated_pole:
                out.append({"standard_name": f"grid_{name}", "long_name": f"{name} in rotated pole grid", "units": "degrees", "axis": letter})
            else:
                out.append(
                    {"standard_name": name, "long_name": f"{name} coordinate", "units": f"degrees_{axis['direction']}", "axis": letter}
                )
    elif kind == "vertical":
        # A vertical CRS's datum is a geopotential one (CF's
        # `geopotential_datum_name`): CF's height above it, or depth, which
        # CF has no name for below one but its own. (pyproj writes
        # `height_above_reference_ellipsoid`, another coordinate.)
        for axis in axes:
            down = axis.get("direction") == "down"
            out.append(
                {
                    "standard_name": "depth" if down else "height_above_geopotential_datum",
                    "long_name": axis["name"],
                    "units": linear_unit(axis),
                    "positive": axis["direction"],
                    "axis": "Z",
                }
            )
    return out


# pyproj's name, which its tests import.
_try_list_if_string = try_list_if_string
