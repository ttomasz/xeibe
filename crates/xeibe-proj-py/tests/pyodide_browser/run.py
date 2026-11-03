#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["playwright==1.63.0"]
# ///
"""The Pyodide wheel's grid fetch in a real browser (docs/pyproj-phase3.md,
Pyodide).

The package's tests run under Pyodide in Node, where there is no
XMLHttpRequest, so they fetch grids through a fake one. This checks the
real one: Chromium loads Pyodide and the wheel, and converts St. George
Island to NAD83 (EPSG:8546, NADCON5), which needs two grids, once on the
page's main thread and once in a web worker. The grids come from a local
server on another origin that answers as cdn.proj.org does: range requests,
and CORS that allows only the `Range` header. Each run must fetch ranges
and match PROJ's results in tests/data/reprojection/operations-grids.jsonl.

    crates/xeibe-proj-py/tests/pyodide_browser/run.py \\
        --wheel target/pyodide-wheels/xeibe_proj-*.whl \\
        --pyodide "$(pyodide config get dist_dir)"

`--pyodide` is the directory of a Pyodide distribution (pyodide.mjs,
pyodide.asm.wasm, python_stdlib.zip, pyodide-lock.json) of the release the
wheel was built for; the cross-build environment's `dist_dir` is one. The
browser is Playwright's Chromium (`playwright install chromium`).
"""

from __future__ import annotations

import argparse
import functools
import http.server
import json
import sys
import threading
from pathlib import Path

from playwright.sync_api import sync_playwright

ROOT = Path(__file__).resolve().parents[4]
GRIDS = ROOT / "tests/data/reprojection/grids"
VECTORS = ROOT / "tests/data/reprojection/operations-grids.jsonl"
OPERATION = 8546

PAGE = """<!doctype html>
<meta charset="utf-8">
<title>xeibe-proj in Pyodide</title>
<script type="module">
import { loadPyodide } from "/pyodide/pyodide.mjs";

async function python(code) {
  const pyodide = await loadPyodide({ indexURL: "/pyodide/" });
  await pyodide.loadPackage(__WHEEL__);
  return pyodide.runPython(code);
}

window.onMainThread = python;
window.inWorker = (code) => new Promise((resolve, reject) => {
  const worker = new Worker("/worker.mjs", { type: "module" });
  worker.onmessage = (e) => e.data.error === undefined ? resolve(e.data.result) : reject(new Error(e.data.error));
  worker.onerror = (e) => reject(new Error(e.message));
  worker.postMessage(code);
});
window.ready = true;
</script>
"""

WORKER = """import { loadPyodide } from "/pyodide/pyodide.mjs";

onmessage = async (e) => {
  try {
    const pyodide = await loadPyodide({ indexURL: "/pyodide/" });
    await pyodide.loadPackage(__WHEEL__);
    postMessage({ result: pyodide.runPython(e.data) });
  } catch (err) {
    postMessage({ error: String(err) });
  }
};
"""

# Run in Pyodide, with __XS__, __YS__ and __CDN__ filled in.
CODE = """
import json, sys
from xeibe_proj import Transformer, _grids, network

assert sys.platform == "emscripten" and _grids._EMSCRIPTEN
_grids.CDN = __CDN__
network.set_network_enabled(True)
t = Transformer.from_crs(4138, 4269)
xs, ys = t.transform(__XS__, __YS__)
single = t.transform(__XS__[0], __YS__[0])
json.dumps({"xs": xs, "ys": ys, "single": single})
"""


class _Site(http.server.SimpleHTTPRequestHandler):
    """The page, its worker, Pyodide under /pyodide/ and the wheel under
    /wheel/."""

    extensions_map = {
        **http.server.SimpleHTTPRequestHandler.extensions_map,
        ".mjs": "text/javascript",
        ".js": "text/javascript",
        ".wasm": "application/wasm",
        ".json": "application/json",
    }

    def __init__(self, *args: object, pyodide: Path, wheel: Path, **kwargs: object) -> None:
        self.pyodide = pyodide
        self.wheel = wheel
        super().__init__(*args, **kwargs)  # type: ignore[arg-type]

    def do_GET(self) -> None:
        url = f"/wheel/{self.wheel.name}"
        pages = {"/": ("text/html", PAGE), "/worker.mjs": ("text/javascript", WORKER)}
        if self.path in pages:
            kind, text = pages[self.path]
            self._send(kind, text.replace("__WHEEL__", json.dumps(url)).encode())
        elif self.path == url:
            self._send("application/zip", self.wheel.read_bytes())
        elif self.path.startswith("/pyodide/") and "/.." not in self.path:
            path = self.pyodide / self.path.removeprefix("/pyodide/")
            if path.is_file():
                self._send(self.guess_type(str(path)), path.read_bytes())
            else:
                self.send_error(404)
        else:
            self.send_error(404)

    def _send(self, kind: str, body: bytes) -> None:
        self.send_response(200)
        self.send_header("Content-Type", kind)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, format: str, *args: object) -> None:
        pass


