#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Check relative links and #anchors in the repository's Markdown files (no network).

Usage: python3 scripts/check-md-links.py [ROOT]   -> exit 1 when a link or anchor is broken.
External http(s)/mailto links are listed with --external but not fetched.
"""
import glob
import os
import re
import sys

SKIP = ("node_modules", "/target", "/dist/", "/gen/")


def slug(heading):
    h = re.sub(r"<[^>]+>", "", heading.strip().lower()).replace("`", "").replace("*", "")
    return "".join(c for c in h if c.isalnum() or c in " -_").replace(" ", "-")


def scan(path):
    """Yield (line_no, line) outside fenced code blocks."""
    fence = None
    with open(path, encoding="utf-8") as fh:
        for no, line in enumerate(fh, 1):
            m = re.match(r"^\s*(```|~~~)", line)
            if m:
                fence = None if fence == m.group(1) else (fence or m.group(1))
                continue
            if not fence:
                yield no, line


def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    root = os.path.abspath(args[0] if args else os.getcwd())
    files = [f for f in glob.glob(os.path.join(root, "**", "*.md"), recursive=True) if not any(s in f for s in SKIP)]
    anchors = {}
    for f in files:
        found, counts = set(), {}
        for _, line in scan(f):
            m = re.match(r"^(#{1,6})\s+(.*?)\s*#*\s*$", line)
            if m:
                a = slug(m.group(2))
                n = counts.get(a, 0)
                counts[a] = n + 1
                found.add(a if n == 0 else f"{a}-{n}")
            for m in re.finditer(r'<a\s+(?:name|id)="([^"]+)"', line):
                found.add(m.group(1))
        anchors[os.path.abspath(f)] = found
    bad, external = [], set()
    for f in files:
        for no, line in scan(f):
            line = re.sub(r"`[^`]*`", "", line)
            for m in re.finditer(r'\]\(<?([^)\s>]+)>?(?:\s+"[^"]*")?\)|(?:href|src)="([^"]+)"', line):
                url = m.group(1) or m.group(2)
                if url.startswith(("http://", "https://", "mailto:")):
                    external.add(url)
                    continue
                path, _, frag = url.partition("#")
                target = os.path.abspath(os.path.join(os.path.dirname(f), path)) if path else os.path.abspath(f)
                rel = os.path.relpath(f, root)
                if path and not os.path.exists(target):
                    bad.append(f"{rel}:{no}: missing file {url}")
                elif frag and target.endswith(".md") and frag not in anchors.get(target, set()):
                    bad.append(f"{rel}:{no}: missing anchor {url}")
    if "--external" in sys.argv:
        print("\n".join(sorted(external)))
    print("\n".join(bad) or f"no broken internal links ({len(files)} files)")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
