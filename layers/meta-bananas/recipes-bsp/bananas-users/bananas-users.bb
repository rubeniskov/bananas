SUMMARY = "BanaNAS users plugin daemon + SPA"
DESCRIPTION = "Plugin daemon answering /api/users/* (via \
bananas-router) and /assets/users/* (via bananas-webadmin's \
sub-proxy). Owns POSIX user management — useradd, userdel, \
chpasswd, group toggling — all delegated to bananas-engine \
which has root."

LICENSE = "MIT"
LIC_FILES_CHKSUM = "file://${COMMON_LICENSE_DIR}/MIT;md5=0835ade698e0bcf8506ecda2f7b4f302"

require recipes-bsp/bananas-version.inc

inherit systemd

FILESEXTRAPATHS:prepend := "${THISDIR}/files:${COREBASE}/../serve/bin:"
SRC_URI = "file://bananas-users.service \
           file://bananas-users \
           file://users.toml"

S = "${WORKDIR}"

COMPATIBLE_MACHINE = "(bananapro)"
INHIBIT_PACKAGE_STRIP = "1"
INSANE_SKIP:${PN} += "arch already-stripped"

SYSTEMD_SERVICE:${PN} = "bananas-users.service"
SYSTEMD_AUTO_ENABLE = "enable"

RDEPENDS:${PN} += "bananas-router bananas-webadmin"

do_compile[noexec] = "1"
do_configure[noexec] = "1"

do_install() {
    install -d ${D}${bindir}
    install -m 0755 ${WORKDIR}/bananas-users ${D}${bindir}/bananas-users

    install -d ${D}${systemd_system_unitdir}
    install -m 0644 ${WORKDIR}/bananas-users.service ${D}${systemd_system_unitdir}/

    install -d ${D}${sysconfdir}/bananas/extensions.d
    install -m 0644 ${WORKDIR}/users.toml ${D}${sysconfdir}/bananas/extensions.d/users.toml
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

FILES:${PN} += "${bindir}/bananas-users \
                ${systemd_system_unitdir}/bananas-users.service \
                ${sysconfdir}/bananas/extensions.d/users.toml"
