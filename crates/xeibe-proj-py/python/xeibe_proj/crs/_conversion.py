"""EPSG's methods and parameters of the conversions the builders and CF
make (``coordinate_operation``, ``_cf1x8``), and the PROJJSON of one."""

from __future__ import annotations

from collections.abc import Iterable

from xeibe_proj._json import JsonDict

# EPSG's methods the conversions use, by code.
METHODS = {
    1028: "Equidistant Cylindrical",
    9801: "Lambert Conic Conformal (1SP)",
    9802: "Lambert Conic Conformal (2SP)",
    9804: "Mercator (variant A)",
    9805: "Mercator (variant B)",
    9807: "Transverse Mercator",
    9810: "Polar Stereographic (variant A)",
    9815: "Hotine Oblique Mercator (variant B)",
    9820: "Lambert Azimuthal Equal Area",
    9822: "Albers Equal Area",
    9829: "Polar Stereographic (variant B)",
    9832: "Modified Azimuthal Equidistant",
    9835: "Lambert Cylindrical Equal Area",
    9838: "Vertical Perspective",
    9840: "Orthographic",
}

# EPSG's parameters, by code: name and unit.
PARAMETERS = {
    8801: ("Latitude of natural origin", "degree"),
    8802: ("Longitude of natural origin", "degree"),
    8805: ("Scale factor at natural origin", "unity"),
    8806: ("False easting", "metre"),
    8807: ("False northing", "metre"),
    8811: ("Latitude of projection centre", "degree"),
    8812: ("Longitude of projection centre", "degree"),
    8813: ("Azimuth at projection centre", "degree"),
    8814: ("Angle from Rectified to Skew Grid", "degree"),
    8815: ("Scale factor at projection centre", "unity"),
    8816: ("Easting at projection centre", "metre"),
    8817: ("Northing at projection centre", "metre"),
    8821: ("Latitude of false origin", "degree"),
    8822: ("Longitude of false origin", "degree"),
    8823: ("Latitude of 1st standard parallel", "degree"),
    8824: ("Latitude of 2nd standard parallel", "degree"),
    8826: ("Easting at false origin", "metre"),
    8827: ("Northing at false origin", "metre"),
    8832: ("Latitude of standard parallel", "degree"),
    8833: ("Longitude of origin", "degree"),
    8834: ("Latitude of topocentric origin", "degree"),
    8835: ("Longitude of topocentric origin", "degree"),
    8836: ("Ellipsoidal height of topocentric origin", "metre"),
    8840: ("Viewpoint height", "metre"),
}

# The natural origin's, the most common.
ORIGIN = (8801, 8802)
FALSE = (8806, 8807)


def conversion(method: int | str, values: Iterable[tuple[int | tuple[str, str], float]], name: str = "unknown") -> JsonDict:
    """A conversion of EPSG's method (by code) or PROJ's (by name), with
    parameters in the order given: EPSG's by code, others as their
    ``(name, unit)``."""
    out: JsonDict = {"type": "Conversion", "name": name}
    out["method"] = (
        {"name": method}
        if isinstance(method, str)
        else {"name": METHODS[method], "id": {"authority": "EPSG", "code": method}}
    )
    parameters: list[JsonDict] = []
    for key, value in values:
        if isinstance(key, int):
            pname, unit = PARAMETERS[key]
            parameters.append({"name": pname, "value": value, "unit": unit, "id": {"authority": "EPSG", "code": key}})
        else:
            parameters.append({"name": key[0], "value": value, "unit": key[1]})
    out["parameters"] = parameters
    return out
