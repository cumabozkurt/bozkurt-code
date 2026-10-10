#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Generate THIRD_PARTY_LICENSES.md for the shipped binaries and the bundled web UI.

Rust: reads `cargo metadata --format-version 1 --locked` (all platforms, so the
Windows/macOS/Linux-only crates are included) and collects every non-workspace
package with its SPDX expression and its license/notice files.
npm: reads package-lock.json and includes every non-dev package installed under
node_modules (the UI bundled into the desktop and mobile apps).

Usage:
  cargo metadata --format-version 1 --locked > metadata.json
  python3 scripts/third-party-licenses.py --metadata metadata.json > THIRD_PARTY_LICENSES.md

Exit status 1 when a package declares no licence at all, or when a licence
expression contains an identifier outside ALLOWED (review it, then extend the list).
"""
import argparse
import json
import os
import re
import sys

ALLOWED = {
    "MIT", "MIT-0", "Apache-2.0", "Apache-2.0 WITH LLVM-exception", "BSD-2-Clause", "BSD-3-Clause",
    "ISC", "Zlib", "0BSD", "BSL-1.0", "CC0-1.0", "Unlicense", "Unicode-3.0", "Unicode-DFS-2016",
    "MPL-2.0", "CDLA-Permissive-2.0", "OpenSSL", "BlueOak-1.0.0", "Python-2.0", "CC-BY-4.0", "WTFPL",
    "NCSA", "bzip2-1.0.6", "LGPL-2.1-or-later",
}
LICENSE_FILE = re.compile(r"^(licen[cs]e|copying|notice|copyright)([-._].*)?$", re.IGNORECASE)


def license_ids(expr):
    expr = expr.replace("/", " OR ")  # old crates use "MIT/Apache-2.0"
    expr = re.sub(r"[()]", " ", expr)
    ids, cur = [], []
    for tok in expr.split():
        if tok in ("OR", "AND"):
            if cur:
                ids.append(" ".join(cur))
            cur = []
        else:
            cur.append(tok)
    if cur:
        ids.append(" ".join(cur))
    return [i.rstrip("+") for i in ids]


def license_texts(directory, explicit=None):
    texts, seen = [], set()
    candidates = []
    if explicit:
        candidates.append(os.path.join(directory, explicit))
    try:
        candidates += [os.path.join(directory, f) for f in sorted(os.listdir(directory)) if LICENSE_FILE.match(f)]
    except OSError:
        pass
    for path in candidates:
        if path in seen or not os.path.isfile(path):
            continue
        seen.add(path)
        try:
            with open(path, encoding="utf-8", errors="replace") as fh:
                texts.append((os.path.basename(path), fh.read().strip()))
        except OSError:
            pass
    return texts


def rust_packages(metadata_path):
    with open(metadata_path, encoding="utf-8") as fh:
        meta = json.load(fh)
    members = set(meta.get("workspace_members", []))
    out = []
    for pkg in meta["packages"]:
        if pkg["id"] in members or pkg.get("source") is None:
            continue
        directory = os.path.dirname(pkg["manifest_path"])
        out.append({
            "name": pkg["name"], "version": pkg["version"], "license": pkg.get("license") or "",
            "url": pkg.get("repository") or "", "texts": license_texts(directory, pkg.get("license_file")),
        })
    return out


def npm_packages(root):
    lock_path = os.path.join(root, "package-lock.json")
    if not os.path.isfile(lock_path):
        return []
    with open(lock_path, encoding="utf-8") as fh:
        lock = json.load(fh)
    out, seen = [], set()
    for path, info in lock.get("packages", {}).items():
        if not path or "node_modules/" not in path or info.get("dev") or info.get("link"):
            continue
        name = path.rsplit("node_modules/", 1)[1]
        if name.startswith("@bozkurt/") or (name, info.get("version")) in seen:
            continue
        seen.add((name, info.get("version")))
        directory = os.path.join(root, path)
        lic = info.get("license") or ""
        url = ""
        pj = os.path.join(directory, "package.json")
        if os.path.isfile(pj):
            with open(pj, encoding="utf-8") as fh:
                data = json.load(fh)
            lic = lic or (data.get("license") if isinstance(data.get("license"), str) else "")
            repo = data.get("repository")
            url = repo.get("url", "") if isinstance(repo, dict) else (repo or "")
        out.append({"name": name, "version": info.get("version", ""), "license": lic, "url": url,
                    "texts": license_texts(directory)})
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--metadata", help="output of `cargo metadata --format-version 1 --locked`")
    ap.add_argument("--root", default=os.getcwd(), help="repository root (for package-lock.json)")
    args = ap.parse_args()

    sections = []
    if args.metadata:
        sections.append(("Rust crates (bozkurtd, bz, bozkurt-mcp, desktop and mobile shells)", rust_packages(args.metadata)))
    sections.append(("npm packages (web UI bundled into the desktop and mobile apps)", npm_packages(args.root)))

    problems = []
    texts = {}  # normalised text -> (index, file name, text, [users])
    w = sys.stdout.write
    w("# Third-party licences\n\n")
    w("BOZKURT CODE is licensed under MIT OR Apache-2.0. The binaries and apps include the third-party\n")
    w("software listed below; each component remains under its own licence, reproduced in full at the end\n")
    w("of this file (identical texts are printed once). Generated by `scripts/third-party-licenses.py`.\n\n")
    for title, pkgs in sections:
        pkgs.sort(key=lambda p: (p["name"].lower(), p["version"]))
        w(f"## {title}\n\n| Package | Version | Licence | Texts |\n|---|---|---|---|\n")
        for p in pkgs:
            refs = []
            for fname, text in p["texts"]:
                key = re.sub(r"\s+", " ", text)
                if key not in texts:
                    texts[key] = (len(texts) + 1, fname, text, [])
                texts[key][3].append(f"{p['name']} {p['version']}")
                refs.append(f"[{texts[key][0]}](#text-{texts[key][0]})")
            w(f"| {p['name']} | {p['version']} | {p['license'] or 'UNKNOWN'} | {', '.join(refs) or '-'} |\n")
            if not p["license"] and not p["texts"]:
                problems.append(f"{p['name']} {p['version']}: no licence declared")
            for lid in license_ids(p["license"]):
                if lid not in ALLOWED:
                    problems.append(f"{p['name']} {p['version']}: licence '{lid}' not in the allow-list")
        w("\n")
    w("## Licence texts\n\n")
    for idx, fname, text, users in sorted(texts.values()):
        w(f"### Text {idx}\n\n<a id=\"text-{idx}\"></a>`{fname}` from {', '.join(users)}\n\n```text\n{text}\n```\n\n")
    if problems:
        for line in problems:
            print(f"::error::{line}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
