#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Generate `crates/xeibe-reproject/src/wkt/names.rs`, WKT1's method and
parameter names, from PROJ's mapping tables (`docs/crs-wkt.md`, "WKT1").

WKT1 names projection methods and parameters its own way
(`PROJECTION["Transverse_Mercator"]`, `PARAMETER["central_meridian"]`). Only
PROJ and GDAL define those names, in PROJ's C++ source (MIT):

* `src/iso19111/operation/parammappings.cpp`: `gProjectionMethodMappings`,
  each method's EPSG code, WKT1_GDAL name and parameters (EPSG code, WKT1
  name, unit type), in the order PROJ builds a conversion from them;
* `src/iso19111/operation/esriparammappings.cpp`: `esriMappings`, each ESRI
  projection name with its EPSG method and ESRI parameter names;
* `src/proj_constants.h`: the `EPSG_NAME_*` / `EPSG_CODE_*` macros both use.

This script reads the three files straight out of the PROJ source release the
test oracle uses (`scripts/build_proj_oracle.py`; keep the two in step; the
tarball is cached in `target/proj-oracle/build/`), parses the C++ tables and
writes them as Rust:

* `METHODS`: the methods with an EPSG code and a WKT1_GDAL name. Methods only
  PROJ defines (code 0: Eckert, Mollweide, Robinson, …) and EPSG methods PROJ
  can't write in WKT1 (no WKT1 name: LCC 1SP variant B, Michigan, Krovak
  modified, Equal Earth, …) are dropped and counted.
* `ESRI_METHODS`: the ESRI projections whose method has an EPSG code and whose
  parameters each map to an EPSG parameter or to a value PROJ fixes. The
  others are dropped and listed.

Then it checks the tables against PROJ's own output for every EPSG CRS
(`target/crs-wkt/wkt.jsonl` from `scripts/gen_wkt_vectors.py --all`; skipped
with a note when missing): each projected CRS's WKT1_GDAL (and WKT1_ESRI)
string is read through the tables and compared with the same CRS's WKT2 2019
method id and parameter ids and values. It prints how many agree and every
disagreement by kind, with examples.

    scripts/gen_wkt1_names.py
    scripts/gen_wkt1_names.py --wkt target/crs-wkt/wkt.jsonl --examples 5
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import re
import sys
import tarfile
import urllib.request
from collections import Counter, defaultdict
from dataclasses import dataclass, field
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
OUT = REPO / "crates" / "xeibe-reproject" / "src" / "wkt" / "names.rs"
CACHE = REPO / "target" / "proj-oracle" / "build"
WKT_JSONL = REPO / "target" / "crs-wkt" / "wkt.jsonl"

PROJ_VERSION = "9.8.1"
PROJ_URL = f"https://download.osgeo.org/proj/proj-{PROJ_VERSION}.tar.gz"
PROJ_SHA256 = "af5b731c145c1d13c4e3b4eeb7d167e94e845e440f71e3496b4ed8dae0291960"

MEMBERS = {
    "constants": "src/proj_constants.h",
    "mappings": "src/iso19111/operation/parammappings.cpp",
    "esri": "src/iso19111/operation/esriparammappings.cpp",
}

UNIT_TYPES = {"LINEAR": "Linear", "ANGULAR": "Angular", "SCALE": "Scale", "NONE": "None"}


# --- PROJ source -------------------------------------------------------------


def tarball() -> Path:
    CACHE.mkdir(parents=True, exist_ok=True)
    path = CACHE / f"proj-{PROJ_VERSION}.tar.gz"
    if not path.exists():
        print(f"downloading {PROJ_URL}")
        with urllib.request.urlopen(PROJ_URL) as response:
            path.write_bytes(response.read())
    digest = hashlib.sha256(path.read_bytes()).hexdigest()
    if digest != PROJ_SHA256:
        path.unlink()
        sys.exit(f"{path.name}: sha256 {digest}, expected {PROJ_SHA256}")
    return path


def read_sources(path: Path) -> dict[str, str]:
    out = {}
    with tarfile.open(path) as tar:
        for key, member in MEMBERS.items():
            f = tar.extractfile(f"proj-{PROJ_VERSION}/{member}")
            if f is None:
                sys.exit(f"{path.name}: no {member}")
            out[key] = f.read().decode("utf-8")
    return out


# --- A small C reader: enough for PROJ's constant tables --------------------


def strip_comments(src: str) -> str:
    """Drop // and /* */ comments, leaving string literals alone."""
    out, i, n = [], 0, len(src)
    while i < n:
        c = src[i]
        if c == '"':
            j = i + 1
            while src[j] != '"':
                j += 2 if src[j] == "\\" else 1
            out.append(src[i : j + 1])
            i = j + 1
        elif src.startswith("//", i):
            i = src.find("\n", i)
            i = n if i < 0 else i
        elif src.startswith("/*", i):
            i = src.index("*/", i) + 2
            out.append(" ")
        else:
            out.append(c)
            i += 1
    return "".join(out)


C_TOKEN = re.compile(
    r'\s*(?:(?P<str>"(?:[^"\\]|\\.)*")|(?P<num>-?\d+(?:\.\d*)?(?:[eE][-+]?\d+)?)'
    r"|(?P<id>[A-Za-z_]\w*(?:::[A-Za-z_]\w*)*)|(?P<p>[{}\[\](),;=&*]))"
)


