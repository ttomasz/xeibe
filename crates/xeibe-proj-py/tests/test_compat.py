"""``xeibe_proj.compat.install_as_pyproj()``. Each test runs in a new
interpreter: the shim changes ``sys.modules``, and the other tests import
the real pyproj."""

from __future__ import annotations

import os
import subprocess
import sys
import textwrap


def run(code: str) -> None:
    # The whole environment: Python on Windows doesn't start without SYSTEMROOT.
    env = {**os.environ, "PROJ_NETWORK": "OFF"}
    subprocess.run([sys.executable, "-c", textwrap.dedent(code)], check=True, env=env)


def test_pyproj_modules_are_xeibe_projs() -> None:
    run("""
        import pickle
        import xeibe_proj, xeibe_proj.compat
        xeibe_proj.compat.install_as_pyproj()
        xeibe_proj.compat.install_as_pyproj()  # again: nothing happens

        import pyproj
        import pyproj.crs.crs
        from pyproj import CRS, Transformer
        from pyproj.aoi import AreaOfInterest
        from pyproj.database import query_utm_crs_info
        from pyproj.exceptions import CRSError

        assert pyproj.__version__ == "3.8.0"
        assert pyproj.__xeibe_proj_version__ == xeibe_proj.__version__
        assert CRS is xeibe_proj.CRS and Transformer is xeibe_proj.Transformer
        assert pyproj.crs is xeibe_proj.crs and pyproj.crs.crs is xeibe_proj.crs.crs
        assert CRSError is xeibe_proj.exceptions.CRSError
        assert isinstance(pickle.loads(pickle.dumps(CRS(2180))), pyproj.CRS)
        # Not imported first, as pyproj's own __init__ imports it.
        assert pyproj.datadir.get_data_dir() == ""
        assert "CRS" in dir(pyproj)
        x, y = Transformer.from_crs(4326, 3857, always_xy=True).transform(1.0, 2.0)
        assert abs(x - 111319.49079327357) < 1e-6
        try:
            pyproj.proj_version_str
        except AttributeError as e:
            assert "no PROJ" in str(e)
        else:
            raise AssertionError("proj_version_str")
    """)


def test_missing_modules_say_why() -> None:
    run("""
        import xeibe_proj.compat
        xeibe_proj.compat.install_as_pyproj()
        for name, reason in [
            ("pyproj.sync", "not supported"),
            ("pyproj._crs", "private"),
            ("pyproj.crs._cf1x8", "not implemented yet"),
            ("pyproj.nothing", "no such module"),
        ]:
            try:
                __import__(name)
            except ModuleNotFoundError as e:
                assert reason in str(e) and e.name == name, e
            else:
                raise AssertionError(name)
        # pyproj's private names of xeibe_proj's private modules aren't mapped.
        try:
            import pyproj._json
        except ModuleNotFoundError:
            pass
        else:
            raise AssertionError("pyproj._json")
    """)


def test_raises_after_pyproj() -> None:
    run("""
        import sys, types
        sys.modules["pyproj"] = types.ModuleType("pyproj")
        import xeibe_proj.compat
        try:
            xeibe_proj.compat.install_as_pyproj()
        except RuntimeError as e:
            assert "already imported" in str(e)
        else:
            raise AssertionError("no error")
    """)
