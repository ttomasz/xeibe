"""Conversions and a transformation for building a CRS, as
``pyproj.crs.coordinate_operation``: pyproj 3.8.0's classes and arguments
(MIT, see NOTICE.md). Each conversion is EPSG's method and parameters
from the tables here (``_conversion``); angles are in degrees, lengths in
metres."""

from __future__ import annotations

import warnings
from typing import Any, Self

from xeibe_proj._json import JsonDict
from xeibe_proj.crs._conversion import FALSE as _FALSE
from xeibe_proj.crs._conversion import ORIGIN as _ORIGIN
from xeibe_proj.crs._conversion import conversion
from xeibe_proj.crs._objects import CoordinateOperation, PartBuilder, nested_json
from xeibe_proj.exceptions import CRSError

__all__ = [
    "AlbersEqualAreaConversion",
    "AzimuthalEquidistantConversion",
    "EquidistantCylindricalConversion",
    "GeostationarySatelliteConversion",
    "HotineObliqueMercatorBConversion",
    "LambertAzimuthalEqualAreaConversion",
    "LambertConformalConic1SPConversion",
    "LambertConformalConic2SPConversion",
    "LambertCylindricalEqualAreaConversion",
    "LambertCylindricalEqualAreaScaleConversion",
    "MercatorAConversion",
    "MercatorBConversion",
    "OrthographicConversion",
    "PlateCarreeConversion",
    "PolarStereographicAConversion",
    "PolarStereographicBConversion",
    "PoleRotationNetCDFCFConversion",
    "RotatedLatitudeLongitudeConversion",
    "SinusoidalConversion",
    "StereographicConversion",
    "ToWGS84Transformation",
    "TransverseMercatorConversion",
    "UTMConversion",
    "VerticalPerspectiveConversion",
]

class AlbersEqualAreaConversion(PartBuilder, CoordinateOperation):
    """Albers Equal Area (EPSG 9822, ``+proj=aea``)."""

    def __new__(
        cls,
        latitude_first_parallel: float,
        latitude_second_parallel: float,
        latitude_false_origin: float = 0.0,
        longitude_false_origin: float = 0.0,
        easting_false_origin: float = 0.0,
        northing_false_origin: float = 0.0,
    ) -> Self:
        """The parallels (``lat_1``, ``lat_2``), the false origin (``lat_0``,
        ``lon_0``) and its easting and northing (``x_0``, ``y_0``)."""
        return cls.from_json_dict(
            conversion(
                9822,
                {
                    8821: latitude_false_origin,
                    8822: longitude_false_origin,
                    8823: latitude_first_parallel,
                    8824: latitude_second_parallel,
                    8826: easting_false_origin,
                    8827: northing_false_origin,
                }.items(),
            )
        )


class AzimuthalEquidistantConversion(PartBuilder, CoordinateOperation):
    """Modified Azimuthal Equidistant (EPSG 9832, ``+proj=aeqd``)."""

    def __new__(
        cls,
        latitude_natural_origin: float = 0.0,
        longitude_natural_origin: float = 0.0,
        false_easting: float = 0.0,
        false_northing: float = 0.0,
    ) -> Self:
        """The natural origin (``lat_0``, ``lon_0``) and false easting and
        northing (``x_0``, ``y_0``)."""
        values = zip((*_ORIGIN, *_FALSE), (latitude_natural_origin, longitude_natural_origin, false_easting, false_northing))
        return cls.from_json_dict(conversion(9832, values))


