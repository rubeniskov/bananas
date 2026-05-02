SUMMARY = "BanaNAS cloud-sync plugin (rclone-driven push/pull/bisync)"
DESCRIPTION = "Optional plugin daemon answering /api/cloud/* (via \
bananas-router) and /assets/cloud/* (via bananas-webadmin sub-proxy). \
Manages cloud accounts (Drive, Dropbox, OneDrive, S3, WebDAV, FTP), \
schedules cron-driven syncs, and dispatches each run to bananas-engine \
for the privileged rclone exec. Pulls bananas-rclone in via Depends so \
a single `opkg install bananas-cloud` brings up the full feature; the \
default image ships none of this. Cross-built by \
`pixi run build-webadmin-arm` into serve/bin/bananas-cloud."

LICENSE = "MIT"
LIC_FILES_CHKSUM = "file://${COMMON_LICENSE_DIR}/MIT;md5=0835ade698e0bcf8506ecda2f7b4f302"

require recipes-bsp/bananas-version.inc

inherit systemd

FILESEXTRAPATHS:prepend := "${THISDIR}/files:${COREBASE}/../serve/bin/${TUNE_PKGARCH}:"
SRC_URI = "file://bananas-cloud.service \
           file://bananas-cloud \
           file://cloud.toml"

S = "${WORKDIR}"

COMPATIBLE_MACHINE = "(bananas-bpi|bananas-rpi)"
INHIBIT_PACKAGE_STRIP = "1"
INSANE_SKIP:${PN} += "arch already-stripped"

SYSTEMD_SERVICE:${PN} = "bananas-cloud.service"
SYSTEMD_AUTO_ENABLE = "enable"

# Hard runtime deps:
#   bananas-rclone — provides /usr/bin/rclone the engine shells out to
#                    for every sync run. Without it, RunCloudSync exits
#                    with a structured "rclone not installed" error.
#   bananas-router — Unix-socket API gateway; reads the manifest this
#                    package drops at install time to route /api/cloud/*
#                    here.
#   bananas-webadmin — owns the public TCP port :8080 and sub-proxies
#                      /assets/cloud/* to this daemon's socket. Also
#                      issues the session cookies this daemon validates.
#
# The cloud SPA is embedded directly in this binary via include_dir!
# (see crates/cloud/build.rs + src/embedded.rs); the separate
# bananas-cloud-ui IPK was retired in v1.5.
RDEPENDS:${PN} += "bananas-rclone bananas-router bananas-webadmin"

do_compile[noexec] = "1"
do_configure[noexec] = "1"

do_install() {
    install -d ${D}${bindir}
    install -m 0755 ${WORKDIR}/bananas-cloud ${D}${bindir}/bananas-cloud

    install -d ${D}${systemd_system_unitdir}
    install -m 0644 ${WORKDIR}/bananas-cloud.service ${D}${systemd_system_unitdir}/

    # Extension manifest — bananas-router reads it for API routing
    # (/api/cloud/* → this daemon's socket); bananas-webadmin reads
    # it to populate its plugin-asset map (/assets/cloud/* → same
    # socket). Both daemons re-read the dir on restart.
    install -d ${D}${sysconfdir}/bananas/extensions.d
    install -m 0644 ${WORKDIR}/cloud.toml ${D}${sysconfdir}/bananas/extensions.d/cloud.toml
}

# Restart both manifest-reading daemons after install/remove so
# their tables include / drop this plugin. ~50 ms downtime per
# daemon.
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

FILES:${PN} += "${bindir}/bananas-cloud \
                ${systemd_system_unitdir}/bananas-cloud.service \
                ${sysconfdir}/bananas/extensions.d/cloud.toml"
