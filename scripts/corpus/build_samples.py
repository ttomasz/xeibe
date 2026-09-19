#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["lxml>=5"]
# ///
"""Build the curated test samples from tests/data/samples.toml.

    scripts/corpus/build_samples.py [--only NAME] [--no-validate] [--no-gdal] [--no-axis-evidence]

For each [[sample]]: extract it from example_data/ (scripts/corpus/extract_sample.py),
validate it against its schemas (scripts/validate-gml --net), record GDAL's reference
output (scripts/gdal ogrinfo -ro -al) next to it as <file>.gdal.txt, and compute
sha256/size. The recorded `axis_order` of each sample is verified with independent
evidence (scripts/corpus/axis_evidence.py, needs scripts/corpus/reference_data.py once);
the build fails if any is not confirmed. Provenance (download URL, retrieval time) is looked up in
example_data/downloads/log.jsonl or example_data/wfs/requests.tsv.

Writes tests/data/samples.json (machine-readable) and tests/data/BOM.md (bill of materials).
"""
import argparse
import datetime
import hashlib
import importlib.util
import json
import re
import subprocess
import sys
import tomllib
from pathlib import Path

PROJECT = Path(__file__).resolve().parents[2]
DATA = PROJECT / "tests" / "data"
SPEC = DATA / "samples.toml"
MANIFEST = DATA / "samples.json"
BOM = DATA / "BOM.md"

spec_ = importlib.util.spec_from_file_location("extract_sample", Path(__file__).with_name("extract_sample.py"))
extract_sample = importlib.util.module_from_spec(spec_)
spec_.loader.exec_module(extract_sample)


def provenance(source: str) -> dict:
    """Download URL and retrieval time for a file in example_data/."""
    log = PROJECT / "example_data" / "downloads" / "log.jsonl"
    if log.exists():
        for line in log.open():
            r = json.loads(line)
            if r.get("path") == source:
                return {"download_url": r["url"], "retrieved": r["retrieved"], "portal": r["portal"]}
    tsv = PROJECT / "example_data" / "wfs" / "requests.tsv"
    rel = source.removeprefix("example_data/wfs/")
    if tsv.exists():
        for line in tsv.open():
            ts, status, path, url = line.rstrip("\n").split("\t")
            if path == rel and status == "ok":
                return {"download_url": url, "retrieved": ts, "portal": "WFS GetFeature"}
    return {"download_url": None, "retrieved": None, "portal": None}


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def validate(path: Path) -> dict:
    """xmllint first; if it can't decide, Xerces via GDAL's GMLAS driver.

    Status: valid (with validator) | invalid (validity errors) | schema unavailable
    (an imported schema could not be fetched/parsed) | not validated.
    """
    try:
        r = subprocess.run([str(PROJECT / "scripts" / "validate-gml"), "--net", str(path)],
                           capture_output=True, text=True, timeout=600)
        xml_out = (r.stdout + r.stderr).strip().splitlines()
        if r.returncode == 0:
            return {"status": "valid", "validator": "xmllint (libxml2)"}
    except subprocess.TimeoutExpired:
        xml_out = ["xmllint: timeout"]
    rel = path.relative_to(DATA)
    try:
        g = subprocess.run([str(PROJECT / "scripts" / "gdal"), "ogrinfo", "-ro", "-so", "-q", f"GMLAS:{rel}",
                            "-oo", "VALIDATE=YES", "-oo", "FAIL_IF_VALIDATION_ERROR=YES",
                            "--config", "GDAL_HTTP_TIMEOUT", "60"],
                           capture_output=True, text=True, cwd=DATA, timeout=900)
        gml_out = [l for l in (g.stdout + g.stderr).splitlines() if l.strip()]
        if g.returncode == 0:
            return {"status": "valid", "validator": "Xerces (GDAL GMLAS)",
                    "notes": [f"xmllint could not decide: {l}" for l in xml_out[:1]]}
    except subprocess.TimeoutExpired:
        gml_out = ["GMLAS: timeout"]
    clean = lambda ls: [l.replace(str(PROJECT) + "/", "") for l in ls]
    content_errors = [l for l in gml_out if any(k in l for k in (
        "is not allowed", "is not declared", "does not match", "is not a valid value", "not valid", "is not expected", "Validation errors encountered"))]
    unresolved = [l for l in xml_out + gml_out if any(k in l for k in (
        "Failed to locate a schema", "failed to load external entity", "Cannot resolve", "HTTP error code",
        "invalid document structure", "loadGrammar failed", "expected end of tag"))]
    if content_errors:
        return {"status": "invalid", "validator": "Xerces (GDAL GMLAS)", "messages": clean(content_errors[:4])}
    if unresolved:
        return {"status": "schema unavailable", "messages": clean(unresolved[:4])}
    validity = [l for l in xml_out if "validity error" in l]
    if validity:
        return {"status": "invalid", "validator": "xmllint (libxml2)", "messages": clean(validity[:3])}
    return {"status": "not validated", "messages": clean((xml_out[:2] + gml_out[:3]))}


