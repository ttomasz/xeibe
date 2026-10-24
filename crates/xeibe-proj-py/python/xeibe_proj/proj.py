"""``Proj``: pyproj's legacy interface to a CRS's projection.

``Proj(crs)(lon, lat)`` projects longitude and latitude, in degrees from
Greenwich, with the CRS's map projection on its own ellipsoid: no datum
change, the output in the CRS's own unit and axis directions, easting
first. ``inverse=True`` goes back. That is what pyproj gets from the CRS's
PROJ string (``docs/pyproj-phase2.md``, ``Proj``). ``Proj`` is a
``Transformer``, so ``transform``, ``itransform`` and ``transform_bounds``
work too.

Differences from pyproj (``docs/pyproj-compat.md``):

- A point that can't be projected raises ``ProjError``; there is no
  ``errcheck``, and ``radians`` is keyword-only in ``__call__``.
- EPSG's parameters are used at full precision, where pyproj's PROJ string
  rounds them to 15 digits, and EPSG's prime meridians, where PROJ's names
  ``+pm=paris`` and the like; results differ by about 1e-6 of the unit.
- ``srs`` is the CRS's ``srs``, not a PROJ string, and the parts that are
  PROJ strings (``Proj(proj="utm", …)``, ``definition_string``,
  ``to_latlong_def``) come with phase 3.
"""

from __future__ import annotations

import threading
from typing import Any

from xeibe_proj import _native, _operation_json
from xeibe_proj._unsupported import not_yet, out
from xeibe_proj.aoi import AreaOfUse
from xeibe_proj.crs import CRS
from xeibe_proj.network import is_network_enabled
from xeibe_proj.transformer import Transformer, _reject  # pyright: ignore[reportPrivateUsage]

__all__ = ["Proj"]


class Proj(Transformer):
    """The projection of a CRS (anything ``CRS.from_user_input`` takes).

    ``preserve_units=False`` gives metres for a CRS in feet, as pyproj
    does."""

    crs: CRS
    srs: str

    def __init__(self, projparams: Any = None, preserve_units: bool = True, **kwargs: Any) -> None:
        self.crs = CRS.from_user_input(projparams, **kwargs)
        axes = self.crs.axis_info
        self._metres = not preserve_units and bool(axes) and "foot" in axes[0].unit_name
        self.srs = self.crs.srs
        native = _native.Projection(self.crs._native_input(), self._metres)  # pyright: ignore[reportPrivateUsage]
        self._source = self.crs
        self._target = self.crs
        self._always_xy = True
        self._network = is_network_enabled()
        self._swap = (False, False)
        self._forward = native
        self._member = None
        self._backward = None
        self._logged = set()
        self._last = threading.local()

    def __call__(
        self,
        longitude: Any,
        latitude: Any,
        inverse: bool = False,
        *args: Any,
        radians: bool = False,
        **kwargs: Any,
    ) -> tuple[Any, Any]:
        """Project longitudes and latitudes in degrees (radians with
        ``radians=True``), or with ``inverse=True`` the reverse."""
        if args or kwargs:
            _reject("Proj.__call__", args, kwargs)
        x, y = self.transform(longitude, latitude, radians=radians, direction="INVERSE" if inverse else "FORWARD")
        return x, y

    def _angular(self, forward: bool) -> tuple[bool, bool]:
        # Degrees on the geographic side, and on the CRS's when it is
        # geographic, as PROJ's angular input and output.
        native = self._forward
        crs = isinstance(native, _native.Projection) and native.crs_is_angular
        return (True, crs) if forward else (crs, True)

    # pyproj's Proj is a transformer from a PROJ pipeline, without CRSs.
    @property
    def source_crs(self) -> None:  # type: ignore[override]
        return None

    @property
    def target_crs(self) -> None:  # type: ignore[override]
        return None

    @property
    def name(self) -> str:
        """The CRS's projection method, as EPSG names it (pyproj gives PROJ's
        short name, ``"tmerc"``)."""
        operation = self.crs.coordinate_operation
        if operation is not None and self.crs.is_projected:
            return operation.method_name
        return "longlat" if self.crs.is_geographic else "geocent"

    @property
    def description(self) -> str:
        operation = self.crs.coordinate_operation
        if operation is not None and self.crs.is_projected:
            return f"Projection: {operation.name}"
        return f"Projection: {self.crs.name}"

    @property
    def accuracy(self) -> float:
        return 0.0

    @property
    def area_of_use(self) -> AreaOfUse | None:
        return self.crs.area_of_use

    def get_last_used_operation(self) -> Transformer:
        return self

    def _operation_json(self) -> dict[str, Any] | None:
        # The CRS's projection; nothing for a geographic or geocentric CRS.
        conversion = _operation_json.projection(self.crs.to_json_dict(), False)
        return None if conversion is None else {"$schema": _operation_json.SCHEMA, **conversion}

    def definition_string(self) -> str:
        raise not_yet("Proj.definition_string", 3)

    def to_latlong_def(self) -> str | None:
        raise not_yet("Proj.to_latlong_def", 3)

    def to_latlong(self) -> Proj:
        """The ``Proj`` of the CRS's geodetic CRS."""
        return Proj(self.crs.geodetic_crs)

    def get_factors(self, longitude: Any, latitude: Any, radians: bool = False, errcheck: bool = False) -> Any:
        raise out("Proj.get_factors", "PROJ's projection factors aren't computed")

    def __eq__(self, other: object) -> bool:
        return isinstance(other, Proj) and self.crs == other.crs and self._metres == other._metres

    def __hash__(self) -> int:
        return hash((self.crs, self._metres))

    def is_exact_same(self, other: Any) -> bool:
        return isinstance(other, Proj) and self.crs.is_exact_same(other.crs) and self._metres == other._metres

    def __reduce__(self) -> tuple[Any, tuple[Any, ...]]:
        return Proj, (self.crs.srs, not self._metres)

    def __str__(self) -> str:
        return self.srs

    def __repr__(self) -> str:
        area = self.area_of_use
        return (
            f"<Projection Transformer: {self.name}>\n"
            f"Description: {self.description}\n"
            f"Area of Use:\n- {'undefined' if area is None else area.name or 'undefined'}"
        )