class _Cdn(http.server.BaseHTTPRequestHandler):
    """The committed grids, as cdn.proj.org serves them: whole or by range,
    with its CORS headers. Records (method, path, Range) of each request."""

    requests: list[tuple[str, str, str | None]] = []

    def _cors(self) -> None:
        self.send_header("Access-Control-Allow-Origin", "*")
        self.send_header("Access-Control-Expose-Headers", "content-length, content-range")

    def do_OPTIONS(self) -> None:
        self.requests.append(("OPTIONS", self.path, self.headers["Access-Control-Request-Headers"]))
        self.send_response(200)
        self._cors()
        self.send_header("Access-Control-Allow-Methods", "GET")
        self.send_header("Access-Control-Allow-Headers", "range")
        self.send_header("Content-Length", "0")
        self.end_headers()

    def do_GET(self) -> None:
        self.requests.append(("GET", self.path, self.headers["Range"]))
        path = GRIDS / self.path.lstrip("/")
        if "/" in self.path.lstrip("/") or not path.is_file():
            self.send_error(404)
            return
        data = path.read_bytes()
        status = 200
        if self.headers["Range"] is not None:
            start, _, end = self.headers["Range"].removeprefix("bytes=").partition("-")
            first, last = int(start), int(end) if end else len(data) - 1
            data = data[first : last + 1]
            status = 206
        self.send_response(status)
        self._cors()
        self.send_header("Content-Type", "image/tiff")
        self.send_header("Content-Length", str(len(data)))
        if status == 206:
            self.send_header("Content-Range", f"bytes {first}-{first + len(data) - 1}/{path.stat().st_size}")
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, format: str, *args: object) -> None:
        pass


def _serve(handler: object) -> http.server.ThreadingHTTPServer:
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)  # type: ignore[arg-type]
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server


def _vectors() -> tuple[list[float], list[float], list[list[float]]]:
    for line in VECTORS.read_text().splitlines()[1:]:
        record = json.loads(line)
        if record["op"] == OPERATION:
            pairs = [(p, w) for p, w in zip(record["in"], record["out"], strict=True) if w is not None]
            return [p[0] for p, _ in pairs], [p[1] for p, _ in pairs], [w for _, w in pairs]
    raise SystemExit(f"no vectors for EPSG:{OPERATION} in {VECTORS}")


def _check(where: str, result: dict[str, list[float]], want: list[list[float]]) -> list[str]:
    errors = []
    got = [*zip(result["xs"], result["ys"], strict=True)]
    for (x, y), (wx, wy) in zip(got, want, strict=True):
        if abs(x - wx) > 1e-9 or abs(y - wy) > 1e-9:
            errors.append(f"{where}: got ({x}, {y}), PROJ gives ({wx}, {wy})")
    sx, sy = result["single"]
    if abs(sx - want[0][0]) > 1e-9 or abs(sy - want[0][1]) > 1e-9:
        errors.append(f"{where}: the single point gives ({sx}, {sy}), PROJ ({want[0][0]}, {want[0][1]})")
    return errors


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--wheel", type=Path, required=True, help="the Pyodide wheel")
    parser.add_argument("--pyodide", type=Path, required=True, help="a Pyodide distribution directory")
    args = parser.parse_args()
    if not (args.pyodide / "pyodide.mjs").is_file():
        parser.error(f"{args.pyodide} has no pyodide.mjs")

    xs, ys, want = _vectors()
    site = _serve(functools.partial(_Site, pyodide=args.pyodide.resolve(), wheel=args.wheel.resolve()))
    cdn = _serve(_Cdn)
    # Another origin than the page's, as cdn.proj.org is.
    cdn_url = f"http://localhost:{cdn.server_address[1]}/"
    code = CODE.replace("__XS__", json.dumps(xs)).replace("__YS__", json.dumps(ys)).replace("__CDN__", json.dumps(cdn_url))

    errors: list[str] = []
    with sync_playwright() as p:
        browser = p.chromium.launch()
        page = browser.new_page()
        messages: list[str] = []
        page.on("console", lambda m: messages.append(m.text))
        page.goto(f"http://127.0.0.1:{site.server_address[1]}/")
        page.wait_for_function("window.ready === true")
        for where, call in [("main thread", "onMainThread"), ("worker", "inWorker")]:
            _Cdn.requests = []
            try:
                result = json.loads(page.evaluate(f"(code) => window.{call}(code)", code))
            except Exception as e:  # noqa: BLE001
                errors.append(f"{where}: {e}")
                continue
            errors += _check(where, result, want)
            ranges = [r for r in _Cdn.requests if r[0] == "GET" and r[2] is not None]
            files = sorted({r[1] for r in ranges})
            print(f"{where}: {len(xs)} points match PROJ; {len(ranges)} range requests for {', '.join(files)}")
            if not ranges:
                errors.append(f"{where}: no grid ranges were fetched")
        browser.close()
    site.shutdown()
    cdn.shutdown()
    if errors:
        print("\n".join(["FAILED:", *errors, "console:", *messages]), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
