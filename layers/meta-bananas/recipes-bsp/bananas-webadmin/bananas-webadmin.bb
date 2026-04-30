SUMMARY = "BanaNAS web admin SPA (wasm bundle)"
DESCRIPTION = "Dioxus 0.7 wasm32-unknown-unknown single-page app served by \
bananas-server from /usr/share/bananas/webadmin/. Built by \
`pixi run build-webadmin` into serve/bin/bananas-webadmin.tar.gz with \
brotli + gzip precompressed companions for every asset."

LICENSE = "MIT"
LIC_FILES_CHKSUM = "file://${COMMON_LICENSE_DIR}/MIT;md5=0835ade698e0bcf8506ecda2f7b4f302"

# PV sourced from <repo>/assets/version.txt — see comment in
# bananas-server.bb for the rationale.
require recipes-bsp/bananas-version.inc

# Pull the bundle from serve/bin/ (top-level repo path; COREBASE is
# poky/, so ../serve/bin is correct).
FILESEXTRAPATHS:prepend := "${COREBASE}/../serve/bin:"
SRC_URI = "file://bananas-webadmin.tar.gz"

S = "${WORKDIR}"

# Pure wasm + static assets — architecture-independent. `inherit
# allarch` is the canonical way to declare this; just setting
# PACKAGE_ARCH = "all" produces an .ipk file but the rootfs-time
# manifest lookup uses `allarch` as the search key, so the image
# can't find the package and bails with "sstate manifest not found".
inherit allarch

do_compile[noexec] = "1"
do_configure[noexec] = "1"

do_install() {
    # do_unpack already extracted bananas-webadmin.tar.gz into WORKDIR,
    # so index.html + assets/ are sitting at the top level. bbfatal if
    # missing — forgetting `pixi run build-webadmin` before `build`
    # would ship a 404-only image.
    if [ ! -f "${WORKDIR}/index.html" ]; then
        bbfatal "web-admin bundle missing — index.html not found in WORKDIR. Run 'pixi run build-webadmin' first."
    fi
    install -d ${D}${datadir}/bananas/webadmin
    # cp -r preserves the .br / .gz companions the build-webadmin task
    # emits next to each asset; bananas-server's ServeDir serves them
    # transparently via Accept-Encoding negotiation.
    cp -r ${WORKDIR}/index.html ${WORKDIR}/assets ${D}${datadir}/bananas/webadmin/
    chmod -R u=rwX,go=rX ${D}${datadir}/bananas/webadmin
}

FILES:${PN} = "${datadir}/bananas/webadmin"
