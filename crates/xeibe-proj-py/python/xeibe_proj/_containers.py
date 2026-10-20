"""pyproj's container rules: coordinates come back in the type they came in.

A scalar gives a ``float``, a list a ``list``, a tuple a ``tuple``, an
``array.array`` an ``array.array``, and a numpy array a numpy array (of
float64, in C order). pandas Series, xarray DataArrays and other objects
with ``__array__`` are converted with it and give numpy arrays. numpy is
never imported: its arrays are converted with their own methods.
"""

from __future__ import annotations

from array import array
from enum import Enum, auto
from typing import Any


class Kind(Enum):
    FLOAT = auto()
    LIST = auto()
    TUPLE = auto()
    ARRAY = auto()


def to_buffer(value: Any) -> tuple[Any, Kind]:
    """A new, writable float64 buffer holding ``value``'s coordinates."""
    # Masked arrays keep their type; other array-likes become numpy arrays.
    if not hasattr(value, "hardmask") and callable(getattr(value, "__array__", None)):
        value = value.__array__()
    if hasattr(value, "shape") and hasattr(value, "astype"):
        if value.shape == ():
            return array("d", (float(value),)), Kind.FLOAT
        return value.astype("d", order="C", copy=True), Kind.ARRAY
    if isinstance(value, (array, memoryview)):
        return array("d", value), Kind.ARRAY  # pyright: ignore[reportUnknownArgumentType]
    if isinstance(value, list):
        return array("d", value), Kind.LIST  # pyright: ignore[reportUnknownArgumentType]
    if isinstance(value, tuple):
        return array("d", value), Kind.TUPLE  # pyright: ignore[reportUnknownArgumentType]
    try:
        return array("d", (float(value),)), Kind.FLOAT
    except (TypeError, ValueError):
        raise TypeError(f"input must be a number or an array of numbers, not {type(value).__name__}") from None


def from_buffer(kind: Kind, buffer: Any) -> Any:
    """The converted coordinates in the input's type."""
    if kind is Kind.FLOAT:
        return buffer[0]
    if kind is Kind.LIST:
        return buffer.tolist()
    if kind is Kind.TUPLE:
        return tuple(buffer)
    return buffer