@dataclass(frozen=True)
class Ref:
    """`&name`: the address of another table entry."""

    name: str


@dataclass(frozen=True)
class Ident:
    name: str


def c_tokens(src: str, pos: int = 0):
    while True:
        m = C_TOKEN.match(src, pos)
        if not m:
            if src[pos:].strip():
                raise ValueError(f"can't tokenise C at {src[pos : pos + 40]!r}")
            return
        pos = m.end()
        kind = m.lastgroup
        yield kind, m.group(kind), pos


def c_initializer(src: str, pos: int) -> list:
    """Parse the brace initializer starting at src[pos] == '{'."""
    toks = c_tokens(src, pos)
    _, text, _ = next(toks)
    assert text == "{", (text, src[pos : pos + 40])

    def parse_list() -> list:
        items: list = []
        current: list = []  # one item's tokens; adjacent strings concatenate
        for kind, text, _ in toks:
            if text == "{":
                current.append(parse_list())
            elif text in (",", "}"):
                if current:
                    items.append(scalar(current))
                    current = []
                if text == "}":
                    return items
            else:
                current.append((kind, text))
        raise ValueError("unterminated initializer")

    return parse_list()


def scalar(parts: list):
    if len(parts) == 1 and isinstance(parts[0], list):
        return parts[0]
    if all(isinstance(p, tuple) and p[0] == "str" for p in parts):
        return "".join(json.loads(p[1]) for p in parts)
    if len(parts) == 2 and parts[0] == ("p", "&"):
        return Ref(parts[1][1])
    if len(parts) == 1:
        kind, text = parts[0]
        if kind == "num":
            return float(text) if any(c in text for c in ".eE") else int(text)
        if text == "nullptr":
            return None
        if text in ("true", "false"):
            return text == "true"
        return Ident(text)
    raise ValueError(f"can't read initializer item {parts}")


def parse_defines(header: str) -> dict[str, object]:
    text = strip_comments(header).replace("\\\n", " ")
    out: dict[str, object] = {}
    for m in re.finditer(r"^[ \t]*#define[ \t]+(\w+)[ \t]+(.+)$", text, re.MULTILINE):
        value = m.group(2).strip()
        if re.fullmatch(r'("(?:[^"\\]|\\.)*"\s*)+', value):
            out[m.group(1)] = "".join(json.loads(s) for s in re.findall(r'"(?:[^"\\]|\\.)*"', value))
        elif re.fullmatch(r"-?\d+", value):
            out[m.group(1)] = int(value)
    return out


def declarations(src: str, type_name: str) -> dict[str, list]:
    """Every `type_name [*const] NAME[] = {...};` / `type_name NAME = {...};`."""
    out = {}
    pattern = re.compile(rf"\b{type_name}\s*(?:\*\s*const\s+)?(\w+)\s*(?:\[\])?\s*=?\s*(?=\{{)")
    for m in pattern.finditer(src):
        out[m.group(1)] = c_initializer(src, m.end())
    return out


def resolve(value, consts: dict[str, object]):
    if isinstance(value, Ident):
        if value.name in consts:
            return consts[value.name]
        if value.name.startswith("common::UnitOfMeasure::Type::"):
            return value.name.rsplit("::", 1)[1]
        raise ValueError(f"unknown identifier {value.name}")
    return value


# --- PROJ's tables -----------------------------------------------------------


@dataclass
class PParam:
    wkt2: str
    epsg: int
    wkt1: str | None
    unit: str
    proj: str | None


@dataclass
class PMethod:
    wkt2: str
    epsg: int
    wkt1: str | None
    params: list[PParam]


@dataclass
class EParam:
    esri: str
    wkt2: str | None
    epsg: int
    fixed: float
    is_fixed: bool


@dataclass
class EMethod:
    esri: str
    wkt2: str
    epsg: int
    params: list[EParam]


def parse_proj(sources: dict[str, str]) -> tuple[list[PMethod], list[EMethod], dict[str, object]]:
    consts = parse_defines(sources["constants"])
    mappings = strip_comments(sources["mappings"])
    for m in re.finditer(r'const\s+char\s*\*\s*(\w+)\s*=\s*("(?:[^"\\]|\\.)*")\s*;', mappings):
        consts[m.group(1)] = json.loads(m.group(2))

    def r(v):
        return resolve(v, consts)

    params = {
        name: PParam(r(v[0]), r(v[1]), r(v[2]), r(v[3]), r(v[4]))
        for name, v in declarations(mappings, "ParamMapping").items()
        if not isinstance(v[0], Ref)
    }
    param_lists = {}
    for name, v in declarations(mappings, r"ParamMapping\s*\*\s*const").items():
        assert v[-1] is None, name
        param_lists[name] = [params[ref.name] for ref in v[:-1]]
    table = declarations(mappings, "MethodMapping")["gProjectionMethodMappings"]
    methods = []
    for entry in table:
        wkt2, epsg, wkt1, _main, _aux, plist = entry
        methods.append(PMethod(r(wkt2), r(epsg), r(wkt1), param_lists[plist.name]))

    esri_src = strip_comments(sources["esri"])
    esri_params = {
        name: [EParam(r(e[0]), r(e[1]), r(e[2]), float(r(e[3])), r(e[4])) for e in v if e[0] is not None]
        for name, v in declarations(esri_src, "ESRIParamMapping").items()
    }
    esri = [
        EMethod(r(e[0]), r(e[1]), r(e[2]), esri_params[e[3].name])
        for e in declarations(esri_src, "ESRIMethodMapping")["esriMappings"]
    ]
    return methods, esri, consts


