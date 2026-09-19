#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""List global elements/attributes of the local OGC GML and WFS schemas that are
not mentioned by name in docs/support-matrix.md.

Abstract elements and whole areas that the matrix covers with a group row (CRS
definitions, coverages, topology, styles, …) are skipped via SKIP_FILES /
SKIP_NAMES. Anything left over should be added to the matrix or to a skip list.
"""
import pathlib
import re
import xml.etree.ElementTree as ET

ROOT = pathlib.Path(__file__).resolve().parent.parent
SCHEMAS = ROOT / "ogc_schemas"
MATRIX = (ROOT / "docs" / "support-matrix.md").read_text(encoding="utf-8")
XS = "{http://www.w3.org/2001/XMLSchema}"

SETS = {
    "GML 2.1.2": "gml/2.1.2", "GML 3.1.1": "gml/3.1.1/base", "GML 3.2.1": "gml/3.2.1",
    "WFS 1.0.0": "wfs/1.0.0", "WFS 1.1.0": "wfs/1.1.0", "WFS 2.0": "wfs/2.0",
}
# Files covered by a single "not planned" group row in the matrix.
SKIP_FILES = {
    "coordinateOperations.xsd", "coordinateReferenceSystems.xsd", "coordinateSystems.xsd",
    "datums.xsd", "referenceSystems.xsd", "dictionary.xsd", "units.xsd", "coverage.xsd",
    "grids.xsd", "topology.xsd", "temporalTopology.xsd", "temporalReferenceSystems.xsd",
    "defaultStyle.xsd", "dataQuality.xsd", "dynamicFeature.xsd", "observation.xsd",
    "valueObjects.xsd", "direction.xsd", "WFS-transaction.xsd", "WFS-capabilities.xsd",
}
# Names that exist only in XSD annotations, or that the matrix covers via a group row.
SKIP_NAMES = {
    "targetElement", "reversePropertyName", "associationName", "defaultCodeSpace",
    "gmlProfileSchema", "remarks",
    # deprecated CRS/operation dictionary internals (dictionary group row)
    "anchorPoint", "includesValue", "methodFormula", "valuesOfGroup",
    # WFS request/response plumbing covered by operation rows
    "Query", "StoredQuery", "PropertyName", "XlinkPropertyName", "WFS_Capabilities",
    "Title", "Abstract", "ValueList", "ValueCollection", "additionalValues",
    "ServesGMLObjectTypeList", "SupportsGMLObjectTypeList", "LockFeatureResponse",
    "TransactionResponse", "LockId", "Insert", "Delete", "Replace", "Native", "Property",
    "ListStoredQueriesResponse", "DescribeStoredQueriesResponse", "CreateStoredQueryResponse",
    "DropStoredQueryResponse", "handle", "service", "version", "code", "locator", "lockId",
    "expiry", "lockAction", "releaseAction", "safeToIgnore", "vendorId", "idgen",
    "inputFormat", "traverseXlinkExpiry", "resolvePath", "valueReference", "about",
    "action", "isPrivate", "language", "returnFeatureTypes", "state",
    # CRS internals that leaked into deprecated/3.1 files
}
CRS_LIKE = re.compile(r"(Ref|ID|Name|CRS|CS|Datum|Operation|Parameter|Ellipsoid|Meridian|"
                      r"Conversion|Transformation|^uses|Accuracy|Axis|Flattening|Sphere$|"
                      r"^dms|^degrees$|^minutes$|^seconds$|^decimalMinutes$|Domain$|Patches$)")


def mentioned(name):
    return re.search(r"(?<![A-Za-z_])" + re.escape(name) + r"(?![A-Za-z_])", MATRIX) is not None


total = 0
for label, rel in SETS.items():
    missing = []
    for f in sorted((SCHEMAS / rel).glob("*.xsd")):
        if f.name in SKIP_FILES:
            continue
        tree = ET.parse(f).getroot()
        for e in tree.findall(XS + "element"):
            n = e.get("name")
            if (e.get("abstract") == "true" or n.startswith("_") or n in SKIP_NAMES
                    or CRS_LIKE.search(n) or mentioned(n)):
                continue
            missing.append(f"{f.name}: <{n}>")
        for a in tree.iter(XS + "attribute"):
            n = a.get("name")
            if n and n not in SKIP_NAMES and not mentioned(n):
                missing.append(f"{f.name}: @{n}")
    missing = sorted(set(missing))
    total += len(missing)
    print(f"== {label}: {len(missing)} not mentioned")
    for m in missing:
        print("   ", m)
print(f"\nTotal: {total}")
