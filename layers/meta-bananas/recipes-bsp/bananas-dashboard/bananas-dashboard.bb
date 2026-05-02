SUMMARY = "BanaNAS LCD dashboard"
DESCRIPTION = "Slint app rendering live CPU / memory / network / disk \
panels on the BanaNAS RGB888 LCD. Reads /var/lib/bananas/stats.db \
(SQLite WAL) read-only, the same store the web admin consumes. Drives \
the panel directly via the Slint software renderer plus the \
linuxkms-noseat backend, with no X server and no Wayland."

LICENSE = "MIT"
LIC_FILES_CHKSUM = "file://${COMMON_LICENSE_DIR}/MIT;md5=0835ade698e0bcf8506ecda2f7b4f302"

# PV sourced from <repo>/assets/version.txt — see comment in
# bananas-webadmin.bb for the rationale.
require recipes-bsp/bananas-version.inc

inherit systemd

# Pull the cross-rs-built binary from serve/bin/ alongside the unit file.
FILESEXTRAPATHS:prepend := "${THISDIR}/files:${COREBASE}/../serve/bin/${TUNE_PKGARCH}:"
SRC_URI = "file://bananas-dashboard \
           file://bananas-dashboard.service \
           file://dashboard.toml"

S = "${WORKDIR}"

# Slint LCD app is bananapro-only. The RPi target uses
# bananas-dashboard-web (the SPA daemon) for the same data set; an
# aarch64 cross-build of Slint + fontconfig sysroot is a v2.1 follow-up
# (Cross.toml only has the armv7 multiarch entries today).
COMPATIBLE_MACHINE = "(bananapro)"
INHIBIT_PACKAGE_STRIP = "1"
INSANE_SKIP:${PN} += "arch already-stripped"

SYSTEMD_SERVICE:${PN} = "bananas-dashboard.service"
SYSTEMD_AUTO_ENABLE = "enable"

# Runtime libraries the cross-rs build linked against. Software +
# femtovg renderers, linuxkms-noseat backend, GLES2 via Mali-400's
# lima driver:
#  * fontconfig — fontique font discovery (panics with NoMatch if
#    fonts aren't there).
#  * libudev + libinput — input device enumeration on KMS.
#  * libxkbcommon — keymap.
#  * libdrm + libgbm — GBM buffer allocation between CPU and the GPU
#    for the femtovg path.
#  * mesa-driver-lima — userspace Gallium driver that talks to the
#    in-kernel `lima` DRM driver. Without this the GLES2 paint calls
#    fall back to softpipe and we lose the perf win that justified
#    enabling femtovg in the first place.
#  * kernel-module-lima — autoloaded so /dev/dri/renderD128 appears
#    before the dashboard tries to open it.
#  * ttf-dejavu-sans — at least one TTF on disk.
RDEPENDS:${PN} += " \
    bananas-stats \
    fontconfig \
    libudev \
    libxkbcommon \
    libinput \
    libdrm \
    libgbm \
    libegl \
    libgles2 \
    mesa-megadriver \
    kernel-module-lima \
    ttf-dejavu-sans \
"

do_compile[noexec] = "1"
do_configure[noexec] = "1"

do_install() {
    install -d ${D}${bindir}
    install -m 0755 ${WORKDIR}/bananas-dashboard ${D}${bindir}/bananas-dashboard
    install -d ${D}${systemd_system_unitdir}
    install -m 0644 ${WORKDIR}/bananas-dashboard.service ${D}${systemd_system_unitdir}/
    install -d ${D}${sysconfdir}/bananas
    install -m 0644 ${WORKDIR}/dashboard.toml ${D}${sysconfdir}/bananas/dashboard.toml
}

FILES:${PN} += "${bindir}/bananas-dashboard \
                ${systemd_system_unitdir}/bananas-dashboard.service \
                ${sysconfdir}/bananas/dashboard.toml"

CONFFILES:${PN} += "${sysconfdir}/bananas/dashboard.toml"