def gdal_reference(path: Path) -> dict:
    rel = path.relative_to(DATA)
    r = subprocess.run([str(PROJECT / "scripts" / "gdal"), "ogrinfo", "-ro", "-al", "-oo", "DOWNLOAD_SCHEMA=NO", str(rel)],
                       capture_output=True, text=True, cwd=DATA, timeout=600)
    lines = [l for l in r.stdout.splitlines() if not l.startswith("INFO: Open of") and "using driver" not in l]
    driver = next((l.split("`", 1)[1].split("'", 1)[0] for l in r.stdout.splitlines() if "using driver" in l), None)
    out = path.with_name(path.name + ".gdal.txt")
    out.write_text("\n".join(lines) + "\n")
    layers = [l for l in lines if l.startswith("Layer name:")]
    counts = [l for l in lines if l.startswith("Feature Count:")]
    return {"file": str(out.relative_to(DATA)), "driver": driver, "exit_code": r.returncode,
            "layers": len(layers), "feature_counts": [int(c.split(":")[1]) for c in counts],
            "stderr": [l for l in r.stderr.splitlines() if l.strip()][:5]}


FIRST_POS = re.compile(r"<(?:\w+:)?(?:pos|posList)\b[^>]*>\s*([-\d.eE+]+)\s+([-\d.eE+]+)"
                       r"|<(?:\w+:)?coordinates\b[^>]*>\s*([-\d.eE+]+),([-\d.eE+]+)")
GDAL_FIRST = re.compile(r"^\s+[A-Z ]+?(?: Z)? \(+([-\d.eE+]+) ([-\d.eE+]+)", re.M)


def axis_check(sample: dict, out: Path, gdal: dict | None) -> dict | None:
    """Compare the true x/y of the first position with GDAL's interpretation."""
    order = sample.get("axis_order")
    if not order:
        return None
    text = out.read_text(encoding="utf-8")
    body = text[max(text.find("featureMember"), text.find(":member"), 0):]
    m = FIRST_POS.search(body)
    a, b = (m.group(1), m.group(2)) if m.group(1) else (m.group(3), m.group(4))
    written = [float(a), float(b)]
    expected = written if order == "x/y" else written[::-1]
    res = {"source_order": order, "first_position_as_written": written, "expected_first_xy": expected}
    if gdal:
        g = GDAL_FIRST.search((DATA / gdal["file"]).read_text())
        if g:
            got = [float(g.group(1)), float(g.group(2))]
            res["gdal_first_xy"] = got
            res["gdal_agrees"] = all(abs(x - y) < 1e-6 for x, y in zip(got, expected))
    return res


def axis_evidence() -> dict:
    """Run scripts/corpus/axis_evidence.py in the GDAL container for all samples with axis_order."""
    r = subprocess.run([str(PROJECT / "scripts" / "gdal"), "python3", "scripts/corpus/axis_evidence.py"],
                       capture_output=True, text=True, cwd=PROJECT, timeout=1800)
    if not r.stdout.strip():
        sys.exit(f"axis_evidence.py failed:\n{r.stderr}")
    return json.loads(r.stdout)


def evidence_summary(ev: dict) -> str:
    """One line for the BOM: status and the checks that reject the swapped reading."""
    parts = []
    for c in ev.get("checks", []):
        rec, swp = c["recorded_order_inside"], c["swapped_order_inside"]
        s = f"{c['check']} ({c['detail']}): recorded {rec:.0%} inside, swapped {swp:.0%}"
        km = c.get("first_position_km_outside", {}).get("swapped")
        if km:
            s += f", first position {km:g} km away"
        parts.append(s + (" — **rejects swapped**" if c["rejects_swapped"] else ""))
    scope = " (whole source document)" if ev.get("scope") == "source" else ""
    head = f"**{ev['status']}**{scope}, {ev.get('positions_checked', 0)} positions" if "checks" in ev \
        else f"**{ev['status']}**: {ev.get('error')}"
    return head + ("; " + "; ".join(parts) if parts else "")