class GeostationarySatelliteConversion(PartBuilder, CoordinateOperation):
    """Geostationary Satellite (PROJ's, ``+proj=geos``)."""

    def __new__(
        cls,
        sweep_angle_axis: str,
        satellite_height: float,
        latitude_natural_origin: float = 0.0,
        longitude_natural_origin: float = 0.0,
        false_easting: float = 0.0,
        false_northing: float = 0.0,
    ) -> Self:
        """The sweep angle axis, ``X`` or ``Y`` (``sweep``), the satellite's
        height (``h``), the natural origin (``lat_0``, ``lon_0``) and false
        easting and northing (``x_0``, ``y_0``). The latitude of origin is
        only written out: PROJ doesn't use it."""
        sweep = sweep_angle_axis.strip().upper()
        if sweep not in ("X", "Y"):
            raise CRSError("sweep_angle_axis only supports ('X', 'Y')")
        if latitude_natural_origin != 0:
            warnings.warn(
                "The latitude of natural origin (lat_0) is not used within PROJ. "
                "It is only supported for exporting to the WKT or PROJ JSON formats.",
                stacklevel=2,
            )
        origin = zip((*_ORIGIN, *_FALSE), (latitude_natural_origin, longitude_natural_origin, false_easting, false_northing))
        values: list[tuple[int | tuple[str, str], float]] = [(("Satellite height", "metre"), satellite_height), *origin]
        return cls.from_json_dict(conversion(f"Geostationary Satellite (Sweep {sweep})", values))


class LambertAzimuthalEqualAreaConversion(PartBuilder, CoordinateOperation):
    """Lambert Azimuthal Equal Area (EPSG 9820, ``+proj=laea``)."""

    def __new__(
        cls,
        latitude_natural_origin: float = 0.0,
        longitude_natural_origin: float = 0.0,
        false_easting: float = 0.0,
        false_northing: float = 0.0,
    ) -> Self:
        """The natural origin (``lat_0``, ``lon_0``) and false easting and
        northing (``x_0``, ``y_0``)."""
        values = zip((*_ORIGIN, *_FALSE), (latitude_natural_origin, longitude_natural_origin, false_easting, false_northing))
        return cls.from_json_dict(conversion(9820, values))


class LambertConformalConic2SPConversion(PartBuilder, CoordinateOperation):
    """Lambert Conic Conformal (2SP) (EPSG 9802, ``+proj=lcc``)."""

    def __new__(
        cls,
        latitude_first_parallel: float,
        latitude_second_parallel: float,
        latitude_false_origin: float = 0.0,
        longitude_false_origin: float = 0.0,
        easting_false_origin: float = 0.0,
        northing_false_origin: float = 0.0,
    ) -> Self:
        """The parallels (``lat_1``, ``lat_2``), the false origin (``lat_0``,
        ``lon_0``) and its easting and northing (``x_0``, ``y_0``)."""
        return cls.from_json_dict(
            conversion(
                9802,
                {
                    8823: latitude_first_parallel,
                    8824: latitude_second_parallel,
                    8821: latitude_false_origin,
                    8822: longitude_false_origin,
                    8826: easting_false_origin,
                    8827: northing_false_origin,
                }.items(),
            )
        )


def _scaled(method: int | str, latitude: float, longitude: float, scale: float, easting: float, northing: float) -> JsonDict:
    """A conversion with a natural origin, a scale factor at it, and false
    easting and northing, in EPSG's order."""
    return conversion(method, {8801: latitude, 8802: longitude, 8805: scale, 8806: easting, 8807: northing}.items())


class LambertConformalConic1SPConversion(PartBuilder, CoordinateOperation):
    """Lambert Conic Conformal (1SP) (EPSG 9801, ``+proj=lcc``)."""

    def __new__(
        cls,
        latitude_natural_origin: float = 0.0,
        longitude_natural_origin: float = 0.0,
        false_easting: float = 0.0,
        false_northing: float = 0.0,
        scale_factor_natural_origin: float = 1.0,
    ) -> Self:
        """The natural origin (``lat_0``, ``lon_0``), false easting and
        northing (``x_0``, ``y_0``) and the scale factor (``k_0``)."""
        return cls.from_json_dict(
            _scaled(9801, latitude_natural_origin, longitude_natural_origin, scale_factor_natural_origin, false_easting, false_northing)
        )


class LambertCylindricalEqualAreaConversion(PartBuilder, CoordinateOperation):
    """Lambert Cylindrical Equal Area (EPSG 9835, ``+proj=cea``)."""

    def __new__(
        cls,
        latitude_first_parallel: float = 0.0,
        longitude_natural_origin: float = 0.0,
        false_easting: float = 0.0,
        false_northing: float = 0.0,
    ) -> Self:
        """The standard parallel (``lat_ts``), the central meridian
        (``lon_0``) and false easting and northing (``x_0``, ``y_0``)."""
        values = zip((8823, 8802, *_FALSE), (latitude_first_parallel, longitude_natural_origin, false_easting, false_northing))
        return cls.from_json_dict(conversion(9835, values))


