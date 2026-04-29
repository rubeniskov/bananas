SUMMARY = "BanaNAS stats sampler daemon"
DESCRIPTION = "Reads /proc + /sys + statvfs once per second and persists \
CPU/mem/network/disk-I/O/partition rows into /var/lib/bananas/stats.db \
(SQLite WAL). Both bananas-server (web admin /api/stats/*) and \
bananas-dashboard (LCD UI) consume this DB read-only."

LICENSE = "MIT"
LIC_FILES_CHKSUM = "file://${COMMON_LICENSE_DIR}/MIT;md5=0835ade698e0bcf8506ecda2f7b4f302"

inherit systemd

# Pull the prebuilt binary from serve/bin/ alongside the unit file.
FILESEXTRAPATHS:prepend := "${THISDIR}/files:${COREBASE}/../serve/bin:"
SRC_URI = "file://bananas-stats \
           file://bananas-stats.service"

S = "${WORKDIR}"

COMPATIBLE_MACHINE = "(bananapro)"
INHIBIT_PACKAGE_STRIP = "1"
INSANE_SKIP:${PN} += "arch already-stripped"

SYSTEMD_SERVICE:${PN} = "bananas-stats.service"
SYSTEMD_AUTO_ENABLE = "enable"

# bananas user/group already exist via the bananas-server recipe's
# USERADD step; we don't redeclare them here.

do_compile[noexec] = "1"
do_configure[noexec] = "1"

do_install() {
    install -d ${D}${bindir}
    install -m 0755 ${WORKDIR}/bananas-stats ${D}${bindir}/bananas-stats
    install -d ${D}${systemd_system_unitdir}
    install -m 0644 ${WORKDIR}/bananas-stats.service ${D}${systemd_system_unitdir}/
}

FILES:${PN} += "${bindir}/bananas-stats \
                ${systemd_system_unitdir}/bananas-stats.service"
RDEPENDS:${PN} += "bananas-server"
