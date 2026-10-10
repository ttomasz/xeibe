"""Read GML files and WFS responses into Arrow/GeoArrow.

`scan()` finds layers and infers their schemas, `read()` streams one layer as
Arrow record batches. Schemas and streams are arro3 objects (`arro3.core`),
which PyArrow, GeoPandas, DuckDB, Polars and SedonaDB take as they are.
`xeibe.sedona` makes GML a SedonaDB data source.
"""

from xeibe._xeibe import (
    NoFeaturesError,
    Scan,
    SkippedSourceWarning,
    UnknownLayerError,
    XeibeError,
    read,
    scan,
)

__all__ = [
    "NoFeaturesError",
    "Scan",
    "SkippedSourceWarning",
    "UnknownLayerError",
    "XeibeError",
    "read",
    "scan",
]