class LambertCylindricalEqualAreaScaleConversion(PartBuilder, CoordinateOperation):
    """Lambert Cylindrical Equal Area by a scale factor (``+proj=cea
    +k_0``): the conversion PROJ reads from the PROJ string, as in pyproj,
    with the standard parallel PROJ computes from the scale factor."""

    def __new__(
        cls,
        longitude_natural_origin: float = 0.0,
        false_easting: float = 0.0,
        false_northing: float = 0.0,
        scale_factor_natural_origin: float = 1.0,
    ) -> Self:
        """The central meridian (``lon_0``), false easting and northing
        (``x_0``, ``y_0``) and the scale factor (``k_0``)."""
        from xeibe_proj.crs.crs import CRS

        crs = CRS(
            f"+proj=cea +lon_0={longitude_natural_origin} +x_0={false_easting} "
            f"+y_0={false_northing} +k_0={scale_factor_natural_origin}"
        )
        operation = crs.coordinate_operation
        if operation is None:
            raise CRSError("+proj=cea has no conversion")
        return cls.from_json(operation.to_json())


class MercatorAConversion(PartBuilder, CoordinateOperation):
    """Mercator (variant A) (EPSG 9804, ``+proj=merc``)."""

    def __new__(
        cls,
        latitude_natural_origin: float = 0.0,
        longitude_natural_origin: float = 0.0,
        false_easting: float = 0.0,
        false_northing: float = 0.0,
        scale_factor_natural_origin: float = 1.0,
    ) -> Self:
        """The natural origin (``lat_0``, which EPSG defines as 0, and
        ``lon_0``), false easting and northing (``x_0``, ``y_0``) and the
        scale factor (``k_0``)."""
        if latitude_natural_origin != 0:
            raise CRSError("This conversion is defined for only latitude_natural_origin = 0.")
        return cls.from_json_dict(
            _scaled(9804, latitude_natural_origin, longitude_natural_origin, scale_factor_natural_origin, false_easting, false_northing)
        )


class MercatorBConversion(PartBuilder, CoordinateOperation):
    """Mercator (variant B) (EPSG 9805, ``+proj=merc``)."""

    def __new__(
        cls,
        latitude_first_parallel: float = 0.0,
        longitude_natural_origin: float = 0.0,
        false_easting: float = 0.0,
        false_northing: float = 0.0,
    ) -> Self:
        """The standard parallel (``lat_ts``), the central meridian
        (``lon_0``) and false easting and northing (``x_0``, ``y_0``)."""
        values = zip((8823, 8802, *_FALSE), (latitude_first_parallel, longitude_natural_origin, false_easting, false_northing))
        return cls.from_json_dict(conversion(9805, values))