# --- Tables for Rust ---------------------------------------------------------


@dataclass
class Tables:
    methods: list[PMethod]  # kept, with params filtered
    esri: list[EMethod]  # kept
    dropped_no_code: list[PMethod] = field(default_factory=list)
    dropped_no_wkt1: list[PMethod] = field(default_factory=list)
    dropped_params: list[tuple[PMethod, PParam]] = field(default_factory=list)
    esri_dropped: list[tuple[EMethod, str]] = field(default_factory=list)


def build_tables(methods: list[PMethod], esri: list[EMethod]) -> Tables:
    t = Tables([], [])
    for m in methods:
        if not m.epsg:
            t.dropped_no_code.append(m)
            continue
        if not m.wkt1:
            t.dropped_no_wkt1.append(m)
            continue
        kept = []
        for p in m.params:
            # A parameter without a WKT1 name is never written in WKT1 (only
            # Mercator_1SP's latitude of natural origin, always 0) and can't
            # be read from it, so it has no place in a WKT1 table.
            if not p.wkt1:
                t.dropped_params.append((m, p))
                continue
            kept.append(p)
        names = Counter(p.wkt1.lower() for p in kept)
        assert max(names.values()) == 1, (m.wkt1, names)
        t.methods.append(PMethod(m.wkt2, m.epsg, m.wkt1, kept))
    for e in esri:
        if not e.epsg:
            t.esri_dropped.append((e, "method has no EPSG code"))
            continue
        bad = [p.esri for p in e.params if p.wkt2 is not None and not p.epsg]
        if bad:
            t.esri_dropped.append((e, f"parameters without an EPSG code: {', '.join(bad)}"))
            continue
        t.esri.append(e)
    return t


def rust_str(s: str) -> str:
    return json.dumps(s, ensure_ascii=False)


def rust_f64(v: float) -> str:
    text = repr(float(v))
    return text if any(c in text for c in ".eEn") else text + ".0"


