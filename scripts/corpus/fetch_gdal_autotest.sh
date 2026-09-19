#!/usr/bin/env bash
# Sparse, blobless clone of GDAL's GML-related autotest data and test scripts into
# example_data/gdal-autotest-src (git-ignored), pinned to the commit recorded in
# example_data/SOURCES.md. Used by scripts/corpus/extract_gdal_geom_cases.py and
# as a source of test samples.
#
#   scripts/corpus/fetch_gdal_autotest.sh [COMMIT]
#
# Cone mode also checks out the files directly in autotest/ogr/ (ogr_gml_geom.py,
# ogr_gml.py, ogr_gml_fgd_read.py, ogr_wfs.py, ...).
set -euo pipefail

COMMIT="${1:-ec7b4b055f80b69dac45b303ed92376301ee2f34}"
PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
DEST="$PROJECT_DIR/example_data/gdal-autotest-src"
DIRS=(autotest/ogr/data/gml autotest/ogr/data/nas autotest/ogr/data/wfs autotest/ogr/data/gmlas)

if [[ ! -d "$DEST/.git" ]]; then
    mkdir -p "$(dirname "$DEST")"
    git clone -q --filter=blob:none --no-checkout https://github.com/OSGeo/gdal.git "$DEST"
fi
cd "$DEST"
git sparse-checkout set --cone "${DIRS[@]}"
git fetch -q --filter=blob:none origin "$COMMIT"
git -c advice.detachedHead=false checkout -q "$COMMIT"
git log -1 --format='-> example_data/gdal-autotest-src @ %h (%cs)'