class HotineObliqueMercatorBConversion(PartBuilder, CoordinateOperation):
    """Hotine Oblique Mercator (variant B) (EPSG 9815, ``+proj=omerc``)."""

    def __new__(
        cls,
        latitude_projection_centre: float,
        longitude_projection_centre: float,
        angle_from_rectified_to_skew_grid: float,
        easting_projection_centre: float = 0.0,
        northing_projection_centre: float = 0.0,
        azimuth_projection_centre: float | None = None,
        scale_factor_projection_centre: float | None = None,
        azimuth_initial_line: float | None = None,
        scale_factor_on_initial_line: float | None = None,
    ) -> Self:
        """The projection centre (``lat_0``, ``lonc``), the azimuth there
        (``alpha``), the angle from the rectified to the skew grid
        (``gamma``), the scale factor at the centre (``k``) and its easting
        and northing (``x_0``, ``y_0``). ``azimuth_initial_line`` and
        ``scale_factor_on_initial_line`` are the old names of the azimuth
        and scale factor, which warn."""
        if scale_factor_on_initial_line is not None:
            if scale_factor_projection_centre is not None:
                raise ValueError(
                    "scale_factor_projection_centre and scale_factor_on_initial_line cannot be provided together."
                )
            warnings.warn(
                "scale_factor_on_initial_line is deprecated. Use scale_factor_projection_centre instead.",
                FutureWarning,
                stacklevel=2,
            )
            scale_factor_projection_centre = scale_factor_on_initial_line
        if azimuth_initial_line is not None:
            if azimuth_projection_centre is not None:
                raise ValueError("azimuth_projection_centre and azimuth_initial_line cannot be provided together.")
            warnings.warn(
                "azimuth_initial_line is deprecated. Use azimuth_projection_centre instead.",
                FutureWarning,
                stacklevel=2,
            )
            azimuth_projection_centre = azimuth_initial_line
        if azimuth_projection_centre is None:
            raise ValueError("azimuth_projection_centre or azimuth_initial_line must be provided.")
        return cls.from_json_dict(
            conversion(
                9815,
                {
                    8811: latitude_projection_centre,
                    8812: longitude_projection_centre,
                    8813: azimuth_projection_centre,
                    8814: angle_from_rectified_to_skew_grid,
                    8815: 1.0 if scale_factor_projection_centre is None else scale_factor_projection_centre,
                    8816: easting_projection_centre,
                    8817: northing_projection_centre,
                }.items(),
            )
        )


class OrthographicConversion(PartBuilder, CoordinateOperation):
    """Orthographic (EPSG 9840, ``+proj=ortho``)."""

    def __new__(
        cls,
        latitude_natural_origin: float = 0.0,
        longitude_natural_origin: float = 0.0,
        false_easting: float = 0.0,
        false_northing: float = 0.0,
    ) -> Self:
        """The natural origin (``lat_0``, ``lon_0``) and false easting and
        northing (``x_0``, ``y_0``)."""
        values = zip((*_ORIGIN, *_FALSE), (latitude_natural_origin, longitude_natural_origin, false_easting, false_northing))
        return cls.from_json_dict(conversion(9840, values))


class PolarStereographicAConversion(PartBuilder, CoordinateOperation):
    """Polar Stereographic (variant A) (EPSG 9810, ``+proj=stere``)."""

    def __new__(
        cls,
        latitude_natural_origin: float,
        longitude_natural_origin: float = 0.0,
        false_easting: float = 0.0,
        false_northing: float = 0.0,
        scale_factor_natural_origin: float = 1.0,
    ) -> Self:
        """The pole (``lat_0``, 90 or -90), the central meridian (``lon_0``),
        false easting and northing (``x_0``, ``y_0``) and the scale factor
        (``k_0``)."""
        return cls.from_json_dict(
            _scaled(9810, latitude_natural_origin, longitude_natural_origin, scale_factor_natural_origin, false_easting, false_northing)
        )


class PolarStereographicBConversion(PartBuilder, CoordinateOperation):
    """Polar Stereographic (variant B) (EPSG 9829, ``+proj=stere``)."""

    def __new__(
        cls,
        latitude_standard_parallel: float = 0.0,
        longitude_origin: float = 0.0,
        false_easting: float = 0.0,
        false_northing: float = 0.0,
    ) -> Self:
        """The standard parallel (``lat_ts``), the central meridian
        (``lon_0``) and false easting and northing (``x_0``, ``y_0``)."""
        values = zip((8832, 8833, *_FALSE), (latitude_standard_parallel, longitude_origin, false_easting, false_northing))
        return cls.from_json_dict(conversion(9829, values))


class SinusoidalConversion(PartBuilder, CoordinateOperation):
    """Sinusoidal (PROJ's, ``+proj=sinu``)."""

    def __new__(cls, longitude_natural_origin: float = 0.0, false_easting: float = 0.0, false_northing: float = 0.0) -> Self:
        """The central meridian (``lon_0``) and false easting and northing
        (``x_0``, ``y_0``)."""
        values = zip((8802, *_FALSE), (longitude_natural_origin, false_easting, false_northing))
        return cls.from_json_dict(conversion("Sinusoidal", values))