def render(t: Tables) -> str:
    lines = [
        "// @generated by scripts/gen_wkt1_names.py -- do not edit.",
        "//",
        f"// Derived from PROJ {PROJ_VERSION} (MIT, see PROJ-NOTICE.md next to this crate's",
        "// Cargo.toml): the tables in src/iso19111/operation/parammappings.cpp and",
        "// esriparammappings.cpp, with the codes of src/proj_constants.h.",
        "",
        "/// What a parameter measures.",
        "#[allow(dead_code)]",
        "#[derive(Debug, Clone, Copy, PartialEq, Eq)]",
        "pub(crate) enum UnitType {",
        "    Linear,",
        "    Angular,",
        "    Scale,",
        "    None,",
        "}",
        "",
        "/// One WKT1 parameter of a method.",
        "#[allow(dead_code)]",
        "#[derive(Debug)]",
        "pub(crate) struct Param {",
        "    pub(crate) epsg: u32,",
        "    /// PROJ's WKT2 name: EPSG's name (`Latitude of natural origin`).",
        "    pub(crate) name: &'static str,",
        "    pub(crate) wkt1: &'static str,",
        "    pub(crate) unit: UnitType,",
        "}",
        "",
        "/// One projection method with its WKT1 name and its parameters, in the",
        "/// order PROJ writes them.",
        "#[allow(dead_code)]",
        "#[derive(Debug)]",
        "pub(crate) struct Method {",
        "    pub(crate) epsg: u32,",
        "    /// PROJ's WKT2 name: EPSG's name (`Transverse Mercator`).",
        "    pub(crate) name: &'static str,",
        "    pub(crate) wkt1: &'static str,",
        "    pub(crate) params: &'static [Param],",
        "}",
        "",
        "/// PROJ's `gProjectionMethodMappings`: the methods with an EPSG code and a",
        "/// WKT1_GDAL name, in PROJ's order, each with its parameters in the order",
        "/// PROJ builds a conversion from them (EPSG's order). PROJ writes WKT1 in",
        "/// the conversion's own parameter order, which for EPSG CRSs is the same.",
        "///",
        "/// - One WKT1 name can name several methods (`Polar_Stereographic`,",
        "///   `Lambert_Azimuthal_Equal_Area`, `Krovak`, …). PROJ reads the name as",
        "///   the first entry that has it, then picks the variant from the",
        "///   parameters (`Polar_Stereographic`), the ellipsoid (the spherical",
        "///   and ellipsoidal variants of CEA, LAEA and Equidistant Cylindrical)",
        "///   or the axes (`Krovak` is 1041, north orientated, unless its `AXIS`",
        "///   are south, west). `Transverse_Mercator` is always 9807, never the",
        "///   3D 1111, and `Azimuthal_Equidistant` always 1125, never 9832.",
        "///   Writing takes the first entry with the method's code.",
        "/// - One method can have several WKT1 names (Albers 9822 also as Oracle's",
        "///   `Albers_Conical_Equal_Area`); the first is the one PROJ writes.",
        "/// - Mercator (variant A) has an EPSG latitude of natural origin that",
        "///   WKT1 never carries (PROJ requires it to be 0): it isn't listed, and",
        "///   the reader supplies 0.",
        "/// - Popular Visualisation Pseudo Mercator (1024) is listed under its",
        "///   own name, but PROJ writes it as `Mercator_1SP` with a `PROJ4`",
        "///   extension.",
        "#[allow(dead_code)]",
        "#[rustfmt::skip]",
        "pub(crate) static METHODS: &[Method] = &[",
    ]
    for m in t.methods:
        lines.append(f"    Method {{ epsg: {m.epsg}, name: {rust_str(m.wkt2)}, wkt1: {rust_str(m.wkt1)}, params: &[")
        for p in m.params:
            lines.append(
                f"        Param {{ epsg: {p.epsg}, name: {rust_str(p.wkt2)}, wkt1: {rust_str(p.wkt1)}, "
                f"unit: UnitType::{UNIT_TYPES[p.unit]} }},"
            )
        lines.append("    ] },")
    lines += [
        "];",
        "",
        "/// One parameter of an ESRI projection.",
        "///",
        "/// PROJ's `ESRIParamMapping` has three cases, kept here as:",
        "///",
        "/// - `epsg` set, `fixed` `None`: the value of that EPSG parameter.",
        "/// - `epsg` set, `fixed` `Some(v)`: the same, and `v` when the WKT leaves",
        "///   the parameter out (PROJ's `is_fixed_value`; only `Behrmann`).",
        "/// - `epsg` 0, `fixed` `Some(v)`: no EPSG counterpart; the parameter always",
        "///   has the value `v`. PROJ writes `v`, and when reading, a different",
        "///   value rules this entry out (it tells the variants of one ESRI name",
        "///   apart: `Krovak`'s `X_Scale`, `Lambert_Conformal_Conic`'s",
        "///   `Scale_Factor`).",
        "#[allow(dead_code)]",
        "#[derive(Debug)]",
        "pub(crate) struct EsriParam {",
        "    pub(crate) esri: &'static str,",
        "    pub(crate) epsg: u32,",
        "    pub(crate) fixed: Option<f64>,",
        "}",
        "",
        "/// One ESRI projection with its EPSG method and its parameters, in the",
        "/// order PROJ writes them.",
        "#[allow(dead_code)]",
        "#[derive(Debug)]",
        "pub(crate) struct EsriMethod {",
        "    pub(crate) esri: &'static str,",
        "    pub(crate) epsg: u32,",
        "    pub(crate) params: &'static [EsriParam],",
        "}",
        "",
        "/// PROJ's `esriMappings`, less the projections whose method has no EPSG",
        "/// code, in PROJ's order.",
        "///",
        "/// Several entries can share an ESRI name (`Lambert_Conformal_Conic` ×4,",
        "/// `Krovak` ×2, `Plate_Carree` ×2). PROJ reads such a name as the entry",
        "/// whose parameters match best: each parameter present counts one, and a",
        "/// fixed one (`epsg` 0) with another value rules the entry out; the first",
        "/// best entry wins; then, as for WKT1_GDAL, the ellipsoid picks the",
        "/// spherical or ellipsoidal variant of CEA, LAEA and Equidistant",
        "/// Cylindrical. Writing takes the first entry with the method's code,",
        "/// with PROJ's exceptions (see the writer).",
        "#[allow(dead_code)]",
        "#[rustfmt::skip]",
        "pub(crate) static ESRI_METHODS: &[EsriMethod] = &[",
    ]
    for e in t.esri:
        lines.append(f"    EsriMethod {{ esri: {rust_str(e.esri)}, epsg: {e.epsg}, params: &[")
        for p in e.params:
            if p.wkt2 is None:
                epsg, fixed = 0, f"Some({rust_f64(p.fixed)})"
            else:
                epsg, fixed = p.epsg, f"Some({rust_f64(p.fixed)})" if p.is_fixed else "None"
            lines.append(f"        EsriParam {{ esri: {rust_str(p.esri)}, epsg: {epsg}, fixed: {fixed} }},")
        lines.append("    ] },")
    lines.append("];")
    return "\n".join(lines) + "\n"


# --- WKT ---------------------------------------------------------------------


class Quoted(str):
    """A quoted WKT string (as opposed to a number or an enumeration word)."""


@dataclass
class Node:
    name: str
    args: list

    def children(self, name: str) -> list[Node]:
        return [a for a in self.args if isinstance(a, Node) and a.name == name]

    def child(self, name: str) -> Node | None:
        found = self.children(name)
        return found[0] if found else None


WKT_TOKEN = re.compile(
    r'\s*(?:(?P<q>"(?:[^"]|"")*")|(?P<open>[\[(])|(?P<close>[\])])|(?P<comma>,)|(?P<word>[^\s\[\](),"]+))'
)