def build(sample: dict, args, prev: dict | None = None) -> dict:
    out = DATA / sample["file"]
    rules = [extract_sample.Rule(r) for r in sample.get("rules", ["count=3"])]
    src = PROJECT / sample["source"]
    with extract_sample.open_source(src, sample.get("member")) as fh:
        xml, info = extract_sample.extract(fh, rules, int(sample.get("max_scan_mb", 0) * 1e6))
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_bytes(xml)
    rec = {k: v for k, v in sample.items() if k not in ("rules",)}
    rec["companions"] = []
    for name in sample.get("companion_files", []):
        target = out.parent / Path(name).name
        with extract_sample.open_source(src, name if sample.get("member") else None) as fh:
            target.write_bytes(fh.read())
        rec["companions"].append({"file": str(target.relative_to(DATA)), "original": name,
                                  "sha256": sha256(target), "bytes": target.stat().st_size})
    rec["extraction"] = {"rules": sample.get("rules", ["count=3"]), "features_kept": info["features_kept"],
                         "features_scanned": info["features_seen"], "changes": info["changes"],
                         "original_encoding": info["encoding"]}
    rec["provenance"] = provenance(sample["source"])
    rec["sha256"] = sha256(out)
    rec["bytes"] = out.stat().st_size
    # --no-validate / --no-gdal keep the previous results (the sample bytes are deterministic)
    prev = prev if prev and prev.get("sha256") == rec["sha256"] else {}
    rec["validation"] = validate(out) if args.validate else prev.get("validation", {"status": "skipped"})
    rec["gdal"] = gdal_reference(out) if args.gdal else prev.get("gdal")
    rec["axis"] = axis_check(sample, out, rec["gdal"])
    print(f"{sample['name']:45} {rec['bytes']:>8} B  {info['features_kept']} features  "
          f"{rec['validation']['status']:13}  gdal: {rec['gdal'] and rec['gdal']['driver']}", file=sys.stderr, flush=True)
    return rec


