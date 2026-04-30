SUMMARY = "BanaNAS modprobe blacklist drop-ins"
DESCRIPTION = "Static /etc/modprobe.d/ overlays for the BanaNAS image. \
Currently blacklists brcmfmac (Broadcom 43362 WiFi) — the BPI-M1+ ships \
that chip on-board, but we don't ship the proprietary brcmfmac43362-sdio \
firmware blob, so brcmfmac probes, fails to load firmware, and spams \
~13 seconds of `HT Avail timeout` errors during every boot. Wired \
gigabit Ethernet covers all NAS traffic; the WiFi chip is dead weight."

LICENSE = "MIT"
LIC_FILES_CHKSUM = "file://${COMMON_LICENSE_DIR}/MIT;md5=0835ade698e0bcf8506ecda2f7b4f302"

# PV sourced from <repo>/assets/version.txt — see comment in
# bananas-server.bb for the rationale.
require recipes-bsp/bananas-version.inc

SRC_URI = "file://blacklist-brcmfmac.conf"

S = "${WORKDIR}"

COMPATIBLE_MACHINE = "(bananapro)"

do_install() {
    install -d ${D}${sysconfdir}/modprobe.d
    install -m 0644 ${WORKDIR}/blacklist-brcmfmac.conf ${D}${sysconfdir}/modprobe.d/blacklist-brcmfmac.conf
}

FILES:${PN} = "${sysconfdir}/modprobe.d/blacklist-brcmfmac.conf"
