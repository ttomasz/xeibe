#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Build a PROJ database (`proj.db`) from the same EPSG release as `xeibe-crs`.

The reprojection tests use PROJ, through pyproj, as their oracle. pyproj ships
a `proj.db` built from whatever EPSG release PROJ last imported (v12.029 for
PROJ 9.8.1), while `xeibe-crs` follows the current one. Operations and even
datums differ between the two (PL-2000 moved from ETRS89 to ETRF2000-PL), so
comparing against pyproj's own database would test the version gap, not our
code. Instead this rebuilds `proj.db` the way PROJ does, from our EPSG
archive, with PROJ's own import script:

1. download the PROJ source release the oracle pyproj wraps (checksum pinned);
2. run its `scripts/build_db.py` on the EPSG PostgreSQL archive, which writes
   PROJ's `data/sql/*.sql` from EPSG's tables;
3. assemble `proj.db` from those files in the order `data/sql_filelist.cmake`
   gives, with PROJ's consistency triggers and final checks on.

Two adjustments, both reported when they apply:

* `build_db.py` asserts on grid transformations it doesn't know how to import
  (EPSG 13.106 added PatchJGD vertical grids with two files). Those are
  skipped: they are grid-based, beyond what the oracle is used for.
* Statements that PROJ's consistency checks reject (its hand-written
  additions sometimes contradict a newer EPSG release) are skipped and listed.

The EPSG version in the database metadata is set to the archive's.

    scripts/build_proj_oracle.py                 # newest EPSG-*-PostgreSQL.zip in example_data/
    scripts/build_proj_oracle.py --sql PATH

Output: `target/proj-oracle/proj.db`, PROJ's database as PROJ builds it,
and `target/proj-oracle-epsg/proj.db`, the same with only EPSG's operations
plus the null operations `build_db.py` adds between a datum ensemble and
each of its members ("WGS 84 to WGS 84 (G2296)", "ETRS89 to ETRF2014"),
which `xeibe-reproject` adds too (`docs/reprojection.md`, level 2). The
reprojection vectors use the second, with no authority restriction; pyproj
compatibility tests the first. Use either from pyproj with
`pyproj.datadir.set_data_dir(...)` (pyproj ignores `PROJ_DATA` in favour of
its bundled data).
"""

from __future__ import annotations

import argparse
import hashlib
import re
import shutil
import sqlite3
import subprocess
import sys
import tarfile
import urllib.request
import zipfile
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
OUT = REPO / "target" / "proj-oracle"
OUT_EPSG = REPO / "target" / "proj-oracle-epsg"

# The PROJ release pyproj 3.8.0 wraps; keep the two in step.
PROJ_VERSION = "9.8.1"
PROJ_URL = f"https://download.osgeo.org/proj/proj-{PROJ_VERSION}.tar.gz"
PROJ_SHA256 = "af5b731c145c1d13c4e3b4eeb7d167e94e845e440f71e3496b4ed8dae0291960"

# build_db.py asserts that a grid transformation it doesn't special-case has
# exactly one parameter; skip the others instead.
BUILD_DB_PATCH = (
    """        else:
            assert n_params == 1, (code, name, method_code, n_params)
""",
    """        elif n_params != 1:
            print('Skipping grid transformation %d (%s): %d parameters' % (code, name, n_params))
            continue
""",
)


def find_sql_zip() -> Path:
    candidates = sorted((REPO / "example_data").glob("EPSG-*PostgreSQL.zip"))
    if not candidates:
        sys.exit("no EPSG PostgreSQL archive in example_data/; run scripts/fetch_epsg_dataset.py")
    return candidates[-1]


def fetch_proj_source(dest: Path) -> Path:
    tarball = dest / f"proj-{PROJ_VERSION}.tar.gz"
    if not tarball.exists():
        print(f"downloading {PROJ_URL}")
        with urllib.request.urlopen(PROJ_URL) as response:
            tarball.write_bytes(response.read())
    digest = hashlib.sha256(tarball.read_bytes()).hexdigest()
    if digest != PROJ_SHA256:
        tarball.unlink()
        sys.exit(f"{tarball.name}: sha256 {digest}, expected {PROJ_SHA256}")
    src = dest / f"proj-{PROJ_VERSION}"
    shutil.rmtree(src, ignore_errors=True)
    wanted = ("scripts/build_db.py", "data/sql/", "data/sql_filelist.cmake")
    with tarfile.open(tarball) as tar:
        members = [
            m for m in tar.getmembers() if any(m.name.startswith(f"proj-{PROJ_VERSION}/{w}") for w in wanted)
        ]
        tar.extractall(dest, members=members, filter="data")
    return src


def run_build_db(src: Path, sql_zip: Path) -> None:
    script = src / "scripts" / "build_db.py"
    text = script.read_text()
    before, after = BUILD_DB_PATCH
    if before not in text:
        sys.exit("build_db.py has changed; review BUILD_DB_PATCH")
    script.write_text(text.replace(before, after))
    with zipfile.ZipFile(sql_zip) as z:
        for name in z.namelist():
            base = Path(name).name
            if base in ("PostgreSQL_Table_Script.sql", "PostgreSQL_Data_Script.sql"):
                (script.parent / base).write_bytes(z.read(name))
    print("running PROJ's build_db.py")
    result = subprocess.run(
        [sys.executable, "-I", script.name], cwd=script.parent, capture_output=True, text=True
    )
    for line in result.stdout.splitlines():
        if line.startswith("Skipping grid transformation"):
            print(f"  {line}")
    if result.returncode != 0:
        sys.stderr.write(result.stdout[-4000:] + result.stderr[-4000:])
        sys.exit("build_db.py failed")


def sql_file_order(src: Path) -> list[str]:
    """The SQL files in the order PROJ's CMake build uses with extra validation."""
    text = (src / "data" / "sql_filelist.cmake").read_text()

    def files(block: str) -> list[str]:
        return re.findall(r"\$\{SQL_DIR\}/([\w.]+\.sql)", block)

    start = text.index("set(SQL_FILES\n")
    triggers = files(text[:start])
    first = files(text[start : text.index("if (PROJ_DB_EXTRA_VALIDATION)", start)])
    main_start = text.index('list(APPEND SQL_FILES\n  "${SQL_DIR}/customizations_early.sql"')
    main = files(text[main_start : text.index(")", main_start)])
    return first + triggers + main + ["commit.sql", "final_consistency_checks.sql", "analyze_vacuum.sql"]


