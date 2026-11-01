"""pytest plugin: GeoPandas' test suite on xeibe_proj, through the shim.

Loaded with ``-p xeibe_geopandas_shim``, before GeoPandas or anything else
imports pyproj, it calls ``xeibe_proj.compat.install_as_pyproj()``. Unlike
pyproj's suite (``tests/pyproj_suite``), there are no placeholders: this is
the shim as users get it.
"""

import xeibe_proj.compat

xeibe_proj.compat.install_as_pyproj()
