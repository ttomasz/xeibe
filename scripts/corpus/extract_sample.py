#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["lxml>=5"]
# ///
"""Extract a small sample (a few features) from a large GML/WFS document.

    scripts/corpus/extract_sample.py SRC [--member ZIP_ENTRY] OUT
        [--rule 'contains=Arc,Circle;count=2'] [--rule 'per_layer=1'] [--rule 'layer=AD_PunktAdresowy;count=3']
        [--max-scan-mb 0]

Streams SRC (optionally an entry inside a zip) and keeps:
  * the root element with all its attributes and namespace declarations,
  * the chain of container elements down to the feature members
    (e.g. NAS: AX_Bestandsdatenauszug/enthaelt/wfs:FeatureCollection),
  * non-member siblings along that chain (e.g. gml:boundedBy, NAS metadata),
  * the features selected by the rules, in document order.

Rules (each keeps up to `count` features; a feature is kept if any rule with
quota left matches it):
  contains=Name1,Name2   feature contains an element with one of these local names
  layer=LocalName        feature element local name
  per_layer=N            N features for every feature type encountered
  count=N                quota (default 1)

Changes made to the document are returned/printed as a list (numberReturned etc.).
"""
import argparse
import copy
import json
import sys
import zipfile
from pathlib import Path

from lxml import etree

GML_NS = {"http://www.opengis.net/gml", "http://www.opengis.net/gml/3.2"}
MEMBER_ONE = {"featureMember", "member"}  # one feature per element (gml / wfs 2.0)
MEMBER_MANY = {"featureMembers"}


def local(tag) -> str:
    return tag.rsplit("}", 1)[-1] if isinstance(tag, str) else ""


def ns(tag) -> str:
    return tag[1:].split("}", 1)[0] if isinstance(tag, str) and tag.startswith("{") else ""


def is_member_container(el) -> str | None:
    n = ns(el.tag)
    if n in GML_NS or n.startswith("http://www.opengis.net/wfs"):
        if local(el.tag) in MEMBER_ONE:
            return "one"
        if local(el.tag) in MEMBER_MANY:
            return "many"
    return None


class Rule:
    def __init__(self, spec: str):
        self.contains, self.layer, self.per_layer, self.count = None, None, None, 1
        for part in filter(None, spec.split(";")):
            k, _, v = part.partition("=")
            k = k.strip()
            if k == "contains":
                self.contains = set(v.split(","))
            elif k == "layer":
                self.layer = v
            elif k == "per_layer":
                self.per_layer = int(v)
            elif k == "count":
                self.count = int(v)
            else:
                raise ValueError(f"unknown rule key {k!r}")
        self.taken = 0
        self.taken_per_layer: dict[str, int] = {}

    def wants(self, feature) -> bool:
        name = local(feature.tag)
        if self.layer and name != self.layer:
            return False
        if self.per_layer is not None:
            return self.taken_per_layer.get(name, 0) < self.per_layer and self._contains_ok(feature)
        return self.taken < self.count and self._contains_ok(feature)

    def _contains_ok(self, feature) -> bool:
        if not self.contains:
            return True
        return any(local(e.tag) in self.contains for e in feature.iter())

    def take(self, feature) -> None:
        self.taken += 1
        name = local(feature.tag)
        self.taken_per_layer[name] = self.taken_per_layer.get(name, 0) + 1

    def done(self) -> bool:
        return self.per_layer is None and self.taken >= self.count


def shallow(el):
    """Copy an element without children (keeps attributes, nsmap and text)."""
    new = etree.Element(el.tag, attrib=dict(el.attrib), nsmap=el.nsmap)
    new.text = el.text
    return new


