"""Types shared by the annotations. Imported only while type checking, so
neither numpy nor typing_extensions is needed at runtime."""

from __future__ import annotations

from array import array
from typing import Any, Protocol, TypeAlias, TypeVar

import numpy as np
import numpy.typing as npt

from xeibe_proj.enums import TransformDirection

__all__ = ["ArrayLike", "Coordinates", "Direction", "FloatArray"]

FloatArray: TypeAlias = npt.NDArray[np.floating[Any]]


class ArrayLike(Protocol):
    """Anything with ``__array__``: pandas Series, xarray DataArrays, …"""

    def __array__(self) -> npt.NDArray[Any]: ...


# The input types whose output type is known: the same type, of floats.
Coordinates = TypeVar("Coordinates", float, list[float], tuple[float, ...], "array[float]", FloatArray)

Direction: TypeAlias = TransformDirection | str
