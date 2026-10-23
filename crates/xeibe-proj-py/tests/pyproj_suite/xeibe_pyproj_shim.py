"""pytest plugin: pyproj's own test suite, run against xeibe_proj.

Loaded with ``-p xeibe_pyproj_shim`` before pyproj's ``conftest.py``, it
puts ``xeibe_proj`` in ``sys.modules`` under pyproj's module names. Each
pyproj module becomes a proxy for its xeibe_proj counterpart, and a name
xeibe_proj doesn't have (a module, class or function) becomes a placeholder
that raises ``NotImplementedError`` when used. So one missing name fails the
tests that use it, not every test in the file that imports it.

A test file that fails to import, because module-level code (usually a
``parametrize`` list) calls something xeibe_proj doesn't support, is run
again statement by statement, leaving out the statements that fail. The
tests those statements define are then missing, and tests that use a name
they would have defined fail: neither can pass by accident.

A test that touches a placeholder while it runs fails, even if nothing
raised: importing a name xeibe_proj doesn't have is not a pass.

Two attributes are set here only, for pyproj's test helpers:
``__proj_version__`` is the PROJ version xeibe-reproject follows (its
conftest gates tests on it), and ``__version__`` is the pyproj release whose
tests these are. The package itself has neither (docs/pyproj-compat.md).
"""

from __future__ import annotations

import ast
import importlib
import sys
import types
import warnings
from pathlib import Path
from typing import Any

import pytest
from _pytest.pathlib import resolve_pkg_root_and_module_name

import xeibe_proj

PROJ_VERSION = "9.8.1"
PYPROJ_VERSION = "3.8.0"

# pyproj's modules (pyproj 3.8.0's source tree), and xeibe_proj's module for
# each, where there is one.
MODULES: dict[str, str | None] = {
    "pyproj": "xeibe_proj",
    "pyproj.aoi": "xeibe_proj.aoi",
    "pyproj.crs": "xeibe_proj.crs",
    "pyproj.crs.crs": "xeibe_proj.crs.crs",
    "pyproj.database": "xeibe_proj.database",
    "pyproj.datadir": "xeibe_proj.datadir",
    "pyproj.enums": "xeibe_proj.enums",
    "pyproj.exceptions": "xeibe_proj.exceptions",
    "pyproj.geod": "xeibe_proj.geod",
    "pyproj.list": "xeibe_proj.list",
    "pyproj.network": "xeibe_proj.network",
    "pyproj.transformer": "xeibe_proj.transformer",
    "pyproj._compat": None,
    "pyproj._context": None,
    "pyproj._crs": None,
    "pyproj.crs._cf1x8": None,
    "pyproj.crs.coordinate_operation": None,
    "pyproj.crs.coordinate_system": None,
    "pyproj.crs.datum": None,
    "pyproj.crs.enums": None,
    "pyproj._geod": None,
    "pyproj.__main__": None,
    "pyproj._network": None,
    "pyproj.proj": None,
    "pyproj._show_versions": None,
    "pyproj._sync": None,
    "pyproj.sync": None,
    "pyproj._transformer": None,
    "pyproj.utils": None,
    "pyproj._version": None,
}


# The placeholders made or used since the current test started.
TOUCHED: list[str] = []


class Missing:
    """A name xeibe_proj doesn't have. Using it raises; reading an attribute
    gives another placeholder, so module-level references still import."""

    def __init__(self, name: str) -> None:
        object.__setattr__(self, "_name", name)
        TOUCHED.append(name)

    def _fail(self) -> NotImplementedError:
        return NotImplementedError(f"{self._name} is not in xeibe_proj")

    def __call__(self, *args: Any, **kwargs: Any) -> Any:
        raise self._fail()

    def __bool__(self) -> bool:
        raise self._fail()

    def __eq__(self, other: object) -> bool:
        raise self._fail()

    def __ne__(self, other: object) -> bool:
        raise self._fail()

    __hash__ = object.__hash__

    def __getattr__(self, attr: str) -> Any:
        if attr.startswith("__"):
            raise AttributeError(attr)
        return Missing(f"{self._name}.{attr}")

    def __iter__(self) -> Any:
        raise self._fail()

    def __repr__(self) -> str:
        return f"<missing {self._name}>"