def parse_wkt(text: str) -> Node:
    tokens = []
    pos = 0
    while pos < len(text):
        m = WKT_TOKEN.match(text, pos)
        if not m:
            if text[pos:].strip():
                raise ValueError(f"bad WKT at {text[pos : pos + 30]!r}")
            break
        pos = m.end()
        kind = m.lastgroup
        tokens.append((kind, m.group(kind)))
    # A keyword is a word directly followed by an opening bracket.
    stack: list[Node] = []
    root = None
    i = 0
    while i < len(tokens):
        kind, tok = tokens[i]
        if kind == "word" and i + 1 < len(tokens) and tokens[i + 1][0] == "open":
            n = Node(tok.upper(), [])
            if stack:
                stack[-1].args.append(n)
            stack.append(n)
            i += 2
            continue
        if kind == "close":
            done = stack.pop()
            if not stack:
                root = done
        elif kind == "q":
            stack[-1].args.append(Quoted(tok[1:-1].replace('""', '"')))
        elif kind == "word":
            stack[-1].args.append(tok)
        i += 1
    if stack or root is None:
        raise ValueError("unbalanced WKT")
    return root


def find_projected(root: Node, names: tuple[str, ...]) -> Node | None:
    if root.name in names:
        return root
    if root.name in ("COMPOUNDCRS", "COMPD_CS"):
        for a in root.args:
            if isinstance(a, Node) and a.name in names:
                return a
    return None


def canon(name: str) -> str:
    return re.sub(r"[^a-z0-9]", "", name.lower())


def epsg_id(node: Node) -> int | None:
    for key in ("ID", "AUTHORITY"):
        for i in node.children(key):
            if str(i.args[0]).upper() == "EPSG":
                return int(i.args[1])
    return None


def close(a: float, b: float, angular: bool = False) -> bool:
    if angular:
        d = math.remainder(a - b, 2 * math.pi)
        return abs(d) <= 1e-9 * max(abs(a), abs(b), 1e-12)
    return abs(a - b) <= 1e-9 * max(abs(a), abs(b), 1e-300)


@dataclass
class Wkt2Conv:
    method: int | None
    params: dict[int, tuple[float, str]]  # epsg code -> (SI value, unit type)


def wkt2_conversion(projcrs: Node) -> Wkt2Conv:
    conv = projcrs.child("CONVERSION")
    method = epsg_id(conv.child("METHOD"))
    params = {}
    for p in conv.children("PARAMETER"):
        code = epsg_id(p)
        unit = next((a for a in p.args if isinstance(a, Node) and a.name.endswith("UNIT")), None)
        factor = float(unit.args[1]) if unit is not None and len(unit.args) > 1 else 1.0
        kind = {"ANGLEUNIT": "ANGULAR", "LENGTHUNIT": "LINEAR", "SCALEUNIT": "SCALE"}.get(
            unit.name if unit is not None else "", "NONE"
        )
        if code is not None:
            params[code] = (float(p.args[1]) * factor, kind)
    return Wkt2Conv(method, params)


@dataclass
class Wkt1Proj:
    projection: str
    name: str
    params: list[tuple[str, float]]
    angle: float  # radians per GEOGCS unit
    linear: float  # metres per PROJCS unit
    sphere: bool  # the GEOGCS ellipsoid is a sphere
    axes: list[str]  # PROJCS AXIS directions, upper case
    proj4: str | None  # EXTENSION["PROJ4", ...]


def wkt1_projection(projcs: Node) -> Wkt1Proj:
    geogcs = projcs.child("GEOGCS")
    angle = float(geogcs.child("UNIT").args[1])
    linear = float(projcs.child("UNIT").args[1])
    params = [(str(p.args[0]), float(p.args[1])) for p in projcs.children("PARAMETER")]
    spheroid = geogcs.child("DATUM").child("SPHEROID")
    sphere = float(spheroid.args[2]) == 0.0
    axes = [str(a.args[1]).upper() for a in projcs.children("AXIS")]
    ext = next((e for e in projcs.children("EXTENSION") if str(e.args[0]) == "PROJ4"), None)
    proj4 = str(ext.args[1]) if ext is not None else None
    name = str(projcs.child("PROJECTION").args[0])
    return Wkt1Proj(name, str(projcs.args[0]), params, angle, linear, sphere, axes, proj4)


def to_si(value: float, unit: str, w1: Wkt1Proj) -> float:
    if unit == "ANGULAR":
        return value * w1.angle
    if unit == "LINEAR":
        return value * w1.linear
    return value


# --- Validation --------------------------------------------------------------


class Report:
    def __init__(self, title: str, examples: int):
        self.title = title
        self.examples = examples
        self.checked = 0
        self.agree = 0
        self.notes: Counter[str] = Counter()
        self.kinds: dict[str, list[str]] = defaultdict(list)
        self.detail: dict[str, Counter[str]] = defaultdict(Counter)

    def problem(self, kind: str, key: str, example: str) -> None:
        self.detail[kind][key] += 1
        self.kinds[kind].append(example)

    def print(self) -> None:
        print(f"\n{self.title}: {self.checked} projected CRSs checked, {self.agree} fully agree")
        for note, n in sorted(self.notes.items()):
            print(f"  {note}: {n}")
        for kind in sorted(self.kinds, key=lambda k: -len(self.kinds[k])):
            print(f"  {kind}: {len(self.kinds[kind])}")
            for key, n in self.detail[kind].most_common():
                print(f"    {n:5}  {key}")
            for ex in self.kinds[kind][: self.examples]:
                print(f"           e.g. {ex}")


def records(path: Path):
    with path.open() as f:
        next(f)  # header
        for line in f:
            yield json.loads(line)