def extract(fh, rules: list[Rule], max_bytes: int = 0) -> tuple[bytes, dict]:
    """Keep the whole document except feature members; re-insert selected features.

    Non-member content (headers, boundedBy, NAS metadata) is small and kept as is.
    Each feature is inspected when it ends, then removed from the tree.
    """
    info = {"features_seen": 0, "features_kept": 0, "layers_seen": {}, "encoding": None, "changes": []}
    insert_at = None          # (member parent element, child index) of the first member container
    selected = []             # (kind, container element (one) or None (many), feature copy)
    ctx = etree.iterparse(fh, events=("start", "end"), huge_tree=True, resolve_entities=False,
                          no_network=True, load_dtd=False)
    root = None
    for event, el in ctx:
        if event == "start":
            if root is None:
                root = el
            if insert_at is None and is_member_container(el):
                parent = el.getparent()
                insert_at = (parent, parent.index(el))
            continue
        parent = el.getparent()
        kind = is_member_container(parent) if parent is not None else None
        if kind and isinstance(el.tag, str):
            info["features_seen"] += 1
            name = local(el.tag)
            info["layers_seen"][name] = info["layers_seen"].get(name, 0) + 1
            matching = [r for r in rules if r.wants(el)]
            if matching:
                for r in matching:
                    r.take(el)
                container = shallow(parent) if kind == "one" else parent
                selected.append((kind, container, copy.deepcopy(el)))
                info["features_kept"] += 1
            # Safe pattern for iterparse: clear the finished element, delete only
            # *previous* siblings (the parser may still attach text to this one).
            el.clear(keep_tail=False)
            while el.getprevious() is not None and kind == "many":
                parent.remove(el.getprevious())
        elif is_member_container(el) == "one" and parent is not None:
            el.clear(keep_tail=False)
            prev = el.getprevious()
            while prev is not None and is_member_container(prev) == "one":
                parent.remove(prev)
                prev = el.getprevious()
        if rules and all(r.done() for r in rules):
            info["changes"].append("stopped scanning once all rules were satisfied (rest of document dropped)")
            break
        if max_bytes and fh.tell() > max_bytes:
            info["changes"].append(f"stopped scanning after {max_bytes} bytes (rest of document dropped)")
            break
    if not selected:
        raise SystemExit("no features selected")
    info["encoding"] = root.getroottree().docinfo.encoding
    member_parent, index = insert_at
    for child in list(member_parent):
        kind = is_member_container(child)
        if kind == "one":
            member_parent.remove(child)
        elif kind == "many":
            for feature in list(child):
                child.remove(feature)
    for kind, container, feature in selected:
        if kind == "one":
            container.append(feature)
            member_parent.insert(index, container)
            index += 1
        else:
            container.append(feature)

    # fix counts on the collection(s) and drop server paging links
    kept = info["features_kept"]
    node = member_parent
    while node is not None:
        for attr in ("numberReturned", "numberOfFeatures"):
            if node.get(attr) is not None and node.get(attr) != str(kept):
                info["changes"].append(f"{local(node.tag)}@{attr}: {node.get(attr)} -> {kept}")
                node.set(attr, str(kept))
        for attr in ("next", "previous"):
            if node.get(attr) is not None:
                info["changes"].append(f"{local(node.tag)}@{attr} removed")
                del node.attrib[attr]
        node = node.getparent()
    xml = etree.tostring(root, xml_declaration=True, encoding="UTF-8")
    if info["encoding"] and info["encoding"].upper() not in ("UTF-8", "UTF8"):
        info["changes"].append(f"re-encoded from {info['encoding']} to UTF-8")
    info["changes"].append(f"kept {kept} of {info['features_seen']} features scanned")
    return xml, info


def open_source(src: Path, member: str | None):
    if member:
        z = zipfile.ZipFile(src)
        return z.open(member)
    return src.open("rb")


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("src", type=Path)
    ap.add_argument("out", type=Path)
    ap.add_argument("--member")
    ap.add_argument("--rule", action="append", default=[])
    ap.add_argument("--max-scan-mb", type=float, default=0)
    args = ap.parse_args()
    rules = [Rule(r) for r in (args.rule or ["count=3"])]
    with open_source(args.src, args.member) as fh:
        xml, info = extract(fh, rules, int(args.max_scan_mb * 1e6))
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_bytes(xml)
    print(json.dumps(info, ensure_ascii=False, indent=1))


if __name__ == "__main__":
    main()
