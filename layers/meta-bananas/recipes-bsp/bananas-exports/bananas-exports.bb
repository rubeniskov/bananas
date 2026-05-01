SUMMARY = "BanaNAS NFS exports plugin daemon + SPA"
DESCRIPTION = "Plugin daemon answering /api/exports/* (via \
bananas-router) and /assets/exports/* (via bananas-webadmin's \
sub-proxy). Manages /etc/exports rows, the path-picker browse \
endpoint, and the NFS-server status badge. All file writes go \
through bananas-engine for privilege separation. Cross-built by \
`pixi run build-webadmin-arm` into serve/bin/bananas-exports."

LICENSE = "MIT"
LIC_FILES_CHKSUM = "file://${COMMON_LICENSE_DIR}/MIT;md5=0835ade698e0bcf8506ecda2f7b4f302"

require recipes-bsp/bananas-version.inc

inherit systemd

FILESEXTRAPATHS:prepend := "${THISDIR}/files:${COREBASE}/../serve/bin:"
SRC_URI = "file://bananas-exports.service \
           file://bananas-exports \
           file://exports.toml"

S = "${WORKDIR}"

COMPATIBLE_MACHINE = "(bananapro)"
INHIBIT_PACKAGE_STRIP = "1"
INSANE_SKIP:${PN} += "arch already-stripped"

SYSTEMD_SERVICE:${PN} = "bananas-exports.service"
SYSTEMD_AUTO_ENABLE = "enable"

# Hard runtime deps:
#   bananas-router   — Unix-socket API gateway; reads the manifest
#                      this package drops at install time to route
#                      /api/exports/* here.
#   bananas-webadmin — owns the public TCP port :8080 and sub-proxies
#                      /assets/exports/* to this daemon's socket.
#                      Also issues the session cookies this daemon
#                      validates.
#   bananas-engine   — privileged ops: writes /etc/exports + creates
#                      mount-target directories on demand.
RDEPENDS:${PN} += "bananas-router bananas-webadmin"

do_compile[noexec] = "1"
do_configure[noexec] = "1"

do_install() {
    install -d ${D}${bindir}
    install -m 0755 ${WORKDIR}/bananas-exports ${D}${bindir}/bananas-exports

    install -d ${D}${systemd_system_unitdir}
    install -m 0644 ${WORKDIR}/bananas-exports.service ${D}${systemd_system_unitdir}/

    # Extension manifest — both bananas-router and bananas-webadmin
    # re-read /etc/bananas/extensions.d/ on restart.
    install -d ${D}${sysconfdir}/bananas/extensions.d
    install -m 0644 ${WORKDIR}/exports.toml ${D}${sysconfdir}/bananas/extensions.d/exports.toml
}

# Restart both manifest-reading daemons after install/remove so
# their tables pick up / drop this plugin.
pkg_postinst:${PN}() {
    if [ -z "$D" ]; then
        systemctl restart bananas-router.service 2>/dev/null || true
        systemctl restart bananas-webadmin.service 2>/dev/null || true
    fi
}

pkg_postrm:${PN}() {
    if [ -z "$D" ]; then
        systemctl restart bananas-router.service 2>/dev/null || true
        systemctl restart bananas-webadmin.service 2>/dev/null || true
    fi
}

FILES:${PN} += "${bindir}/bananas-exports \
                ${systemd_system_unitdir}/bananas-exports.service \
                ${sysconfdir}/bananas/extensions.d/exports.toml"