# The methods PROJ's selectSphericalOrEllipsoidal() swaps, filled in from
# proj_constants.h by set_sphere_pairs(): each code to its other variant.
SPHERE_PAIRS: dict[int, int] = {}
SPHERICAL: set[int] = set()


def set_sphere_pairs(consts: dict[str, object]) -> None:
    for base in ("LAMBERT_CYLINDRICAL_EQUAL_AREA", "LAMBERT_AZIMUTHAL_EQUAL_AREA", "EQUIDISTANT_CYLINDRICAL"):
        ell = consts[f"EPSG_CODE_METHOD_{base}"]
        sph = consts[f"EPSG_CODE_METHOD_{base}_SPHERICAL"]
        SPHERE_PAIRS[ell], SPHERE_PAIRS[sph] = sph, ell
        SPHERICAL.add(sph)


def sphere_variant(code: int, w1: Wkt1Proj) -> int:
    """PROJ's selectSphericalOrEllipsoidal(): CEA, LAEA and Equidistant
    Cylindrical go by the ellipsoid, whatever the name said."""
    if code in SPHERE_PAIRS and (code in SPHERICAL) != w1.sphere:
        return SPHERE_PAIRS[code]
    return code


EPSG_NAMED = "outside PROJ's table: written under EPSG's method name (spaces as _) and parameter names"


def epsg_named(w1: Wkt1Proj) -> bool:
    """PROJ writes a method it has no WKT1 (or ESRI) mapping for under EPSG's
    names: the method's with spaces as underscores, the parameters' as they
    are. No WKT1 or ESRI parameter name has a space."""
    return bool(w1.params) and all(" " in n for n, _ in w1.params)


def gdal_read_method(cands: list[PMethod], w1: Wkt1Proj) -> tuple[int | None, str | None]:
    """The EPSG method PROJ's WKT1 reader (buildProjectionStandard) gives,
    and the special case that decided it, if any."""
    name = canon(w1.projection)
    values = {n.lower(): v for n, v in w1.params}
    if name == "mercator1sp":
        lat0 = values.get("latitude_of_origin")
        if lat0:
            return 9805, "Mercator_1SP with a latitude_of_origin read as Mercator_2SP"
        if w1.proj4 and "+nadgrids=@null" in w1.proj4 and w1.proj4.startswith("+proj=merc"):
            return 1024, "Mercator_1SP with the PROJ4 Web Mercator extension read as 1024"
    if name == "polarstereographic":
        lat0 = values.get("latitude_of_origin")
        if lat0 is not None and values.get("scale_factor", 1.0) == 1.0:
            return 9829, "Polar_Stereographic with scale_factor 1 read as variant B"
        if lat0 is not None and abs(lat0) == 90.0:
            return 9810, "Polar_Stereographic at a pole with another scale read as variant A"
        return None, None  # PROJ keeps the name, with no EPSG method
    first = cands[0].epsg
    if name == "krovak":
        if w1.axes[:2] == ["SOUTH", "WEST"]:
            return 9819, "Krovak with AXIS south, west read as 9819"
        return first, "Krovak without AXIS south, west read as 1041 (the first entry)"
    chosen = sphere_variant(first, w1)
    if chosen != first:
        return chosen, "spherical/ellipsoidal variant chosen by the ellipsoid"
    return first, None


