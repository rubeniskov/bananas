SUMMARY = "BanaNAS storage plugin daemon + SPA"
DESCRIPTION = "Plugin daemon answering /api/storage/* (via \
bananas-router) and /assets/storage/* (via bananas-webadmin's \
sub-proxy). Owns the disk dashboard (lsblk + smartctl), the \
/etc/fstab editor, and the chmod/chown helper. Cross-built by \
`pixi run build-webadmin-arm` into serve/bin/bananas-storage."

LICENSE = "MIT"
LIC_FILES_CHKSUM = "file://${COMMON_LICENSE_DIR}/MIT;md5=0835ade698e0bcf8506ecda2f7b4f302"

require recipes-bsp/bananas-version.inc

inherit systemd

FILESEXTRAPATHS:prepend := "${THISDIR}/files:${COREBASE}/../serve/bin/${TUNE_PKGARCH}:"
SRC_URI = "file://bananas-storage.service \
           file://bananas-storage \
           file://storage.toml"

S = "${WORKDIR}"

COMPATIBLE_MACHINE = "(bananas-bpi|bananas-rpi)"
INHIBIT_PACKAGE_STRIP = "1"
INSANE_SKIP:${PN} += "arch already-stripped"

SYSTEMD_SERVICE:${PN} = "bananas-storage.service"
SYSTEMD_AUTO_ENABLE = "enable"

RDEPENDS:${PN} += "bananas-router bananas-webadmin"

do_compile[noexec] = "1"
do_configure[noexec] = "1"

do_install() {
    install -d ${D}${bindir}
    install -m 0755 ${WORKDIR}/bananas-storage ${D}${bindir}/bananas-storage

    install -d ${D}${systemd_system_unitdir}
    install -m 0644 ${WORKDIR}/bananas-storage.service ${D}${systemd_system_unitdir}/

    install -d ${D}${sysconfdir}/bananas/extensions.d
    install -m 0644 ${WORKDIR}/storage.toml ${D}${sysconfdir}/bananas/extensions.d/storage.toml
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

FILES:${PN} += "${bindir}/bananas-storage \
                ${systemd_system_unitdir}/bananas-storage.service \
                ${sysconfdir}/bananas/extensions.d/storage.toml"
