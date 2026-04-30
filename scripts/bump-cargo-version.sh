#!/usr/bin/env bash
# Update [workspace.package].version in Cargo.toml + refresh Cargo.lock to
# match, and also write the version into assets/version.txt so the Yocto
# recipes' `require recipes-bsp/bananas-version.inc` picks it up. Invoked
# by .github/workflows/release.yml on every release so the binaries bake
# the right version via env!("CARGO_PKG_VERSION") AND the published .ipk
# files carry the matching PV (otherwise opkg sees no upgrade).
#
# Usage: bump-cargo-version.sh <version>
#   e.g. bump-cargo-version.sh 1.2.0

set -euo pipefail

ver="${1:?usage: $0 <version>}"

# Targeted in-place replacement: only the version key under
# [workspace.package], not any other version = "..." line that might appear
# elsewhere (e.g. inline workspace deps).
awk -v ver="$ver" '
  /^\[workspace\.package\]/ { in_section = 1 }
  /^\[/ && !/^\[workspace\.package\]/ { in_section = 0 }
  in_section && /^version[[:space:]]*=/ { print "version = \"" ver "\""; next }
  { print }
' Cargo.toml > Cargo.toml.tmp
mv Cargo.toml.tmp Cargo.toml

# Refresh Cargo.lock so the workspace member entries get the new version.
# `cargo update -w --offline` updates only workspace member entries from the
# manifests we just edited; it does not touch external crates.io packages
# and does not need network.
cargo update -w --offline

# Single source of truth for Yocto recipe PV. Read at parse time by
# layers/meta-bananas/recipes-bsp/bananas-version.inc. Writing this in
# the same script keeps Cargo + .ipk versions in lockstep.
echo "$ver" > assets/version.txt

echo "Bumped Cargo.toml + Cargo.lock + assets/version.txt to $ver"
