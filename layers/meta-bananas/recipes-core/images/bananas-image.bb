SUMMARY = "Headless NAS/NFS image for Banana Pro (BPI-M1+)"
LICENSE = "MIT"

inherit core-image

IMAGE_FEATURES += "ssh-server-openssh package-management splash"

# Root password (SHA-512 hash) baked into /etc/shadow.
#
# SSH keeps PermitRootLogin = prohibit-password (OpenSSH default), so this
# password only unlocks the serial console + LCD getty + the web admin's
# login form; SSH itself stays key-only.
#
# `ROOT_PASSWORD_HASH` is now OPTIONAL — when unset, we ship the default
# placeholder below ("bananas"). The `expire_root_password` postprocess
# zeroes the lastchg field, so PAM (console) and `bananas-helper`'s
# `verify_shadow_password` (web UI) both flag the credential as expired
# on first sign-in and force the operator to rotate it before granting a
# session. Operators who want the build to embed a specific hash they
# already trust can still set ROOT_PASSWORD_HASH in `.env`.
#
# Implementation note: extrausers/EXTRA_USERS_PARAMS is avoided here
# because its eval pass strips '$' chars from SHA-512 hashes. We patch
# /etc/shadow directly via sed in single-quoted shell, so the hash is
# bitbake-substituted but never re-evaluated by the shell.
ROOT_PASSWORD_HASH ??= "$6$bananas$wLIzzjPHUgftWxH6Mos.t/90VkyZZoZvOr/hKobl0Mx1plIx6.HBYIFvMpk9SjOCLQrXbRuW8udE70.wACVAi."

# Force do_rootfs to invalidate when either of these change. Without this,
# kas-imported env vars (via kas.yml's `env:` block) don't always make it
# into bitbake's task-signature hash, and a stale cached rootfs gets
# reused when only .env changed.
do_rootfs[vardeps] += "ROOT_PASSWORD_HASH"

ROOTFS_POSTPROCESS_COMMAND += "set_root_password;"

set_root_password() {
    sed -i 's%^root:[^:]*:%root:${ROOT_PASSWORD_HASH}:%' ${IMAGE_ROOTFS}/etc/shadow
}

ROOT_AUTHKEY := "${THISDIR}/files/authorized_keys"

ROOTFS_POSTPROCESS_COMMAND += "install_root_authkey;"

install_root_authkey() {
    root_home=$(awk -F: '$1=="root"{print $6}' ${IMAGE_ROOTFS}/etc/passwd)
    install -d -m 0700 ${IMAGE_ROOTFS}${root_home}/.ssh
    install -m 0600 ${ROOT_AUTHKEY} ${IMAGE_ROOTFS}${root_home}/.ssh/authorized_keys
    chown -R root:root ${IMAGE_ROOTFS}${root_home}/.ssh
}

# Force root to change its password on first sign-in. This sets shadow's
# third field (last password change, in days since epoch) to 0 — PAM
# treats that as "must change at next login" for both serial console and
# any password-using SSH session. The web UI checks the same field via
# bananas-helper and surfaces a "set new password" form on the login
# page until it's been rotated. After the first ChangePassword the
# field is bumped to today's day count and the prompt stops appearing.
ROOTFS_POSTPROCESS_COMMAND += "expire_root_password;"

expire_root_password() {
    sed -i 's%^\(root:[^:]*\):[^:]*:%\1:0:%' ${IMAGE_ROOTFS}/etc/shadow
}

# First-boot rootfs grow — only fires on MMC boots; NFS rootfs (the
# iterate-loop dev path) makes the script a no-op. The guard file is
# created on success so the service short-circuits on every subsequent
# boot via ConditionPathExists=.
FIRSTBOOT_SCRIPT := "${THISDIR}/files/bananas-firstboot-resize"
FIRSTBOOT_UNIT   := "${THISDIR}/files/bananas-firstboot-resize.service"
SWAP_SYSCTL      := "${THISDIR}/files/90-bananas-swap.conf"

ROOTFS_POSTPROCESS_COMMAND += "install_firstboot_resize;"

install_firstboot_resize() {
    install -d -m 0755 ${IMAGE_ROOTFS}/usr/sbin
    install -m 0755 ${FIRSTBOOT_SCRIPT} ${IMAGE_ROOTFS}/usr/sbin/bananas-firstboot-resize
    install -d -m 0755 ${IMAGE_ROOTFS}/lib/systemd/system
    install -m 0644 ${FIRSTBOOT_UNIT} \
        ${IMAGE_ROOTFS}/lib/systemd/system/bananas-firstboot-resize.service
    # multi-user.target.wants — see the inline comment in the unit file
    # for why this is NOT local-fs.target.wants. tl;dr: the previous
    # placement created an ordering cycle that silently broke
    # systemd-tmpfiles-setup and cascaded into 4 other services failing.
    install -d -m 0755 ${IMAGE_ROOTFS}/etc/systemd/system/multi-user.target.wants
    ln -sf /lib/systemd/system/bananas-firstboot-resize.service \
        ${IMAGE_ROOTFS}/etc/systemd/system/multi-user.target.wants/bananas-firstboot-resize.service
    # vm.swappiness=10 drop-in. The firstboot-resize script carves a
    # RAM-sized swap partition (capped at 1 GiB) on the SD tail; this
    # sysctl keeps it cold on the NAS workload so flash-wear stays low.
    install -d -m 0755 ${IMAGE_ROOTFS}/etc/sysctl.d
    install -m 0644 ${SWAP_SYSCTL} \
        ${IMAGE_ROOTFS}/etc/sysctl.d/90-bananas-swap.conf
}