class StereographicConversion(PartBuilder, CoordinateOperation):
    """Stereographic (PROJ's, ``+proj=stere``)."""

    def __new__(
        cls,
        latitude_natural_origin: float = 0.0,
        longitude_natural_origin: float = 0.0,
        false_easting: float = 0.0,
        false_northing: float = 0.0,
        scale_factor_natural_origin: float = 1.0,
    ) -> Self:
        """The natural origin (``lat_0``, ``lon_0``), false easting and
        northing (``x_0``, ``y_0``) and the scale factor (``k_0``)."""
        return cls.from_json_dict(
            _scaled("Stereographic", latitude_natural_origin, longitude_natural_origin, scale_factor_natural_origin, false_easting, false_northing)
        )


class UTMConversion(PartBuilder, CoordinateOperation):
    """A UTM zone: EPSG's conversion of that name."""

    def __new__(cls, zone: str, hemisphere: str = "N") -> Self:
        """The zone (1 to 60) and hemisphere (``N`` or ``S``)."""
        return cls.from_name(f"UTM zone {zone}{hemisphere}")


class TransverseMercatorConversion(PartBuilder, CoordinateOperation):
    """Transverse Mercator (EPSG 9807, ``+proj=tmerc``)."""

    def __new__(
        cls,
        latitude_natural_origin: float = 0.0,
        longitude_natural_origin: float = 0.0,
        false_easting: float = 0.0,
        false_northing: float = 0.0,
        scale_factor_natural_origin: float = 1.0,
    ) -> Self:
        """The natural origin (``lat_0``, ``lon_0``), false easting and
        northing (``x_0``, ``y_0``) and the scale factor (``k_0``)."""
        return cls.from_json_dict(
            _scaled(9807, latitude_natural_origin, longitude_natural_origin, scale_factor_natural_origin, false_easting, false_northing)
        )


class VerticalPerspectiveConversion(PartBuilder, CoordinateOperation):
    """Vertical Perspective (EPSG 9838, ``+proj=nsper``)."""

    def __new__(
        cls,
        viewpoint_height: float,
        latitude_topocentric_origin: float = 0.0,
        longitude_topocentric_origin: float = 0.0,
        ellipsoidal_height_topocentric_origin: float = 0.0,
        false_easting: float = 0.0,
        false_northing: float = 0.0,
    ) -> Self:
        """The viewpoint's height (``h``), the topocentric origin (``lat_0``,
        ``lon_0``) and its ellipsoidal height, and false easting and
        northing (``x_0``, ``y_0``)."""
        values = {
            8834: latitude_topocentric_origin,
            8835: longitude_topocentric_origin,
            8836: ellipsoidal_height_topocentric_origin,
            8840: viewpoint_height,
            8806: false_easting,
            8807: false_northing,
        }
        return cls.from_json_dict(conversion(9838, values.items()))


class RotatedLatitudeLongitudeConversion(PartBuilder, CoordinateOperation):
    """A rotated pole (PROJ's ``+proj=ob_tran +o_proj=longlat``)."""

    def __new__(cls, o_lat_p: float, o_lon_p: float, lon_0: float = 0.0) -> Self:
        """The new pole's latitude and longitude (``o_lat_p``, ``o_lon_p``)
        and the rotation about it (``lon_0``)."""
        values = {(k, "degree"): v for k, v in (("o_lat_p", o_lat_p), ("o_lon_p", o_lon_p), ("lon_0", lon_0))}
        return cls.from_json_dict(conversion("PROJ ob_tran o_proj=longlat", values.items()))


class PoleRotationNetCDFCFConversion(PartBuilder, CoordinateOperation):
    """A rotated pole as netCDF's CF conventions give it."""

    def __new__(
        cls,
        grid_north_pole_latitude: float,
        grid_north_pole_longitude: float,
        north_pole_grid_longitude: float = 0.0,
    ) -> Self:
        """The grid's north pole and the north pole's grid longitude
        (``grid_north_pole_latitude``, ``grid_north_pole_longitude`` and
        ``north_pole_grid_longitude`` in CF)."""
        name = "Pole rotation (netCDF CF convention)"
        values = {
            ("Grid north pole latitude (netCDF CF convention)", "degree"): grid_north_pole_latitude,
            ("Grid north pole longitude (netCDF CF convention)", "degree"): grid_north_pole_longitude,
            ("North pole grid longitude (netCDF CF convention)", "degree"): north_pole_grid_longitude,
        }
        return cls.from_json_dict(conversion(name, values.items(), name))


