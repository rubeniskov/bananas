SUMMARY = "BanaNAS web admin: bananas-webadmin (HTTP UI) + bananas-engine (root)"
DESCRIPTION = "Two prebuilt Rust binaries cross-compiled on the host with \
cargo-zigbuild for armv7-unknown-linux-gnueabihf. The unprivileged HTTP daemon \
listens on :8080, serves the wasm SPA from /usr/share/bananas/webadmin/ \
(shipped by the sibling bananas-webadmin-ui package), and reaches the privileged \
bananas-engine over a Unix socket for everything that needs root (NFS exports, \
fstab, users, service configs, opkg upgrades, smartctl, lsblk, reboot). \
Run `pixi run build-webadmin-arm` before `pixi run build` to stage the binaries \
in serve/bin/."

LICENSE = "MIT"
LIC_FILES_CHKSUM = "file://${COMMON_LICENSE_DIR}/MIT;md5=0835ade698e0bcf8506ecda2f7b4f302"

# PV sourced from <repo>/assets/version.txt so .ipk filenames track
# the Cargo workspace version. Without this each release shipped IPKs
# stuck at the bitbake default `1.0` and opkg saw no upgrade.
require recipes-bsp/bananas-version.inc

inherit systemd useradd

# Pull binaries from serve/bin/ (top-level repo path; COREBASE is poky/,
# so ../serve/bin is correct) and the unit files from the local files/
# dir. The wasm SPA bundle ships as its own bananas-webadmin-ui package
# (RDEPENDS below) so SPA-only updates don't churn this IPK + restart
# the HTTP daemon.
FILESEXTRAPATHS:prepend := "${THISDIR}/files:${COREBASE}/../serve/bin:"
SRC_URI = "file://bananas-engine.service \
           file://bananas-webadmin.service \
           file://bananas-webadmin \
           file://bananas-engine \
           file://bananas-motd.sh"

S = "${WORKDIR}"

# These binaries are armv7-unknown-linux-gnueabihf — only valid for the
# bananapro machine.
COMPATIBLE_MACHINE = "(bananapro)"

# The release binaries are already stripped by zig; QA pass would strip
# again, then complain. Skip the strip + the arch check (Yocto's `file`
# probe reports "ARM, EABI5" while the target is cortexa7t2hf-neon).
INHIBIT_PACKAGE_STRIP = "1"
INSANE_SKIP:${PN} += "arch already-stripped"

SYSTEMD_SERVICE:${PN} = "bananas-engine.service bananas-webadmin.service"
SYSTEMD_AUTO_ENABLE = "enable"

# The HTTP daemon serves the SPA from /usr/share/bananas/webadmin/.
# Pull bananas-webadmin-ui in transitively so installing this package
# also installs the SPA bundle. Splitting the SPA into its own package
# means SPA-only updates skip the daemon restart (ServeDir re-stats
# files per request).
RDEPENDS:${PN} += "bananas-webadmin-ui"

USERADD_PACKAGES = "${PN}"
# `bananas`: the unprivileged service user.
# `bananas-admin`: authorization gate for the web UI. Members can sign in;
# everyone else (including freshly created users) gets a generic
# "invalid credentials" 401. Root bypasses the check because it always
# has access via SSH and the engine anyway.
GROUPADD_PARAM:${PN} = "-r bananas; -r bananas-admin"
USERADD_PARAM:${PN} = "-r -g bananas -d /var/lib/bananas -s /sbin/nologin \
                       -c 'BanaNAS web admin' bananas"

do_compile[noexec] = "1"
do_configure[noexec] = "1"

do_install() {
    install -d ${D}${bindir}
    install -m 0755 ${WORKDIR}/bananas-webadmin ${D}${bindir}/bananas-webadmin
    install -m 0755 ${WORKDIR}/bananas-engine ${D}${bindir}/bananas-engine

    install -d ${D}${systemd_system_unitdir}
    install -m 0644 ${WORKDIR}/bananas-engine.service ${D}${systemd_system_unitdir}/
    install -m 0644 ${WORKDIR}/bananas-webadmin.service ${D}${systemd_system_unitdir}/

    # Cockpit-style SSH login banner: ASCII art + live web-console URL.
    install -d ${D}${sysconfdir}/profile.d
    install -m 0644 ${WORKDIR}/bananas-motd.sh ${D}${sysconfdir}/profile.d/bananas-motd.sh

    # Persistent state dir — bananas-webadmin.service has StateDirectory=bananas,
    # so systemd creates /var/lib/bananas (mode 0700, owned by the bananas
    # user) on the first start. We don't ship the dir in the package itself.
}

FILES:${PN} += "${bindir}/bananas-webadmin \
                ${bindir}/bananas-engine \
                ${systemd_system_unitdir}/bananas-engine.service \
                ${systemd_system_unitdir}/bananas-webadmin.service \
                ${sysconfdir}/profile.d/bananas-motd.sh"