class Proxy(types.ModuleType):
    """A pyproj module: xeibe_proj's attributes, else placeholders."""

    def __init__(self, name: str, target: types.ModuleType | None) -> None:
        super().__init__(name)
        object.__setattr__(self, "_target", target)
        # A package, so that `import pyproj.x` looks in sys.modules.
        object.__setattr__(self, "__path__", [])

    def __getattr__(self, attr: str) -> Any:
        target = object.__getattribute__(self, "_target")
        if target is not None:
            try:
                return getattr(target, attr)
            except AttributeError:
                pass
        if attr.startswith("__"):
            raise AttributeError(attr)
        return Missing(f"{self.__name__}.{attr}")

    def __setattr__(self, attr: str, value: Any) -> None:
        # Monkeypatching a pyproj module patches xeibe_proj's.
        target = object.__getattribute__(self, "_target")
        if target is not None and hasattr(target, attr):
            setattr(target, attr, value)
        else:
            object.__setattr__(self, attr, value)

    def __delattr__(self, attr: str) -> None:
        target = object.__getattribute__(self, "_target")
        if target is not None and hasattr(target, attr):
            delattr(target, attr)
        else:
            object.__delattr__(self, attr)


def install() -> None:
    proxies: dict[str, Proxy] = {}
    for name, target in MODULES.items():
        module = None if target is None else importlib.import_module(target)
        proxies[name] = Proxy(name, module)
    for name, proxy in proxies.items():
        sys.modules[name] = proxy
        parent, _, child = name.rpartition(".")
        if parent:
            object.__setattr__(proxies[parent], child, proxy)
    root = proxies["pyproj"]
    object.__setattr__(root, "__proj_version__", PROJ_VERSION)
    object.__setattr__(root, "__version__", PYPROJ_VERSION)
    object.__setattr__(root, "__xeibe_proj__", xeibe_proj.__version__)


def _lenient_import(path: Path) -> types.ModuleType:
    """Import a test file one top-level statement at a time."""
    try:
        root, name = resolve_pkg_root_and_module_name(path)
    except Exception:  # noqa: BLE001 - not in a package (test/crs has no __init__.py)
        root, name = path.parent, path.stem
    if str(root) not in sys.path:
        sys.path.insert(0, str(root))
    module = types.ModuleType(name)
    module.__file__ = str(path)
    sys.modules[name] = module
    source = path.read_text()
    for statement in ast.parse(source, str(path)).body:
        code = compile(ast.Module([statement], type_ignores=[]), str(path), "exec")
        try:
            exec(code, module.__dict__)
        # pytest.fail and pytest.warns raise pytest's Failed, not an Exception.
        except (Exception, pytest.fail.Exception) as e:  # noqa: BLE001 - leave the statement out
            warnings.warn(
                f"{path.name}:{statement.lineno}: left out ({type(e).__name__}: {e})",
                stacklevel=1,
            )
    return module


class LenientModule(pytest.Module):
    def _getobj(self) -> Any:
        try:
            return super()._getobj()
        except (Exception, pytest.fail.Exception):  # noqa: BLE001 - try statement by statement
            return _lenient_import(self.path)


@pytest.hookimpl(wrapper=True)
def pytest_runtest_call(item: pytest.Item) -> Any:
    TOUCHED.clear()
    result = yield
    if TOUCHED:
        names = ", ".join(sorted(set(TOUCHED)))
        raise NotImplementedError(f"the test used names xeibe_proj doesn't have: {names}")
    return result


@pytest.hookimpl(tryfirst=True)
def pytest_pycollect_makemodule(module_path: Path, parent: pytest.Collector) -> pytest.Module | None:
    if module_path.name.startswith("test_"):
        return LenientModule.from_parent(parent, path=module_path)
    return None


install()
