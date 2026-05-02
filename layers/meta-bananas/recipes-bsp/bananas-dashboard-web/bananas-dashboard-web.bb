SUMMARY = "BanaNAS dashboard config web plugin daemon + SPA"
DESCRIPTION = "Plugin daemon answering /api/dashboard/config (a \
small TOML editor for /etc/bananas/dashboard.toml — the SLINT \
LCD app's appearance/refresh settings) and /assets/dashboard/* \
(the wasm SPA). The SLINT LCD app itself ships in the separate \
bananas-dashboard.ipk."

LICENSE = "MIT"
LIC_FILES_CHKSUM = "file://${COMMON_LICENSE_DIR}/MIT;md5=0835ade698e0bcf8506ecda2f7b4f302"

require recipes-bsp/bananas-version.inc

inherit systemd

FILESEXTRAPATHS:prepend := "${THISDIR}/files:${COREBASE}/../serve/bin/${TUNE_PKGARCH}:"
SRC_URI = "file://bananas-dashboard-web.service \
           file://bananas-dashboard-web \
           file://dashboard-web.toml"

S = "${WORKDIR}"

COMPATIBLE_MACHINE = "(bananas-bpi|bananas-rpi)"
INHIBIT_PACKAGE_STRIP = "1"
INSANE_SKIP:${PN} += "arch already-stripped"

SYSTEMD_SERVICE:${PN} = "bananas-dashboard-web.service"
SYSTEMD_AUTO_ENABLE = "enable"

RDEPENDS:${PN} += "bananas-router bananas-webadmin"

do_compile[noexec] = "1"
do_configure[noexec] = "1"

do_install() {
    install -d ${D}${bindir}
    install -m 0755 ${WORKDIR}/bananas-dashboard-web ${D}${bindir}/bananas-dashboard-web

    install -d ${D}${systemd_system_unitdir}
    install -m 0644 ${WORKDIR}/bananas-dashboard-web.service ${D}${systemd_system_unitdir}/

    install -d ${D}${sysconfdir}/bananas/extensions.d
    install -m 0644 ${WORKDIR}/dashboard-web.toml ${D}${sysconfdir}/bananas/extensions.d/dashboard.toml
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

FILES:${PN} += "${bindir}/bananas-dashboard-web \
                ${systemd_system_unitdir}/bananas-dashboard-web.service \
                ${sysconfdir}/bananas/extensions.d/dashboard.toml"
