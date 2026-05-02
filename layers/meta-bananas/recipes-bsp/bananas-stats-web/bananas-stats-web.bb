SUMMARY = "BanaNAS stats web plugin daemon + SPA"
DESCRIPTION = "Plugin daemon that reads the SQLite metrics DB the \
collector (bananas-stats) writes and answers /api/stats/* + \
/assets/stats/*. Subscribes to the collector's live socket for \
WebSocket push to the SPA. The collector daemon is a separate \
process from the same crate (also named bananas-stats); it stays \
in its own .ipk."

LICENSE = "MIT"
LIC_FILES_CHKSUM = "file://${COMMON_LICENSE_DIR}/MIT;md5=0835ade698e0bcf8506ecda2f7b4f302"

require recipes-bsp/bananas-version.inc

inherit systemd

FILESEXTRAPATHS:prepend := "${THISDIR}/files:${COREBASE}/../serve/bin/${TUNE_PKGARCH}:"
SRC_URI = "file://bananas-stats-web.service \
           file://bananas-stats-web \
           file://stats-web.toml"

S = "${WORKDIR}"

COMPATIBLE_MACHINE = "(bananapro|bananas-rpi)"
INHIBIT_PACKAGE_STRIP = "1"
INSANE_SKIP:${PN} += "arch already-stripped"

SYSTEMD_SERVICE:${PN} = "bananas-stats-web.service"
SYSTEMD_AUTO_ENABLE = "enable"

# bananas-stats provides the collector binary that writes the
# SQLite DB this daemon reads + the live socket it subscribes to.
RDEPENDS:${PN} += "bananas-router bananas-webadmin bananas-stats"

do_compile[noexec] = "1"
do_configure[noexec] = "1"

do_install() {
    install -d ${D}${bindir}
    install -m 0755 ${WORKDIR}/bananas-stats-web ${D}${bindir}/bananas-stats-web

    install -d ${D}${systemd_system_unitdir}
    install -m 0644 ${WORKDIR}/bananas-stats-web.service ${D}${systemd_system_unitdir}/

    install -d ${D}${sysconfdir}/bananas/extensions.d
    install -m 0644 ${WORKDIR}/stats-web.toml ${D}${sysconfdir}/bananas/extensions.d/stats.toml
}

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

FILES:${PN} += "${bindir}/bananas-stats-web \
                ${systemd_system_unitdir}/bananas-stats-web.service \
                ${sysconfdir}/bananas/extensions.d/stats.toml"
