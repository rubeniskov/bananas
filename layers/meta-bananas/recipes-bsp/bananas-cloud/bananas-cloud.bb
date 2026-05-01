SUMMARY = "BanaNAS cloud-sync plugin (rclone-driven push/pull/bisync)"
DESCRIPTION = "Optional plugin daemon that owns /api/cloud/* and /cloud/* \
behind bananas-router. Manages cloud accounts (Drive, Dropbox, OneDrive, \
S3, WebDAV, FTP), schedules cron-driven syncs, and dispatches each run \
to bananas-engine for the privileged rclone exec. Pulls bananas-rclone \
in via Depends so a single `opkg install bananas-cloud` brings up the \
full feature; the default image ships none of this. Cross-built by \
`pixi run build-webadmin-arm` into serve/bin/bananas-cloud."

LICENSE = "MIT"
LIC_FILES_CHKSUM = "file://${COMMON_LICENSE_DIR}/MIT;md5=0835ade698e0bcf8506ecda2f7b4f302"

require recipes-bsp/bananas-version.inc

inherit systemd

FILESEXTRAPATHS:prepend := "${THISDIR}/files:${COREBASE}/../serve/bin:"
SRC_URI = "file://bananas-cloud.service \
           file://bananas-cloud \
           file://cloud.toml"

S = "${WORKDIR}"

COMPATIBLE_MACHINE = "(bananapro)"
INHIBIT_PACKAGE_STRIP = "1"
INSANE_SKIP:${PN} += "arch already-stripped"

SYSTEMD_SERVICE:${PN} = "bananas-cloud.service"
SYSTEMD_AUTO_ENABLE = "enable"

# Hard runtime deps:
#   bananas-rclone — provides /usr/bin/rclone the engine shells out to
#                    for every sync run. Without it, RunCloudSync exits
#                    with a structured "rclone not installed" error.
#   bananas-router — owns :8080 and proxies /api/cloud/* + /cloud/* here
#                    via the manifest this package drops at install time.
#   bananas-webadmin — issues the session cookies this daemon validates.
RDEPENDS:${PN} += "bananas-rclone bananas-router bananas-webadmin bananas-cloud-ui"

do_compile[noexec] = "1"
do_configure[noexec] = "1"

do_install() {
    install -d ${D}${bindir}
    install -m 0755 ${WORKDIR}/bananas-cloud ${D}${bindir}/bananas-cloud

    install -d ${D}${systemd_system_unitdir}
    install -m 0644 ${WORKDIR}/bananas-cloud.service ${D}${systemd_system_unitdir}/

    # Extension manifest — bananas-router reads this on
    # `systemctl reload bananas-router` to learn this daemon owns
    # /api/cloud/* and /cloud/* paths.
    install -d ${D}${sysconfdir}/bananas/extensions.d
    install -m 0644 ${WORKDIR}/cloud.toml ${D}${sysconfdir}/bananas/extensions.d/cloud.toml
}

# Restart bananas-router after install/remove so the manifest table
# is rebuilt with the new/dropped manifest. ~50 ms downtime.
pkg_postinst:${PN}() {
    if [ -z "$D" ]; then
        systemctl restart bananas-router.service 2>/dev/null || true
    fi
}

pkg_postrm:${PN}() {
    if [ -z "$D" ]; then
        systemctl restart bananas-router.service 2>/dev/null || true
    fi
}

FILES:${PN} += "${bindir}/bananas-cloud \
                ${systemd_system_unitdir}/bananas-cloud.service \
                ${sysconfdir}/bananas/extensions.d/cloud.toml"
