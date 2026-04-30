#!/usr/bin/env bash
# Update [workspace.package].version in Cargo.toml + refresh Cargo.lock to
# match. Invoked by @semantic-release/exec on every release so the binaries
# bake the right version via env!("CARGO_PKG_VERSION").
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

echo "Bumped Cargo.toml + Cargo.lock to $ver"
