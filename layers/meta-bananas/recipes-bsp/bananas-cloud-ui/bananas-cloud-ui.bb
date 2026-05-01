SUMMARY = "BanaNAS cloud-sync SPA (wasm bundle, optional)"
DESCRIPTION = "Dioxus 0.7 wasm32-unknown-unknown single-page app served by \
bananas-cloud from /usr/share/bananas/cloud-ui/. Built by \
`pixi run build-cloud-ui` into serve/bin/bananas-cloud-ui.tar.gz with \
brotli + gzip precompressed companions for every asset. Optional plugin \
package — pulled by bananas-cloud, not part of the default image."

LICENSE = "MIT"
LIC_FILES_CHKSUM = "file://${COMMON_LICENSE_DIR}/MIT;md5=0835ade698e0bcf8506ecda2f7b4f302"

require recipes-bsp/bananas-version.inc

FILESEXTRAPATHS:prepend := "${COREBASE}/../serve/bin:"
SRC_URI = "file://bananas-cloud-ui.tar.gz"

S = "${WORKDIR}"

# Pure wasm + static assets — architecture-independent.
inherit allarch

do_compile[noexec] = "1"
do_configure[noexec] = "1"

do_install() {
    if [ ! -f "${WORKDIR}/index.html" ]; then
        bbfatal "cloud-ui bundle missing — index.html not found in WORKDIR. Run 'pixi run build-cloud-ui' first."
    fi
    install -d ${D}${datadir}/bananas/cloud-ui
    cp -r ${WORKDIR}/index.html ${WORKDIR}/assets ${D}${datadir}/bananas/cloud-ui/
    chmod -R u=rwX,go=rX ${D}${datadir}/bananas/cloud-ui
}

FILES:${PN} = "${datadir}/bananas/cloud-ui"