# Wire up chronyd at boot. Yocto's systemctl preset run during rootfs
# assembly marks the unit `enabled` per the upstream preset file but
# does NOT write the `WantedBy=multi-user.target` symlink, so chrony
# never actually starts at boot and the system sits on RTC drift.
# Same enabled-but-no-symlink bug bites systemd-timesyncd; we don't
# fix that here because chrony is what IMAGE_INSTALL pulls in (the two
# are mutually-exclusive NTP impls — running both would conflict).
# Mirrors the install_nfs_server pattern below.
ROOTFS_POSTPROCESS_COMMAND += "install_chrony;"

install_chrony() {
    install -d -m 0755 ${IMAGE_ROOTFS}/etc/systemd/system/multi-user.target.wants
    ln -sf /lib/systemd/system/chronyd.service \
        ${IMAGE_ROOTFS}/etc/systemd/system/multi-user.target.wants/chronyd.service
}

# Wire up nfs-server.service so the daemon is ready the moment the
# operator adds an export through the web UI — but ship /etc/exports
# empty. Same story for /etc/fstab: no default mount entries are
# baked in; the operator configures storage through the Mount points
# tab post-boot. /srv is left as a conventional landing spot for new
# mounts (created by base-files), but no children are pre-populated.
ROOTFS_POSTPROCESS_COMMAND += "install_nfs_server;"

install_nfs_server() {
    install -d -m 0755 ${IMAGE_ROOTFS}/etc/systemd/system/multi-user.target.wants
    ln -sf /lib/systemd/system/nfs-server.service \
        ${IMAGE_ROOTFS}/etc/systemd/system/multi-user.target.wants/nfs-server.service

    # nfs-server.service's ExecStart is "rpc.nfsd $NFSD_OPTS $NFSD_COUNT",
    # sourced from /etc/nfs-utils.conf which OE doesn't ship. Without
    # NFSD_COUNT rpc.nfsd exits 1 and the unit fails — so we keep this
    # drop-in even with an empty /etc/exports.
    install -d -m 0755 ${IMAGE_ROOTFS}/etc/systemd/system/nfs-server.service.d
    cat > ${IMAGE_ROOTFS}/etc/systemd/system/nfs-server.service.d/override.conf <<EOF
[Unit]
# Pull nfs-statd in alongside nfs-server. macOS NFS clients refuse the
# mount with "RPC prog. not avail" if the lock-state daemon (statd, RPC
# program 100024) isn't registered with rpcbind, even when the client
# passes nolocks. Wants= keeps it loose: statd-down does not block
# nfs-server, but starting nfs-server starts statd as a side effect.
Wants=nfs-statd.service

[Service]
Environment=NFSD_COUNT=8
EOF
}

# Base packages for a small NAS/server
IMAGE_INSTALL += " \
    bash \
    coreutils \
    e2fsprogs \
    e2fsprogs-resize2fs \
    e2fsprogs-tune2fs \
    e2fsprogs-mke2fs \
    e2fsprogs-e2fsck \
    parted \
    gptfdisk \
    dosfstools \
    util-linux \
    util-linux-blkid \
    util-linux-mkswap \
    util-linux-swaponoff \
    util-linux-partx \
    procps \
    rsync \
    curl \
    wget \
    htop \
    iproute2 \
    iptables \
    ethtool \
    net-tools \
    openssh \
    openssh-sftp-server \
    ca-certificates \
    tzdata \
    chrony \
    nfs-utils \
    rpcbind \
    avahi-daemon \
    avahi-utils \
    usbutils \
    i2c-tools \
    bananas-server \
    bananas-stats \
    bananas-config \
"
# bananas-dashboard (Slint LCD UI) and the Mali GPU userspace stack
# (mesa lima + libegl + libgles2 + libgbm + libdrm + fontconfig + fonts)
# are intentionally NOT pulled in yet — the Slint cross-compile needs an
# armhf fontconfig + pkg-config sysroot we don't have. Add them back once
# build-stats-arm switches to `cross` (docker w/ multiarch fontconfig-dev)
# or vendors a sysroot. The CONFIG_DRM_LIMA kernel fragment stays so a
# userspace lima driver is functional the moment we ship it.

# Optional CIFS/Samba (comment in if needed)
# IMAGE_INSTALL += " samba samba-client cifs-utils"

# Optional storage helpers
IMAGE_INSTALL += " \
    mdadm \
    hdparm \
    smartmontools \
"

# Kernel modules often useful for NAS bring-up (board/branch dependent)
IMAGE_INSTALL += " \
    kernel-modules \
"

# /etc/modprobe.d/ drop-ins (currently: blacklist brcmfmac so the missing
# WiFi firmware doesn't spam the boot log).
IMAGE_INSTALL += " \
    bananas-modprobe \
"

# Cloud sync runtime: ships /usr/bin/rclone (vendored prebuilt armv7
# binary). Pulled by the helper's RunCloudSync command when the operator
# clicks "Run now" in the Cloud tab.
IMAGE_INSTALL += " \
    bananas-rclone \
"

# LCD dashboard (Slint app on /dev/fb0 via DRM/KMS). Pulls bananas-stats
# + the runtime libs (fontconfig + udev + xkbcommon + libinput) in as
# RDEPENDS via the recipe. Software renderer only — no Mali GPU path,
# no x11/wayland, so no libgbm/libdrm in the runtime graph.
IMAGE_INSTALL += " \
    bananas-dashboard \
"
