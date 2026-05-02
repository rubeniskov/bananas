SUMMARY = "BanaNAS operator console (TUI + CLI)"
DESCRIPTION = "Standalone bananas-config binary for managing the BPI \
without a browser: status table, timezone editor, reboot, and CLI \
subcommands for in-place updates. Talks straight to the bananas-engine \
Unix socket — same trust boundary as the web admin."

LICENSE = "MIT"
LIC_FILES_CHKSUM = "file://${COMMON_LICENSE_DIR}/MIT;md5=0835ade698e0bcf8506ecda2f7b4f302"

# PV sourced from <repo>/assets/version.txt — see comment in
# bananas-webadmin.bb for the rationale.
require recipes-bsp/bananas-version.inc

# No systemd unit — bananas-config is invoked interactively (or scripted)
# rather than running as a daemon.

# Pull the prebuilt binary from serve/bin/ where pixi run build-config-arm
# stages it.
FILESEXTRAPATHS:prepend := "${THISDIR}/files:${COREBASE}/../serve/bin/${TUNE_PKGARCH}:"
SRC_URI = "file://bananas-config"

S = "${WORKDIR}"

COMPATIBLE_MACHINE = "(bananas-bpi|bananas-rpi)"
INHIBIT_PACKAGE_STRIP = "1"
INSANE_SKIP:${PN} += "arch already-stripped"

# bananas user/group already exist via the bananas-webadmin recipe's
# USERADD step. The TUI requires `bananas` group membership for the
# helper socket; root works too.

do_compile[noexec] = "1"
do_configure[noexec] = "1"

do_install() {
    install -d ${D}${bindir}
    install -m 0755 ${WORKDIR}/bananas-config ${D}${bindir}/bananas-config
}

FILES:${PN} += "${bindir}/bananas-config"
