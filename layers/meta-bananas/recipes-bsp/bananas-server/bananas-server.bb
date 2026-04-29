SUMMARY = "BanaNAS web admin: bananas-server (HTTP UI) + bananas-helper (root)"
DESCRIPTION = "Two prebuilt Rust binaries cross-compiled on the host with \
cargo-zigbuild for armv7-unknown-linux-gnueabihf, plus the Dioxus Web UI \
bundle (wasm/CSS/HTML). The unprivileged HTTP daemon listens on :8080, \
serves the wasm SPA from /usr/share/bananas/ui/, and reaches a small \
root-owned helper over a Unix socket for tightly-scoped operations \
(NFS exports rewrite, group ops). \
Run `pixi run build-ui && pixi run build-server-arm` before `pixi run build` \
to stage the binaries in serve/bin/ and the UI bundle in serve/ui/."

LICENSE = "MIT"
LIC_FILES_CHKSUM = "file://${COMMON_LICENSE_DIR}/MIT;md5=0835ade698e0bcf8506ecda2f7b4f302"

inherit systemd useradd

# Pull binaries + UI bundle from serve/bin/ (top-level repo path; COREBASE is
# poky/, so ../serve/bin is correct) and the unit files from the local files/
# dir. The UI bundle ships as a tarball so its contents are part of the
# SRC_URI checksum — without that, bitbake caches do_install based only on
# the binary checksums and ships a stale UI when only wasm/CSS changes.
FILESEXTRAPATHS:prepend := "${THISDIR}/files:${COREBASE}/../serve/bin:"
SRC_URI = "file://bananas-helper.service \
           file://bananas-server.service \
           file://bananas-server \
           file://bananas-helper \
           file://bananas-ui.tar.gz \
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

SYSTEMD_SERVICE:${PN} = "bananas-helper.service bananas-server.service"
SYSTEMD_AUTO_ENABLE = "enable"

USERADD_PACKAGES = "${PN}"
# `bananas`: the unprivileged service user.
# `bananas-admin`: authorization gate for the web UI. Members can sign in;
# everyone else (including freshly created users) gets a generic
# "invalid credentials" 401. Root bypasses the check because it always
# has access via SSH and the helper anyway.
GROUPADD_PARAM:${PN} = "-r bananas; -r bananas-admin"
USERADD_PARAM:${PN} = "-r -g bananas -d /var/lib/bananas -s /sbin/nologin \
                       -c 'BanaNAS web admin' bananas"

do_compile[noexec] = "1"
do_configure[noexec] = "1"

do_install() {
    install -d ${D}${bindir}
    install -m 0755 ${WORKDIR}/bananas-server ${D}${bindir}/bananas-server
    install -m 0755 ${WORKDIR}/bananas-helper ${D}${bindir}/bananas-helper

    install -d ${D}${systemd_system_unitdir}
    install -m 0644 ${WORKDIR}/bananas-helper.service ${D}${systemd_system_unitdir}/
    install -m 0644 ${WORKDIR}/bananas-server.service ${D}${systemd_system_unitdir}/

    # Cockpit-style SSH login banner: ASCII art + live web-console URL.
    install -d ${D}${sysconfdir}/profile.d
    install -m 0644 ${WORKDIR}/bananas-motd.sh ${D}${sysconfdir}/profile.d/bananas-motd.sh

    # UI bundle (wasm + CSS + HTML). do_unpack already extracted the tarball
    # alongside the binaries in WORKDIR. bananas-server reads BANANAS_UI_DIR
    # from its unit file (default /usr/share/bananas/ui). bbfatal if missing —
    # forgetting `pixi run build-ui` before `build` would otherwise ship a
    # 404-only server.
    if [ ! -f "${WORKDIR}/index.html" ]; then
        bbfatal "UI bundle missing — index.html not found in WORKDIR. Run 'pixi run build-ui' first."
    fi
    install -d ${D}${datadir}/bananas/ui
    cp -r ${WORKDIR}/index.html ${WORKDIR}/assets ${D}${datadir}/bananas/ui/
    chmod -R u=rwX,go=rX ${D}${datadir}/bananas/ui

    # Persistent state dir — bananas-server.service has StateDirectory=bananas,
    # so systemd creates /var/lib/bananas (mode 0700, owned by the bananas
    # user) on the first start. We don't ship the dir in the package itself.
}

FILES:${PN} += "${bindir}/bananas-server \
                ${bindir}/bananas-helper \
                ${systemd_system_unitdir}/bananas-helper.service \
                ${systemd_system_unitdir}/bananas-server.service \
                ${datadir}/bananas/ui \
                ${sysconfdir}/profile.d/bananas-motd.sh"