def validate_gdal(t: Tables, path: Path, examples: int) -> Report:
    rep = Report("WKT1_GDAL against METHODS (read as PROJ reads it)", examples)
    by_name: dict[str, list[PMethod]] = defaultdict(list)
    for m in t.methods:
        by_name[canon(m.wkt1)].append(m)
    by_code = {}
    for m in t.methods:
        by_code.setdefault(m.epsg, m)
    dropped_params = {(m.epsg, p.epsg) for m, p in t.dropped_params}
    for rec in records(path):
        w = rec["wkt"]
        if not w.get("WKT2_2019") or not w.get("WKT1_GDAL"):
            continue
        p2 = find_projected(parse_wkt(w["WKT2_2019"]), ("PROJCRS",))
        p1 = find_projected(parse_wkt(w["WKT1_GDAL"]), ("PROJCS",))
        if p2 is None or p1 is None:
            continue
        rep.checked += 1
        code = rec["code"]
        w2 = wkt2_conversion(p2)
        w1 = wkt1_projection(p1)
        same = (rec.get("read") or {}).get("WKT1_GDAL", {}).get("equals")
        tag = f"EPSG:{code} {w1.projection} (PROJ reads it back as equal: {same})"
        cands = by_name.get(canon(w1.projection), [])
        codes = sorted({m.epsg for m in cands})
        if not cands:
            if epsg_named(w1):
                rep.problem(EPSG_NAMED, f"{w1.projection} (WKT2 method {w2.method})", tag)
            else:
                rep.problem("unknown projection name", f"{w1.projection} (WKT2 method {w2.method})", tag)
            continue
        read, rule = gdal_read_method(cands, w1)
        if rule:
            rep.notes[rule] += 1
        if len(codes) > 1:
            rep.notes[f"{w1.projection} names {codes}"] += 1
        if read != w2.method:
            kind = "ambiguous method" if w2.method in codes else "method mismatch"
            rep.problem(kind, f"{w1.projection} -> {codes}, PROJ reads {read}, WKT2 says {w2.method}", tag)
            continue
        ok = True
        method = by_code[read]
        if read == 1024:
            # PROJ reads Web Mercator from the PROJ4 extension; the PARAMETERs
            # are Mercator_1SP's, with scale_factor 1.
            method = by_code[9804]
            values = {n.lower(): v for n, v in w1.params}
            if values.pop("scale_factor", 1.0) != 1.0:
                rep.problem("value mismatch", "Web Mercator: scale_factor", tag)
                ok = False
            w1.params = [(n, v) for n, v in w1.params if n.lower() != "scale_factor"]
        if rule and rule.startswith("Mercator_1SP with a latitude"):
            method = by_code[9805]
        seen = set()
        order = []
        for name, value in w1.params:
            param = next((p for p in method.params if p.wkt1.lower() == name.lower()), None)
            if param is None:
                rep.problem("unknown parameter", f"{w1.projection}: {name}", tag)
                ok = False
                continue
            order.append(method.params.index(param))
            seen.add(param.epsg)
            if param.epsg not in w2.params:
                rep.problem("parameter not in WKT2", f"{w1.projection}: {name} ({param.epsg})", tag)
                ok = False
                continue
            v2, unit2 = w2.params[param.epsg]
            unit = param.unit
            if unit2 != param.unit and unit2 != "NONE":
                # PROJ converts by the unit the value has, not by what the
                # parameter measures.
                rep.notes[f"EPSG gives {w1.projection} {name} a {unit2} unit (values compared as such)"] += 1
                unit = unit2
            v1 = to_si(value, unit, w1)
            if not close(v1, v2):
                rep.problem("value mismatch", f"{w1.projection}: {name}", f"{tag} {name} {v1!r} vs {v2!r}")
                ok = False
        if order != sorted(order):
            rep.problem("parameter order differs from the table", w1.projection, tag)
            ok = False
        for p_code, (v2, _) in w2.params.items():
            if p_code in seen:
                continue
            if (method.epsg, p_code) in dropped_params and v2 == 0:
                rep.notes[f"WKT2 parameter {p_code} of method {w2.method} left out of WKT1 (0, as PROJ requires)"] += 1
                continue
            rep.problem("WKT2 parameter missing from WKT1", f"{w1.projection}: EPSG param {p_code}", tag)
            ok = False
        if ok:
            rep.agree += 1
    return rep


def esri_select(cands: list[EMethod], w1: Wkt1Proj) -> EMethod | None:
    """PROJ's getESRIMapping(): the candidate whose parameters match best."""
    present = {n.lower(): v for n, v in w1.params}
    best, best_count = None, -1
    for m in cands:
        count = 0
        for p in m.params:
            if p.esri.lower() in present:
                if p.wkt2 is None and p.fixed != present[p.esri.lower()]:
                    count = -1
                    break
                count += 1
        if count > best_count:
            best, best_count = m, count
    return best


