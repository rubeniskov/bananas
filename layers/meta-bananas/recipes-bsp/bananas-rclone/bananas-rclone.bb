SUMMARY = "rclone (vendored prebuilt) for cloud sync"
DESCRIPTION = "Ships the upstream rclone armv7 binary at /usr/bin/rclone. \
Used by bananas-webadmin's /api/cloud/syncs/<idx>/run handler to push, \
pull, or bisync directories against configured cloud accounts. The \
binary is statically linked Go so no system runtime deps are required."

LICENSE = "MIT"
LIC_FILES_CHKSUM = "file://${COMMON_LICENSE_DIR}/MIT;md5=0835ade698e0bcf8506ecda2f7b4f302"

# PV sourced from <repo>/assets/version.txt — see comment in
# bananas-webadmin.bb. The version reflects the BanaNAS release that
# carries this rclone vendored copy, NOT rclone's upstream version
# (which is pinned by `pixi run setup-rclone-arm`). Operators may
# see no-op upgrades on releases that don't bump rclone — accepted
# trade-off for v1; refine if it becomes noisy.
require recipes-bsp/bananas-version.inc

# Pull the prebuilt binary from serve/bin/ (staged by the
# `setup-rclone-arm` pixi task — version pin + sha256 verify is on
# the pixi side; once the bytes land in serve/bin/ the recipe just
# packages them).
FILESEXTRAPATHS:prepend := "${COREBASE}/../serve/bin/${TUNE_PKGARCH}:"
SRC_URI = "file://rclone"

S = "${WORKDIR}"

COMPATIBLE_MACHINE = "(bananas-bpi|bananas-rpi)"
INHIBIT_PACKAGE_STRIP = "1"
INSANE_SKIP:${PN} += "arch already-stripped"

do_compile[noexec] = "1"
do_configure[noexec] = "1"

do_install() {
    install -d ${D}${bindir}
    install -m 0755 ${WORKDIR}/rclone ${D}${bindir}/rclone
}

FILES:${PN} = "${bindir}/rclone"
