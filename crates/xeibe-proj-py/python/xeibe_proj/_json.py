"""Typed access to parsed PROJJSON, whose values are untyped."""

from __future__ import annotations

from typing import Any, cast

JsonDict = dict[str, Any]


def as_dict(value: object) -> JsonDict | None:
    """``value`` if it is a JSON object."""
    return cast(JsonDict, value) if isinstance(value, dict) else None


def get_dict(d: JsonDict, key: str) -> JsonDict:
    """The object at ``key``, or an empty one."""
    return as_dict(d.get(key)) or {}


def as_list(value: object) -> list[Any] | None:
    """``value`` if it is a JSON array."""
    # mypy narrows to list[Any] and calls the cast redundant; pyright
    # narrows to list[Unknown] and needs it.
    return cast(list[Any], value) if isinstance(value, list) else None  # type: ignore[redundant-cast]


def get_list(d: JsonDict, key: str) -> list[Any]:
    """The array at ``key``, or an empty one."""
    return as_list(d.get(key)) or []


def get_dicts(d: JsonDict, key: str) -> list[JsonDict]:
    """The objects in the array at ``key``."""
    return [o for o in map(as_dict, get_list(d, key)) if o is not None]


def ident(d: JsonDict) -> tuple[str, str] | None:
    """``(authority, code)`` from the object's ``id``."""
    i = as_dict(d.get("id"))
    if i is None:
        return None
    return str(i.get("authority", "")), str(i.get("code", ""))