def epsg_version(sql_zip: Path) -> tuple[str, str]:
    """Version and date of the archive, from its version history table."""
    m = re.search(r"EPSG-v(\d+)_(\d+)", sql_zip.name)
    version = f"v{m.group(1)}.{m.group(2)}" if m else "unknown"
    date = "unknown"
    with zipfile.ZipFile(sql_zip) as z:
        name = next(n for n in z.namelist() if n.endswith("PostgreSQL_Data_Script.sql"))
        data = z.read(name).decode("utf-8-sig")
    for row in re.finditer(r"INSERT INTO epsg_versionhistory [^;]*?VALUES \(\d+, '([\d-]+)', '([\d.]+)'", data):
        if f"v{row.group(2)}" == version:
            date = row.group(1)
    return version, date


def assemble(src: Path, db_path: Path, version: str, date: str) -> list[str]:
    db_path.unlink(missing_ok=True)
    db = sqlite3.connect(db_path, isolation_level=None)
    rejected: dict[tuple[str, str], int] = {}
    for name in sql_file_order(src):
        text = (src / "data" / "sql" / name).read_text().replace("${PROJ_VERSION}", PROJ_VERSION)
        if name == "metadata.sql":
            text = re.sub(r"('EPSG\.VERSION', )'[^']*'", rf"\1'{version}'", text)
            text = re.sub(r"('EPSG\.DATE', )'[^']*'", rf"\1'{date}'", text)
        statement = ""
        for line in text.splitlines(keepends=True):
            statement += line
            if sqlite3.complete_statement(statement):
                try:
                    db.execute(statement)
                except sqlite3.Error as e:
                    key = (name, str(e)[:120])
                    rejected[key] = rejected.get(key, 0) + 1
                statement = ""
    db.close()
    return [f"{n} x {file}: {message}" for (file, message), n in rejected.items()]


def epsg_only(full: Path, out: Path) -> dict[str, int]:
    """A copy of `full` without other authorities' operations, except PROJ's
    null operations between a datum ensemble and its members. Returns how
    many of those it keeps, by table."""
    out.unlink(missing_ok=True)
    shutil.copyfile(full, out)
    db = sqlite3.connect(out, isolation_level=None)
    ensemble = "auth_name = 'PROJ' AND description LIKE '%from datum ensemble definition'"
    db.execute(f"DELETE FROM helmert_transformation_table WHERE auth_name != 'EPSG' AND NOT ({ensemble})")
    db.execute(f"DELETE FROM other_transformation WHERE auth_name != 'EPSG' AND NOT ({ensemble})")
    db.execute("DELETE FROM grid_transformation WHERE auth_name != 'EPSG'")
    db.execute("DELETE FROM concatenated_operation_step WHERE operation_auth_name != 'EPSG'")
    db.execute("DELETE FROM concatenated_operation WHERE auth_name != 'EPSG'")
    kept = {
        table: n
        for table, n in db.execute(
            "SELECT table_name, count(*) FROM coordinate_operation_view WHERE auth_name != 'EPSG' GROUP BY 1"
        )
    }
    db.execute("VACUUM")
    db.close()
    return kept


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--sql", type=Path, help="EPSG PostgreSQL scripts zip")
    args = parser.parse_args()
    sql_zip = args.sql or find_sql_zip()
    version, date = epsg_version(sql_zip)
    print(f"EPSG {version} ({date}) from {sql_zip.name}, PROJ {PROJ_VERSION}")

    work = OUT / "build"
    work.mkdir(parents=True, exist_ok=True)
    src = fetch_proj_source(work)
    run_build_db(src, sql_zip)
    print("assembling proj.db (about a minute)")
    rejected = assemble(src, OUT / "proj.db", version, date)
    for line in rejected:
        print(f"  rejected by PROJ's checks: {line}")
    print(f"wrote {(OUT / 'proj.db').relative_to(REPO)} ({(OUT / 'proj.db').stat().st_size // 1024} KiB)")
    OUT_EPSG.mkdir(parents=True, exist_ok=True)
    kept = epsg_only(OUT / "proj.db", OUT_EPSG / "proj.db")
    summary = ", ".join(f"{n} in {table}" for table, n in sorted(kept.items()))
    print(f"wrote {(OUT_EPSG / 'proj.db').relative_to(REPO)}: EPSG plus PROJ's ensemble operations ({summary})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
