"""Datums, ellipsoids and prime meridians for building a CRS, as
``pyproj.crs.datum``: pyproj 3.8.0's classes and arguments (MIT, see
NOTICE.md)."""

from __future__ import annotations

from typing import Any, Self

from xeibe_proj._json import JsonDict
from xeibe_proj.crs._objects import Datum, Ellipsoid, PrimeMeridian, PartBuilder, nested_json
from xeibe_proj.exceptions import CRSError

__all__ = ["CustomDatum", "CustomEllipsoid", "CustomPrimeMeridian", "Datum", "Ellipsoid", "PrimeMeridian"]


class CustomDatum(PartBuilder, Datum):
    """A geodetic datum of an ellipsoid and a prime meridian."""

    def __new__(cls, name: str = "undefined", ellipsoid: Any = "WGS 84", prime_meridian: Any = "Greenwich") -> Self:
        """
        Parameters
        ----------
        name: str, default="undefined"
            The datum's name.
        ellipsoid: Any, default="WGS 84"
            What :meth:`Ellipsoid.from_user_input` takes, or a
            :class:`CustomEllipsoid`.
        prime_meridian: Any, default="Greenwich"
            What :meth:`PrimeMeridian.from_user_input` takes.
        """
        doc: JsonDict = {
            "type": "GeodeticReferenceFrame",
            "name": name,
            "ellipsoid": nested_json(Ellipsoid.from_user_input(ellipsoid)),
        }
        pm = PrimeMeridian.from_user_input(prime_meridian)
        # PROJ leaves Greenwich out, but not another meridian at 0°.
        if pm.name != "Greenwich" or pm.longitude != 0:
            doc["prime_meridian"] = nested_json(pm)
        return cls.from_json_dict(doc)


class CustomEllipsoid(PartBuilder, Ellipsoid):
    """An ellipsoid by its semi-major axis and inverse flattening or
    semi-minor axis, or a sphere by its radius, in metres."""

    def __new__(
        cls,
        name: str = "undefined",
        semi_major_axis: float | None = None,
        inverse_flattening: float | None = None,
        semi_minor_axis: float | None = None,
        radius: float | None = None,
    ) -> Self:
        """
        Parameters
        ----------
        name: str, default="undefined"
            The ellipsoid's name.
        semi_major_axis: float, optional
            Needed unless ``radius`` is given.
        inverse_flattening: float, optional
            Needed unless ``semi_minor_axis`` or ``radius`` is given.
        semi_minor_axis: float, optional
            Needed unless ``inverse_flattening`` or ``radius`` is given.
        radius: float, optional
            A sphere's radius, alone.
        """
        doc: JsonDict = {"type": "Ellipsoid", "name": name}
        given = {
            "semi_major_axis": semi_major_axis,
            "inverse_flattening": inverse_flattening,
            "semi_minor_axis": semi_minor_axis,
            "radius": radius,
        }
        doc.update({k: v for k, v in given.items() if v is not None})
        if radius is None and (semi_major_axis is None or (inverse_flattening is None and semi_minor_axis is None)):
            raise CRSError(f"Invalid ellipsoid: {doc}")
        return cls.from_json_dict(doc)


class CustomPrimeMeridian(PartBuilder, PrimeMeridian):
    """A prime meridian by its longitude in degrees."""

    def __new__(cls, longitude: float, name: str = "undefined") -> Self:
        """
        Parameters
        ----------
        longitude: float
            Degrees east of Greenwich.
        name: str, optional
            The prime meridian's name.
        """
        return cls.from_json_dict({"type": "PrimeMeridian", "name": name, "longitude": longitude})
