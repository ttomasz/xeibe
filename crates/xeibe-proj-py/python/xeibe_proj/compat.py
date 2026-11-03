"""Running code written for pyproj on xeibe_proj, without changing it.

``install_as_pyproj()`` makes ``import pyproj`` give xeibe_proj, for
libraries such as GeoPandas that import pyproj themselves. It is
experimental: importing ``xeibe_proj`` directly stays the recommended way.
See ``docs/pyproj-phase3.md`` in the xeibe repository.
"""

from __future__ import annotations

import importlib
import importlib.abc
import importlib.machinery
import sys
import types
from collections.abc import Sequence

__all__ = ["install_as_pyproj"]

#: The pyproj release whose API xeibe_proj follows, given as
#: ``pyproj.__version__`` under the shim.
PYPROJ_VERSION = "3.8.0"

_PLAN = "see docs/pyproj-compat.md in the xeibe repository"

# pyproj 3.8.0's modules below the top level. Each maps to the xeibe_proj
# module of the same name when there is one; the reason is for when there
# isn't.
_MODULES: dict[str, str] = {
    "aoi": "",
    "crs": "",
    "crs.crs": "",
    "crs.coordinate_operation": "",
    "crs.coordinate_system": "",
    "crs.datum": "",
    "crs.enums": "",
    "crs._cf1x8": "",
    "database": "",
    "datadir": "",
    "enums": "",
    "exceptions": "",
    "geod": "",
    "list": "",
    "network": "",
    "proj": "",
    "transformer": "",
    "utils": "not implemented yet (planned for phase 3)",
    "sync": "not supported: it downloads PROJ-data, and grids are fetched when needed",
    "__main__": "not supported: there is no pyproj command line",
}
_PRIVATE = "not supported: pyproj's private modules bind PROJ, which isn't there"


class _Finder(importlib.abc.MetaPathFinder, importlib.abc.Loader):
    """Finds ``pyproj`` and its modules, ahead of an installed pyproj."""

    def find_spec(
        self,
        fullname: str,
        path: Sequence[str] | None = None,
        target: types.ModuleType | None = None,
    ) -> importlib.machinery.ModuleSpec | None:
        if fullname == "pyproj":
            return importlib.machinery.ModuleSpec(fullname, self, is_package=True)
        if not fullname.startswith("pyproj."):
            return None
        name = fullname.removeprefix("pyproj.")
        reason = _MODULES.get(name, _PRIVATE if name.split(".")[-1].startswith("_") else None)
        if reason is None:
            reason = "pyproj 3.8.0 has no such module"
        if reason:
            raise ModuleNotFoundError(f"{fullname}: {reason} ({_PLAN})", name=fullname)
        module = importlib.import_module(f"xeibe_proj.{name}")
        return importlib.machinery.ModuleSpec(
            fullname, self, is_package=hasattr(module, "__path__")
        )

    def create_module(self, spec: importlib.machinery.ModuleSpec) -> types.ModuleType:
        if spec.name == "pyproj":
            return _top_level()
        # The xeibe_proj module itself, so that its classes are pyproj's.
        return importlib.import_module("xeibe_proj." + spec.name.removeprefix("pyproj."))

    def exec_module(self, module: types.ModuleType) -> None:
        pass


def _top_level() -> types.ModuleType:
    """``pyproj``: xeibe_proj's attributes, with pyproj's version."""
    import xeibe_proj

    module = types.ModuleType("pyproj", xeibe_proj.__doc__)

    def __getattr__(name: str) -> object:
        try:
            return getattr(xeibe_proj, name)
        except AttributeError:
            # pyproj imports its modules, so `pyproj.datadir` works unimported.
            if _MODULES.get(name) == "":
                return importlib.import_module(f"pyproj.{name}")
            raise

    def __dir__() -> list[str]:
        return sorted(set(dir(xeibe_proj)) | set(module.__dict__))

    module.__dict__.update(
        __getattr__=__getattr__,
        __dir__=__dir__,
        __all__=list(xeibe_proj.__all__),
        __version__=PYPROJ_VERSION,
        __xeibe_proj_version__=xeibe_proj.__version__,
    )
    return module


_FINDER = _Finder()


def install_as_pyproj() -> None:
    """Make ``import pyproj`` and ``import pyproj.X`` give xeibe_proj.

    Each ``pyproj.X`` is the module ``xeibe_proj.X`` itself, so
    ``pyproj.CRS is xeibe_proj.CRS``. The top-level ``pyproj`` forwards to
    ``xeibe_proj`` and has ``__version__ = "3.8.0"``, the pyproj release
    whose API is followed, and ``__xeibe_proj_version__``. A pyproj module
    xeibe_proj doesn't have raises ``ModuleNotFoundError`` with the reason.
    This takes precedence over an installed pyproj, and calling it again
    does nothing.

    Call it before anything imports pyproj. ``importlib.metadata`` still
    reports the installed pyproj distribution, if any.

    Raises:
        RuntimeError: pyproj was imported already, so objects of both
            libraries would mix.
    """
    if _FINDER in sys.meta_path:
        return
    loaded = sorted(n for n in sys.modules if n == "pyproj" or n.startswith("pyproj."))
    if loaded:
        raise RuntimeError(
            f"pyproj is already imported ({', '.join(loaded[:3])}): "
            "call install_as_pyproj() before anything imports pyproj"
        )
    sys.meta_path.insert(0, _FINDER)