def validate_esri(t: Tables, all_esri: list[EMethod], all_methods: list[PMethod], path: Path, examples: int) -> Report:
    rep = Report("WKT1_ESRI against ESRI_METHODS", examples)
    by_name: dict[str, list[EMethod]] = defaultdict(list)
    for e in t.esri:
        by_name[e.esri.lower()].append(e)
    all_by_name: dict[str, list[EMethod]] = defaultdict(list)
    for e in all_esri:
        all_by_name[e.esri.lower()].append(e)
    unit_of: dict[int, str] = {}
    for m in all_methods:
        for p in m.params:
            if p.epsg:
                unit_of.setdefault(p.epsg, p.unit)
    for rec in records(path):
        w = rec["wkt"]
        if not w.get("WKT2_2019") or not w.get("WKT1_ESRI"):
            continue
        p2 = find_projected(parse_wkt(w["WKT2_2019"]), ("PROJCRS",))
        p1 = find_projected(parse_wkt(w["WKT1_ESRI"]), ("PROJCS",))
        if p2 is None or p1 is None:
            continue
        rep.checked += 1
        code = rec["code"]
        w2 = wkt2_conversion(p2)
        w1 = wkt1_projection(p1)
        same = (rec.get("read") or {}).get("WKT1_ESRI", {}).get("equals")
        tag = f"EPSG:{code} {w1.projection} (PROJ reads it back as equal: {same})"
        name = w1.projection.lower()
        if epsg_named(w1):
            rep.problem(EPSG_NAMED, f"{w1.projection} (WKT2 method {w2.method})", tag)
            continue
        cands = by_name.get(name, [])
        special = None
        method_code = None
        if not cands:
            # PROJ's own special case: ESRI "Stereographic" (no EPSG code) at a
            # pole is read as Polar Stereographic (variant A).
            lat0 = next((v for n, v in w1.params if n.lower() == "latitude_of_origin"), None)
            if name == "stereographic" and lat0 is not None and abs(lat0) == 90.0:
                cands = all_by_name[name]
                method_code = 9810
                special = "Stereographic at a pole read as 9810 (PROJ special case)"
            else:
                why = "not in ESRI_METHODS" if name in all_by_name else "unknown to PROJ"
                rep.problem("unknown projection name", f"{w1.projection} ({why}; WKT2 method {w2.method})", tag)
                continue
        chosen = esri_select(cands, w1)
        if chosen is None:
            rep.problem("no entry matches the fixed parameters", w1.projection, tag)
            continue
        if method_code is None:
            method_code = chosen.epsg
            if name == "krovak" and "_east_north" in w1.name.lower():
                method_code = 1041
                special = "Krovak named *_East_North read as 1041 (PROJ special case)"
            elif sphere_variant(method_code, w1) != method_code:
                method_code = sphere_variant(method_code, w1)
                special = "spherical/ellipsoidal variant chosen by the ellipsoid (PROJ special case)"
        if method_code != w2.method:
            if w2.method == SPHERE_PAIRS.get(method_code) or {method_code, w2.method} == {1125, 9832}:
                kind = "ambiguous method"
            elif (method_code, w2.method) == (9805, 9804):
                kind = "method converted by PROJ's ESRI writer"
            else:
                kind = "method mismatch"
            rep.problem(kind, f"{w1.projection} -> {method_code}, WKT2 says {w2.method}", tag)
            continue
        if len({e.epsg for e in cands}) > 1:
            rep.notes[f"{w1.projection}: chosen among {sorted({e.epsg for e in cands})} by parameters"] += 1
        if special:
            rep.notes[special] += 1
        ok = True
        present = {n.lower(): v for n, v in w1.params}
        seen = set()
        for p in chosen.params:
            if p.esri.lower() not in present:
                if p.wkt2 is not None and not p.is_fixed:
                    rep.problem("ESRI parameter missing", f"{w1.projection}: {p.esri}", tag)
                    ok = False
                continue
            value = present.pop(p.esri.lower())
            if p.wkt2 is None:
                continue  # fixed value, already checked by esri_select
            seen.add(p.epsg)
            if p.epsg not in w2.params:
                rep.problem("parameter not in WKT2", f"{w1.projection}: {p.esri} ({p.epsg})", tag)
                ok = False
                continue
            v2, unit2 = w2.params[p.epsg]
            unit = unit_of.get(p.epsg, unit2)
            v1 = to_si(value, unit, w1)
            if not close(v1, v2, angular=unit == "ANGULAR"):
                rep.problem("value mismatch", f"{w1.projection}: {p.esri}", f"{tag} {p.esri} {v1!r} vs {v2!r}")
                ok = False
        for extra in present:
            rep.problem("unknown parameter", f"{w1.projection}: {extra}", tag)
            ok = False
        for p_code, (v2, _) in w2.params.items():
            if p_code in seen:
                continue
            # PROJ's own special case: Hotine without "Rectified_Skew" in the
            # name has no skew angle; PROJ takes it from the azimuth.
            if (
                p_code == 8814
                and w2.method in (9812, 9815)
                and not name.startswith("rectified_skew")
                and 8813 in w2.params
                and close(w2.params[8813][0], v2, angular=True)
            ):
                rep.notes["Hotine angle to skew grid taken from Azimuth (PROJ special case)"] += 1
                continue
            rep.problem("WKT2 parameter missing from ESRI", f"{w1.projection}: EPSG param {p_code}", tag)
            ok = False
        if ok:
            rep.agree += 1
    return rep


# --- Main --------------------------------------------------------------------


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--wkt", type=Path, default=WKT_JSONL, help="PROJ's WKT for every EPSG CRS (wkt.jsonl)")
    ap.add_argument("--examples", type=int, default=3, help="examples to print per disagreement kind")
    args = ap.parse_args()

    methods, esri, consts = parse_proj(read_sources(tarball()))
    set_sphere_pairs(consts)
    t = build_tables(methods, esri)
    OUT.write_text(render(t))
    n_params = sum(len(m.params) for m in t.methods)
    n_esri_params = sum(len(e.params) for e in t.esri)
    print(f"wrote {OUT.relative_to(REPO)}")
    print(f"  PROJ: {len(methods)} projection method mappings, {len(esri)} ESRI mappings")
    print(f"  METHODS: {len(t.methods)} methods, {n_params} parameters")
    print(f"  dropped, no EPSG code: {len(t.dropped_no_code)}")
    print(f"  dropped, EPSG method without a WKT1 name: {len(t.dropped_no_wkt1)}")
    for m in t.dropped_no_wkt1:
        print(f"    {m.epsg} {m.wkt2}")
    for m, p in t.dropped_params:
        print(f"  parameter without a WKT1 name left out: {m.wkt1} ({m.epsg}): {p.epsg} {p.wkt2}")
    zero = [(m, p) for m in t.methods for p in m.params if not p.epsg]
    for m, p in zero:
        print(f"  parameter without an EPSG code kept (epsg 0): {m.wkt1} ({m.epsg}): {p.wkt1}")
    print(f"  ESRI_METHODS: {len(t.esri)} projections, {n_esri_params} parameters")
    print(f"  ESRI dropped: {len(t.esri_dropped)}")
    for e, why in t.esri_dropped:
        print(f"    {e.esri} ({e.wkt2}): {why}")

    if not args.wkt.exists():
        print(f"\n{args.wkt} not found; run scripts/gen_wkt_vectors.py --all to validate")
        return 0
    validate_gdal(t, args.wkt, args.examples).print()
    validate_esri(t, esri, methods, args.wkt, args.examples).print()
    return 0


if __name__ == "__main__":
    sys.exit(main())
