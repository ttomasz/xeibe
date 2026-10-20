"""The errors for what isn't implemented (yet, or ever).

``docs/pyproj-compat.md`` lists each such part of pyproj's API with its phase
or as "Out"; the messages say which.
"""

from __future__ import annotations

_PLAN = "see docs/pyproj-compat.md in the xeibe repository"


def not_yet(what: str, phase: int) -> NotImplementedError:
    """For a planned part that isn't implemented yet."""
    return NotImplementedError(f"{what} is not implemented yet (planned for phase {phase}; {_PLAN})")


def out(what: str, reason: str) -> NotImplementedError:
    """For a part of pyproj's API that is not planned."""
    return NotImplementedError(f"{what} is not supported: {reason} ({_PLAN})")
