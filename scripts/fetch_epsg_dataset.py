#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["requests>=2.32"]
# ///
"""Download the current EPSG Dataset archives, signing in on the way.

The download page at epsg.org is gated behind an OpenID Connect login, but the
links it hands out are plain public CDN URLs. So the credentials are used for
exactly one thing: reading the link targets off the page. The archives
themselves are fetched unauthenticated.

Credentials come from the environment, or from a `.env` file in the repo root
when running locally:

    EPSG_LOGIN_USER=...
    EPSG_LOGIN_PASSWORD=...

Register for free at <https://epsg.org/>. They are never logged, written to a
file, or sent anywhere but epsg.org's own login endpoint.

    scripts/fetch_epsg_dataset.py                 # into example_data/
    scripts/fetch_epsg_dataset.py --print-links   # resolve only, download nothing
    scripts/fetch_epsg_dataset.py --out DIR --formats PostgreSQL WKT
"""

from __future__ import annotations

import argparse
import html
import os
import re
import sys
from pathlib import Path

import requests

REPO = Path(__file__).resolve().parent.parent
DOWNLOAD_PAGE = "https://epsg.org/download-dataset.html"
# A browser-ish agent: the portal serves the login flow differently otherwise.
USER_AGENT = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120 Safari/537.36"
CDN_PREFIX = "https://drive.tiny.cloud/"


def load_credentials() -> tuple[str, str]:
    user = os.environ.get("EPSG_LOGIN_USER")
    password = os.environ.get("EPSG_LOGIN_PASSWORD")
    env_file = REPO / ".env"
    if (not user or not password) and env_file.exists():
        for line in env_file.read_text().splitlines():
            line = line.strip()
            if not line or line.startswith("#") or "=" not in line:
                continue
            key, value = line.split("=", 1)
            value = value.strip().strip('"').strip("'")
            if key.strip() == "EPSG_LOGIN_USER" and not user:
                user = value
            elif key.strip() == "EPSG_LOGIN_PASSWORD" and not password:
                password = value
    if not user or not password:
        sys.exit(
            "set EPSG_LOGIN_USER and EPSG_LOGIN_PASSWORD (environment or .env).\n"
            "Register for free at https://epsg.org/."
        )
    return user, password


def field(text: str, name: str) -> str | None:
    """Read one hidden input's value. The portal mixes quote styles."""
    for pattern in (
        rf'name="{name}"[^>]*value="([^"]*)"',
        rf'value="([^"]*)"[^>]*name="{name}"',
        rf"name='{name}'[^>]*value='([^']*)'",
        rf"value='([^']*)'[^>]*name='{name}'",
    ):
        if m := re.search(pattern, text):
            return html.unescape(m.group(1))
    return None


def post_form(session: requests.Session, text: str) -> requests.Response | None:
    """Submit an auto-posting form, as the browser's onload handler would.

    OIDC here uses `response_mode=form_post`, so the authorization result comes
    back as a self-submitting HTML form rather than a redirect. Note the markup
    uses single quotes, which is easy to miss.
    """
    action = re.search(r"<form[^>]*action=['\"]([^'\"]+)['\"]", text, re.I)
    if not action:
        return None
    fields = {}
    for m in re.finditer(r"<input\b[^>]*>", text, re.I):
        tag = m.group(0)
        name = re.search(r"name=['\"]([^'\"]+)['\"]", tag)
        value = re.search(r"value=['\"]([^'\"]*)['\"]", tag)
        if name:
            fields[name.group(1)] = html.unescape(value.group(1)) if value else ""
    if not fields:
        return None
    if "error" in fields:
        sys.exit(f"the login was rejected by epsg.org ({fields['error']}); check the credentials")
    return session.post(html.unescape(action.group(1)), data=fields, timeout=60)


