SUMMARY = "BanaNAS LCD dashboard"
DESCRIPTION = "Slint app rendering live CPU / memory / network / disk \
panels on the BanaNAS RGB888 LCD. Reads /var/lib/bananas/stats.db \
(SQLite WAL) read-only, the same store the web admin consumes. Drives \
the panel directly via the Slint software renderer plus the \
linuxkms-noseat backend, with no X server and no Wayland."

LICENSE = "MIT"
LIC_FILES_CHKSUM = "file://${COMMON_LICENSE_DIR}/MIT;md5=0835ade698e0bcf8506ecda2f7b4f302"

inherit systemd

# Pull the cross-rs-built binary from serve/bin/ alongside the unit file.
FILESEXTRAPATHS:prepend := "${THISDIR}/files:${COREBASE}/../serve/bin:"
SRC_URI = "file://bananas-dashboard \
           file://bananas-dashboard.service"

S = "${WORKDIR}"

COMPATIBLE_MACHINE = "(bananapro)"
INHIBIT_PACKAGE_STRIP = "1"
INSANE_SKIP:${PN} += "arch already-stripped"

SYSTEMD_SERVICE:${PN} = "bananas-dashboard.service"
SYSTEMD_AUTO_ENABLE = "enable"

# Runtime libraries the cross-rs build linked against. The cross
# container apt-installed dev packages (libfontconfig1-dev:armhf etc.)
# but on-device we need the corresponding Yocto runtime packages so
# the binary's NEEDED entries resolve at startup.
RDEPENDS:${PN} += " \
    bananas-stats \
    fontconfig \
    libdrm \
    libgbm \
    libudev \
    libxkbcommon \
    libinput \
"

do_compile[noexec] = "1"
do_configure[noexec] = "1"

do_install() {
    install -d ${D}${bindir}
    install -m 0755 ${WORKDIR}/bananas-dashboard ${D}${bindir}/bananas-dashboard
    install -d ${D}${systemd_system_unitdir}
    install -m 0644 ${WORKDIR}/bananas-dashboard.service ${D}${systemd_system_unitdir}/
}

FILES:${PN} += "${bindir}/bananas-dashboard \
                ${systemd_system_unitdir}/bananas-dashboard.service"
