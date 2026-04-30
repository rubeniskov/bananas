SUMMARY = "BanaNAS opkg feed configuration"
DESCRIPTION = "Ships /etc/opkg/customfeeds.conf with three src/gz entries \
pointing at the GitHub Pages opkg feed published by the release workflow. \
Without this file the on-device opkg has no upstream to upgrade against."

LICENSE = "MIT"
LIC_FILES_CHKSUM = "file://${COMMON_LICENSE_DIR}/MIT;md5=0835ade698e0bcf8506ecda2f7b4f302"

# PV sourced from <repo>/assets/version.txt — see comment in
# bananas-server.bb. The feed config doesn't change between releases
# (the URLs are stable), but tracking the workspace version keeps
# opkg's metadata honest on inspection.
require recipes-bsp/bananas-version.inc

# Pure config — no per-machine variation, lands in feed/latest/all/.
PACKAGE_ARCH = "all"

SRC_URI = "file://customfeeds.conf"
S = "${WORKDIR}"

do_compile[noexec] = "1"
do_configure[noexec] = "1"

do_install() {
    install -d ${D}${sysconfdir}/opkg
    install -m 0644 ${WORKDIR}/customfeeds.conf ${D}${sysconfdir}/opkg/customfeeds.conf
}

# Mark customfeeds.conf as a CONFFILE so an operator who edits the file
# (e.g. to point at a private mirror, or pin to a specific feed/v<tag>/
# directory) doesn't lose their changes on the next opkg upgrade of
# this package.
CONFFILES:${PN} = "${sysconfdir}/opkg/customfeeds.conf"

FILES:${PN} = "${sysconfdir}/opkg/customfeeds.conf"
