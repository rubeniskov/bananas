SUMMARY = "BanaNAS web admin: bananas-server (HTTP UI) + bananas-helper (root)"
DESCRIPTION = "Two prebuilt Rust binaries cross-compiled on the host with \
cargo-zigbuild for armv7-unknown-linux-gnueabihf. The unprivileged HTTP \
daemon listens on :8080 and reaches a small root-owned helper over a Unix \
socket for tightly-scoped operations (NFS exports rewrite, group ops). \
Run `pixi run build-server-arm` before `pixi run build` to stage the \
binaries in serve/bin/ where this recipe picks them up."

LICENSE = "MIT"
LIC_FILES_CHKSUM = "file://${COMMON_LICENSE_DIR}/MIT;md5=0835ade698e0bcf8506ecda2f7b4f302"

inherit systemd useradd

# Pull binaries from serve/bin/ (top-level repo path; COREBASE is poky/, so
# ../serve/bin is correct) and the unit files from the local files/ dir.
FILESEXTRAPATHS:prepend := "${THISDIR}/files:${COREBASE}/../serve/bin:"
SRC_URI = "file://bananas-helper.service \
           file://bananas-server.service \
           file://bananas-server \
           file://bananas-helper"

S = "${WORKDIR}"

# These binaries are armv7-unknown-linux-gnueabihf — only valid for the
# bananapro machine.
COMPATIBLE_MACHINE = "(bananapro)"

# The release binaries are already stripped by zig; QA pass would strip
# again, then complain. Skip the strip + the arch check (Yocto's `file`
# probe reports "ARM, EABI5" while the target is cortexa7t2hf-neon).
INHIBIT_PACKAGE_STRIP = "1"
INSANE_SKIP:${PN} += "arch already-stripped"

SYSTEMD_SERVICE:${PN} = "bananas-helper.service bananas-server.service"
SYSTEMD_AUTO_ENABLE = "enable"

USERADD_PACKAGES = "${PN}"
GROUPADD_PARAM:${PN} = "-r bananas"
USERADD_PARAM:${PN} = "-r -g bananas -d /var/lib/bananas -s /sbin/nologin \
                       -c 'BanaNAS web admin' bananas"

do_compile[noexec] = "1"
do_configure[noexec] = "1"

do_install() {
    install -d ${D}${bindir}
    install -m 0755 ${WORKDIR}/bananas-server ${D}${bindir}/bananas-server
    install -m 0755 ${WORKDIR}/bananas-helper ${D}${bindir}/bananas-helper
    install -d ${D}${systemd_system_unitdir}
    install -m 0644 ${WORKDIR}/bananas-helper.service ${D}${systemd_system_unitdir}/
    install -m 0644 ${WORKDIR}/bananas-server.service ${D}${systemd_system_unitdir}/
}

FILES:${PN} += "${bindir}/bananas-server \
                ${bindir}/bananas-helper \
                ${systemd_system_unitdir}/bananas-helper.service \
                ${systemd_system_unitdir}/bananas-server.service"
