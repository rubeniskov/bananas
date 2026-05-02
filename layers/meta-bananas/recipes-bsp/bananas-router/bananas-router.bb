SUMMARY = "BanaNAS reverse-proxy gateway (path-based to per-package daemons)"
DESCRIPTION = "Tiny axum daemon that listens on :8080 and forwards each \
request, by longest-prefix match against /etc/bananas/extensions.d/*.toml \
manifests, to the matching extension daemon over a Unix socket. Has no \
business logic, no auth, no SPA serving — extensions own all of that. \
The plugin model lets bananas-cloud (and any future feature package) \
land as a self-contained daemon + manifest without router-side changes. \
Cross-built by `pixi run build-router-arm` into serve/bin/bananas-router."

LICENSE = "MIT"
LIC_FILES_CHKSUM = "file://${COMMON_LICENSE_DIR}/MIT;md5=0835ade698e0bcf8506ecda2f7b4f302"

require recipes-bsp/bananas-version.inc

inherit systemd

FILESEXTRAPATHS:prepend := "${THISDIR}/files:${COREBASE}/../serve/bin/${TUNE_PKGARCH}:"
SRC_URI = "file://bananas-router.service \
           file://bananas-router"

S = "${WORKDIR}"

COMPATIBLE_MACHINE = "(bananapro|bananas-rpi)"
INHIBIT_PACKAGE_STRIP = "1"
INSANE_SKIP:${PN} += "arch already-stripped"

SYSTEMD_SERVICE:${PN} = "bananas-router.service"
SYSTEMD_AUTO_ENABLE = "enable"

# The router runs as the bananas user, same as bananas-webadmin and
# bananas-cloud — they share /run/bananas/ via systemd's per-unit
# RuntimeDirectory=bananas. The user is created by bananas-webadmin
# already; we RDEPENDS it so the user account exists by the time
# bananas-router starts.
RDEPENDS:${PN} += "bananas-webadmin"

do_compile[noexec] = "1"
do_configure[noexec] = "1"

do_install() {
    install -d ${D}${bindir}
    install -m 0755 ${WORKDIR}/bananas-router ${D}${bindir}/bananas-router

    install -d ${D}${systemd_system_unitdir}
    install -m 0644 ${WORKDIR}/bananas-router.service ${D}${systemd_system_unitdir}/

    # Create the manifest dir so bananas-router can read it even before
    # any extension is installed. Extensions drop their manifests as
    # /etc/bananas/extensions.d/<id>.toml via their own postinst.
    install -d ${D}${sysconfdir}/bananas/extensions.d
}

FILES:${PN} += "${bindir}/bananas-router \
                ${systemd_system_unitdir}/bananas-router.service \
                ${sysconfdir}/bananas/extensions.d"
