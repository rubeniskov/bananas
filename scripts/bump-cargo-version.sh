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
# `cargo update -w` updates only workspace member entries from the
# manifests we just edited; it leaves external crates.io packages
# pinned at their existing Cargo.lock versions. We don't pass --offline
# even though the workspace-only update should be a local operation —
# cargo's resolver still wants the registry index loaded to verify
# version constraints on the dep graph, and the CI runner's cache
# may not have every transitive crate (especially for a new dep added
# in this commit). Hitting crates.io once per release is cheap.
cargo update -w

# Single source of truth for Yocto recipe PV. Read at parse time by
# layers/meta-bananas/recipes-bsp/bananas-version.inc. Writing this in
# the same script keeps Cargo + .ipk versions in lockstep.
echo "$ver" > assets/version.txt

echo "Bumped Cargo.toml + Cargo.lock + assets/version.txt to $ver"
