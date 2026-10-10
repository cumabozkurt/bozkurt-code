#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# Recompute SHA256SUMS over every asset of a (draft) GitHub release and upload it.
# Usage: scripts/release-checksums.sh <tag>      (needs GH_TOKEN and GH_REPO or a git checkout)
# Called by release.yml (after the desktop builds) and mobile.yml (after APK/AAB/IPA upload).
set -euo pipefail
tag="${1:?usage: release-checksums.sh <tag>}"
dir="$(mktemp -d)"
trap 'rm -rf "$dir"' EXIT
gh release download "$tag" --dir "$dir" --clobber
rm -f "$dir/SHA256SUMS"
(
  cd "$dir"
  sha256sum -- * | LC_ALL=C sort -k2 > SHA256SUMS
  cat SHA256SUMS
)
gh release upload "$tag" "$dir/SHA256SUMS" --clobber
