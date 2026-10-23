"""Checked by mypy and pyright, never run: the overloads give these types."""

from __future__ import annotations

from array import array
from typing import Any, assert_type

import numpy as np
import numpy.typing as npt

from xeibe_proj import CRS, Transformer
from xeibe_proj.aoi import AreaOfUse
from xeibe_proj.crs import AxisInfo

t = Transformer.from_crs(4326, 2180, always_xy=True)

assert_type(t.transform(21.0, 52.0), tuple[float, float])
assert_type(t.transform(21, 52), tuple[float, float])
assert_type(t.transform(21.0, 52.0, 100.0), tuple[float, float, float])
assert_type(t.transform(21.0, 52.0, 100.0, 2020.0), tuple[float, float, float, float])
assert_type(t.transform([21.0], [52.0]), tuple[list[float], list[float]])
assert_type(t.transform((21.0,), (52.0,)), tuple[tuple[float, ...], tuple[float, ...]])
assert_type(t.transform(array("d", [21.0]), array("d", [52.0])), tuple["array[float]", "array[float]"])
xs: npt.NDArray[np.float64] = np.array([21.0])
assert_type(t.transform(xs, xs), tuple[npt.NDArray[np.floating[Any]], npt.NDArray[np.floating[Any]]])
assert_type(t.transform(21.0, 52.0, direction="INVERSE"), tuple[float, float])
assert_type(t.transform_bounds(14.0, 49.0, 24.0, 55.0), tuple[float, float, float, float])
for point in t.itransform([(21.0, 52.0)]):
    assert_type(point, tuple[float, ...])

crs = CRS.from_user_input("EPSG:2180")
assert_type(crs, CRS)
assert_type(CRS.from_epsg(2180), CRS)
assert_type(crs.to_epsg(), int | None)
assert_type(crs.axis_info, list[AxisInfo])
assert_type(crs.area_of_use, AreaOfUse | None)
assert_type(crs.to_json_dict(), dict[str, Any])

# Not in the signatures: rejected by the type checker as well as at runtime.
t.transform(21.0, 52.0, errcheck=True)  # type: ignore[call-overload]  # pyright: ignore[reportCallIssue]
t.transform(21.0, 52.0, inplace=True)  # type: ignore[call-overload]  # pyright: ignore[reportCallIssue]

from xeibe_proj import Geod  # noqa: E402

g = Geod(ellps="WGS84")
assert_type(g.inv(0.0, 0.0, 1.0, 1.0), tuple[Any, Any, Any])
assert_type(g.polygon_area_perimeter([0.0, 1.0, 1.0], [0.0, 0.0, 1.0]), tuple[float, float])
assert_type(crs.get_geod(), Geod | None)
g.fwd(0.0, 0.0, 90.0, 1.0, inplace=True)  # type: ignore[call-arg]  # pyright: ignore[reportCallIssue]
