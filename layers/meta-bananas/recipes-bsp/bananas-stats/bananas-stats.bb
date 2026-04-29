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
           file://bananas-stats.service \
           file://stats.toml"

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
    # Default config — operators can edit either by hand or via the web
    # admin's "Stats config" modal (which round-trips through the
    # WriteServiceConfig helper command and atomic-replaces this file).
    install -d ${D}${sysconfdir}/bananas
    install -m 0644 ${WORKDIR}/stats.toml ${D}${sysconfdir}/bananas/stats.toml
}

FILES:${PN} += "${bindir}/bananas-stats \
                ${systemd_system_unitdir}/bananas-stats.service \
                ${sysconfdir}/bananas/stats.toml"
# Mark the config as a CONFFILE so package-management upgrades don't
# silently overwrite operator edits.
CONFFILES:${PN} += "${sysconfdir}/bananas/stats.toml"
RDEPENDS:${PN} += "bananas-server"