class EquidistantCylindricalConversion(PartBuilder, CoordinateOperation):
    """Equidistant Cylindrical (EPSG 1028, ``+proj=eqc``), also
    ``PlateCarreeConversion``."""

    def __new__(
        cls,
        latitude_first_parallel: float = 0.0,
        latitude_natural_origin: float = 0.0,
        longitude_natural_origin: float = 0.0,
        false_easting: float = 0.0,
        false_northing: float = 0.0,
    ) -> Self:
        """The standard parallel (``lat_ts``), the natural origin (``lat_0``,
        ``lon_0``) and false easting and northing (``x_0``, ``y_0``)."""
        values = zip(
            (8823, *_ORIGIN, *_FALSE),
            (latitude_first_parallel, latitude_natural_origin, longitude_natural_origin, false_easting, false_northing),
        )
        return cls.from_json_dict(conversion(1028, values))


PlateCarreeConversion = EquidistantCylindricalConversion

_ARC_SECOND: JsonDict = {"type": "AngularUnit", "name": "arc-second", "conversion_factor": 4.84813681109536e-06}
_PPM: JsonDict = {"type": "ScaleUnit", "name": "parts per million", "conversion_factor": 1e-06}


class ToWGS84Transformation(PartBuilder, CoordinateOperation):
    """A transformation to WGS 84 by seven Position Vector parameters, as
    ``+towgs84`` gives them: a Bound CRS's."""

    def __new__(
        cls,
        source_crs: Any,
        x_axis_translation: float = 0,
        y_axis_translation: float = 0,
        z_axis_translation: float = 0,
        x_axis_rotation: float = 0,
        y_axis_rotation: float = 0,
        z_axis_rotation: float = 0,
        scale_difference: float = 0,
    ) -> Self:
        """The CRS transformed from (what :meth:`CRS.from_user_input`
        takes), the translations in metres, the rotations in arc-seconds
        and the scale difference in parts per million."""
        from xeibe_proj.crs.coordinate_system import Ellipsoidal2DCS
        from xeibe_proj.crs.crs import CRS
        from xeibe_proj.crs.enums import Ellipsoidal2DCSAxis

        wgs84: JsonDict = {
            "type": "GeographicCRS",
            "name": "WGS 84",
            "datum": {
                "type": "GeodeticReferenceFrame",
                "name": "World Geodetic System 1984",
                "ellipsoid": {"name": "WGS 84", "semi_major_axis": 6378137, "inverse_flattening": 298.257223563},
            },
            "coordinate_system": nested_json(Ellipsoidal2DCS(Ellipsoidal2DCSAxis.LATITUDE_LONGITUDE)),
            "id": {"authority": "EPSG", "code": 4326},
        }
        units = ("metre", "metre", "metre", _ARC_SECOND, _ARC_SECOND, _ARC_SECOND, _PPM)
        names = (
            "X-axis translation",
            "Y-axis translation",
            "Z-axis translation",
            "X-axis rotation",
            "Y-axis rotation",
            "Z-axis rotation",
            "Scale difference",
        )
        values = (
            x_axis_translation,
            y_axis_translation,
            z_axis_translation,
            x_axis_rotation,
            y_axis_rotation,
            z_axis_rotation,
            scale_difference,
        )
        return cls.from_json_dict(
            {
                "type": "Transformation",
                "name": "Transformation from unknown to WGS84",
                "source_crs": nested_json(CRS.from_user_input(source_crs)),
                "target_crs": wgs84,
                "method": {
                    "name": "Position Vector transformation (geog2D domain)",
                    "id": {"authority": "EPSG", "code": 9606},
                },
                "parameters": [
                    {"name": n, "value": v, "unit": u, "id": {"authority": "EPSG", "code": 8605 + i}}
                    for i, (n, v, u) in enumerate(zip(names, values, units))
                ],
            }
        )
