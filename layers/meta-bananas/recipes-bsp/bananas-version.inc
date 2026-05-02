# Single source of truth for the BanaNAS workspace version. Read at
# parse time from `<repo>/assets/version.txt` — the same file
# `scripts/bump-cargo-version.sh` writes when it bumps Cargo.toml. This
# keeps the .ipk version (PV) in lockstep with `cargo --version`, so
# opkg sees a real upgrade on every release instead of looking at six
# packages all stuck at the bitbake default of `1.0`.
#
# Recipes consume it with: `require recipes-bsp/bananas-version.inc`.
# The fallback `0.0.0+dev` only triggers in dev shells where TOPDIR
# isn't the in-tree build directory (or the file is missing) — the
# release pipeline always has it.

def bananas_version(d):
    import os
    p = os.path.join(d.getVar('TOPDIR'), '..', 'assets', 'version.txt')
    try:
        v = open(p).read().strip()
        return v or '0.0.0+dev'
    except Exception:
        return '0.0.0+dev'

PV = "${@bananas_version(d)}"