def bom(spec: dict, samples: list[dict], statics: list[dict]) -> str:
    lic = spec["licences"]
    L = ["# Bill of materials: test data",
         "",
         "Generated by `scripts/corpus/build_samples.py` from `samples.toml`, do not edit by hand.",
         f"Last build: {datetime.date.today().isoformat()}. Machine-readable version: `samples.json`.",
         "",
         "Every file under `tests/data/` that was derived from third-party data is listed here with",
         "its origin, licence and the attribution the licence requires. Samples are small excerpts",
         "(a few features) of the original datasets; the extraction rules and every change made to",
         "the documents are recorded per sample. Reference outputs (`*.gdal.txt`) were produced with",
         f"GDAL ({spec['defaults']['gdal_image']}) and are factual derivatives of the samples.",
         "They record GDAL's *interpretation*, not ground truth: for axis order the true order",
         "is stored per sample (`axis_order`), verified with independent evidence (below) and",
         "compared with GDAL's result.",
         "",
         "Axis order evidence (`scripts/corpus/axis_evidence.py`) reads every position both ways",
         "and tests each reading against the CRS area of use and against a region the features are",
         "known to lie in from something other than their geometry (a TERYT code in the attributes,",
         "the file name, the publisher), using PRG boundaries (GUGiK, official material; \"Wykorzystano/opracowano",
         "na podstawie materiałów państwowego zasobu geodezyjnego i kartograficznego\") and Natural Earth",
         "countries (public domain). These reference datasets are not committed",
         "(`scripts/corpus/reference_data.py` downloads them).",
         "",
         "## Summary",
         "",
         "| File | Publisher | Licence | Covers |",
         "|---|---|---|---|"]
    for s in samples:
        L.append(f"| [`{s['file']}`](#{s['name']}) | {s['publisher']} | {lic[s['licence']]['name'].split(' (')[0]} | "
                 f"{'; '.join(s['covers'][:3])} |")
    for s in statics:
        L.append(f"| [`{s['file']}`](#{s['name']}) | {s['publisher']} | {s['licence_name']} | {'; '.join(s['covers'][:2])} |")
    L += ["", "## Samples", ""]
    for s in samples:
        l = lic[s["licence"]]
        p = s["provenance"]
        e = s["extraction"]
        v = s["validation"]
        g = s["gdal"] or {}
        L += [f"### {s['name']}", "",
              f"- **File:** `{s['file']}` ({s['bytes']} bytes, sha256 `{s['sha256']}`)",
              f"- **Covers:** {'; '.join(s['covers'])}",
              f"- **Publisher:** {s['publisher']}",
              f"- **Dataset:** [{s['dataset']}]({s['dataset_url']})",
              f"- **Downloaded from:** <{p['download_url']}> on {p['retrieved']}" + (f" (via {p['portal']})" if p['portal'] else ""),
              f"- **Original file:** `{Path(s['source']).name}`" + (f", entry `{s['member']}`" if s.get('member') else ""),
              *[f"- **Companion file:** `{c['file']}` (copied unchanged from `{c['original']}`, sha256 `{c['sha256']}`)"
                for c in s.get("companions", [])],
              f"- **Licence:** " + (f"[{l['name']}]({l['url']})" if l["url"] else l["name"]),
              f"- **Licence evidence:** {s['licence_evidence']}",
              f"- **Attribution:** {s['attribution']}",
              f"- **Extraction:** rules `{'`, `'.join(e['rules'])}`; kept {e['features_kept']} of {e['features_scanned']} features scanned",
              f"- **Modifications:** " + ("; ".join(c for c in e["changes"] if not c.startswith("kept ")) or "none (features copied verbatim; document re-serialized as UTF-8)"),
              f"- **Schema validation:** {v['status']}" + (f" ({v['validator']})" if v.get("validator") else "")
              + (f" — `{v['messages'][0][:200]}`" if v.get("messages") else ""),
              f"- **GDAL reference:** `{g.get('file')}` (driver {g.get('driver')}, feature counts {g.get('feature_counts')})" if g else "- **GDAL reference:** not built",
              *([f"- **Axis order:** source is {s['axis']['source_order']}; expected first x/y `{s['axis']['expected_first_xy']}`; "
                 + (f"GDAL gives `{s['axis'].get('gdal_first_xy')}` — "
                    + ("agrees" if s['axis'].get('gdal_agrees') else "**GDAL gets this wrong**") if 'gdal_first_xy' in s['axis'] else "GDAL value not found")]
                if s.get("axis") else []),
              *([f"- **Axis order evidence:** {evidence_summary(s['axis']['evidence'])}"]
                if s.get("axis", {}) and s["axis"].get("evidence") else []),
              ""]
    for s in statics:
        L += [f"### {s['name']}", "",
              f"- **File:** `{s['file']}` ({s['bytes']} bytes, sha256 `{s['sha256']}`)",
              f"- **Covers:** {'; '.join(s['covers'])}",
              f"- **Source:** [{s['dataset']}]({s['dataset_url']}) — {s['publisher']}",
              f"- **Licence:** [{s['licence_name']}]({s['licence_url']}) — {s['licence_evidence']}",
              f"- **Attribution:** {s['attribution']}",
              f"- **Generated by:** `{s['generated_by']}`",
              ""]
    return "\n".join(L)


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--only", action="append")
    ap.add_argument("--no-validate", dest="validate", action="store_false")
    ap.add_argument("--no-gdal", dest="gdal", action="store_false")
    ap.add_argument("--no-axis-evidence", dest="axis_evidence", action="store_false")
    args = ap.parse_args()
    spec = tomllib.loads(SPEC.read_text())
    previous = {r["name"]: r for r in json.loads(MANIFEST.read_text())["samples"]} if MANIFEST.exists() else {}
    samples = []
    for s in spec["sample"]:
        if s["licence"] not in spec["licences"]:
            sys.exit(f"{s['name']}: unknown licence key {s['licence']}")
        if args.only and s["name"] not in args.only and s["name"] in previous:
            samples.append(previous[s["name"]])
            continue
        samples.append(build(s, args, previous.get(s["name"])))
    unconfirmed = []
    if args.axis_evidence:
        evidence = axis_evidence()
        for rec in samples:
            if rec.get("axis") is not None:
                rec["axis"]["evidence"] = evidence.get(rec["name"], {"status": "missing"})
                if rec["axis"]["evidence"]["status"] != "confirmed":
                    unconfirmed.append(f"{rec['name']}: {rec['axis']['evidence']['status']}")
    statics = []
    for s in spec.get("static", []):
        f = DATA / s["file"]
        statics.append({**s, "sha256": sha256(f), "bytes": f.stat().st_size})
    MANIFEST.write_text(json.dumps({"samples": samples, "static": statics, "licences": spec["licences"]},
                                   indent=1, ensure_ascii=False) + "\n")
    BOM.write_text(bom(spec, samples, statics) + "\n")
    print(f"-> {MANIFEST.relative_to(PROJECT)}, {BOM.relative_to(PROJECT)}", file=sys.stderr)
    if unconfirmed:
        sys.exit("axis order not confirmed by evidence:\n  " + "\n  ".join(unconfirmed))


if __name__ == "__main__":
    main()