def sign_in() -> requests.Session:
    user, password = load_credentials()
    session = requests.Session()
    session.headers["User-Agent"] = USER_AGENT

    response = session.get(DOWNLOAD_PAGE, timeout=60)
    if "/Account/Login" not in response.url:
        return session  # already public, or already signed in

    token = field(response.text, "__RequestVerificationToken")
    return_url = field(response.text, "ReturnUrl") or ""
    if not token:
        sys.exit("could not find the antiforgery token on the login page; the portal changed")

    response = session.post(
        response.url,
        data={
            "Username": user,
            "Password": password,
            "ReturnUrl": return_url,
            # Omitting this gets a bare HTTP 400: ASP.NET checks the form token
            # against the cookie the GET above set.
            "__RequestVerificationToken": token,
            "RememberLogin": "false",
            # The handler branches on this. Without it the post is treated as
            # a cancel and the flow comes back with error=access_denied.
            "button": "login",
        },
        timeout=60,
    )
    # Complete the OIDC form_post hop, which sets the session cookie.
    if (hop := post_form(session, response.text)) is not None:
        response = hop
    if "/Account/Login" in response.url:
        sys.exit("still on the login page after signing in; check the credentials")
    return session


def resolve_links(session: requests.Session) -> tuple[str, dict[str, str]]:
    """Returns the advertised version and {archive name: CDN url}."""
    page = html.unescape(session.get(DOWNLOAD_PAGE, timeout=60).text)
    if "/Account/Login" in page and "Download Version" not in page:
        sys.exit("the download page is still gated; sign-in did not take effect")

    version = (m.group(1) if (m := re.search(r"Download Version:\s*([0-9.]+)", page)) else "unknown")
    links: dict[str, str] = {}
    for pattern in (
        rf'href="({re.escape(CDN_PREFIX)}[^"]+)"[^>]*title="([^"]+)"',
        rf'title="([^"]+)"[^>]*href="({re.escape(CDN_PREFIX)}[^"]+)"',
    ):
        for a, b in re.findall(pattern, page):
            url, title = (a, b) if a.startswith(CDN_PREFIX) else (b, a)
            links.setdefault(title.strip(), url)
    if not links:
        sys.exit("signed in, but found no download links; the portal changed")
    return version, links


def pick(links: dict[str, str], wanted: str) -> tuple[str, str]:
    for title, url in links.items():
        # "EPSG-v13_103-PostgreSQL.zip", "EPSG-v13_103-WKT.Zip"
        if re.search(rf"-{re.escape(wanted)}\.zip$", title, re.I):
            return title, url
    sys.exit(f"no {wanted} archive among {sorted(links)}")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--out", type=Path, default=REPO / "example_data", help="where to write the archives")
    ap.add_argument("--formats", nargs="+", default=["PostgreSQL", "WKT"], help="which archives to fetch")
    ap.add_argument("--print-links", action="store_true", help="resolve the links and exit")
    ap.add_argument("--github", action="store_true", help="append key=value pairs to $GITHUB_OUTPUT")
    args = ap.parse_args()

    session = sign_in()
    version, links = resolve_links(session)
    print(f"signed in; EPSG Dataset v{version}, {len(links)} archives offered")

    chosen = dict(pick(links, fmt) for fmt in args.formats)
    for title, url in chosen.items():
        print(f"  {title}  {url}")

    if args.github and (out := os.environ.get("GITHUB_OUTPUT")):
        with open(out, "a") as fh:
            fh.write(f"version={version}\n")
            for fmt in args.formats:
                title, url = pick(links, fmt)
                fh.write(f"{fmt.lower()}_url={url}\n")
                fh.write(f"{fmt.lower()}_name={title}\n")

    if args.print_links:
        return 0

    args.out.mkdir(parents=True, exist_ok=True)
    # The CDN links need no authentication, so fetch them with a clean session.
    for title, url in chosen.items():
        target = args.out / title
        with requests.get(url, stream=True, timeout=300) as response:
            response.raise_for_status()
            with open(target, "wb") as fh:
                for chunk in response.iter_content(1 << 16):
                    fh.write(chunk)
        print(f"wrote {target} ({target.stat().st_size / 1024:.0f} KiB)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
