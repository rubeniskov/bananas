SUMMARY = "Headless NAS/NFS image for Banana Pro (BPI-M1+)"
LICENSE = "MIT"

inherit core-image

IMAGE_FEATURES += "ssh-server-openssh package-management"

# Root password (SHA-512 hash). SSH keeps PermitRootLogin = prohibit-password
# (OpenSSH default), so this password unlocks serial / LCD console only — SSH
# stays key-only. ROOT_PASSWORD_HASH is required at build time (no default
# here, no leaked example in the tree); kas.yml's `env:` block reads it from
# the calling environment. Generate one with:
#   openssl passwd -6 -salt $(openssl rand -hex 8)
#
# Implementation note: extrausers/EXTRA_USERS_PARAMS is avoided here because
# its eval pass strips '$' chars from SHA-512 hashes. We patch /etc/shadow
# directly via sed in single-quoted shell, so the hash is bitbake-substituted
# but never re-evaluated by the shell.
ROOT_PASSWORD_HASH ??= ""

# Force do_rootfs to invalidate when ROOT_PASSWORD_HASH changes. Without this,
# kas-imported env vars (via the kas.yml `env:` block) don't always make it
# into bitbake's task-signature hash, and a stale cached rootfs gets reused
# when only .env changed.
do_rootfs[vardeps] += "ROOT_PASSWORD_HASH NFS_EXPORT_NETWORK"

ROOTFS_POSTPROCESS_COMMAND += "set_root_password;"

set_root_password() {
    if [ -z "${ROOT_PASSWORD_HASH}" ]; then
        bbfatal "ROOT_PASSWORD_HASH is required (set it in .env or export before pixi run build)"
    fi
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

# Bake fstab entries for the SATA storage volumes. nofail = don't drop to
# rescue mode if the drive is missing; x-systemd.device-timeout=10 = give
# the SATA controller 10 s to enumerate then fail-soft.
ROOTFS_POSTPROCESS_COMMAND += "install_storage_fstab;"

install_storage_fstab() {
    install -d -m 0755 ${IMAGE_ROOTFS}/srv/media
    install -d -m 0755 ${IMAGE_ROOTFS}/srv/services
    cat >> ${IMAGE_ROOTFS}/etc/fstab <<EOF
LABEL=media     /srv/media     ext4  defaults,noatime,nofail,x-systemd.device-timeout=10  0  2
LABEL=services  /srv/services  ext4  defaults,noatime,nofail,x-systemd.device-timeout=10  0  2
EOF
}

# NFS export the SATA storage to ${NFS_EXPORT_NETWORK} (a CIDR or hostname).
# Both volumes are exported with the same options. no_root_squash lets clients
# keep root identity (handy for rsync of mode/owner-sensitive trees); insecure
# allows source ports above 1024 which some clients use by default.
#
# NFS_EXPORT_NETWORK is required at build time (no default — passed through
# kas.yml's BB_ENV_PASSTHROUGH_ADDITIONS). Set it to your LAN CIDR, e.g.
# `NFS_EXPORT_NETWORK=192.168.1.0/24 pixi run build`.
NFS_EXPORT_NETWORK ?= ""

ROOTFS_POSTPROCESS_COMMAND += "install_nfs_exports;"

install_nfs_exports() {
    if [ -z "${NFS_EXPORT_NETWORK}" ]; then
        bbfatal "NFS_EXPORT_NETWORK is required (set it in kas.yml local_conf or export it before pixi run build)"
    fi
    # all_squash + anonuid=1000/anongid=1000: every client (root or not, any
    # UID) is remapped to UID 1000 / GID 1000 on the server. This matches the
    # ownership of pre-existing /srv/media files (UID 1000) and gives all
    # clients consistent write access to /srv/services without needing a
    # shared UID across machines. Tradeoff: file-level user attribution is
    # gone — everything is "the NAS user". For a single-user personal NAS
    # this is the simplest correct model.
    cat > ${IMAGE_ROOTFS}/etc/exports <<EOF
/srv/media     ${NFS_EXPORT_NETWORK}(rw,sync,no_subtree_check,all_squash,anonuid=1000,anongid=1000,insecure)
/srv/services  ${NFS_EXPORT_NETWORK}(rw,sync,no_subtree_check,all_squash,anonuid=1000,anongid=1000,insecure)
EOF
    install -d -m 0755 ${IMAGE_ROOTFS}/etc/systemd/system/multi-user.target.wants
    ln -sf /lib/systemd/system/nfs-server.service \
        ${IMAGE_ROOTFS}/etc/systemd/system/multi-user.target.wants/nfs-server.service

    # Drop-in: nfs-server.service's ExecStart is "rpc.nfsd $NFSD_OPTS $NFSD_COUNT",
    # both sourced from /etc/nfs-utils.conf which OE doesn't ship. Without
    # NFSD_COUNT, rpc.nfsd exits 1 and the unit fails. Also pin RequiresMountsFor
    # so the SATA volumes are mounted before exportfs runs (otherwise the export
    # paths are still part of the NFS rootfs, exportfs sees NFS-on-NFS and
    # demands fsid=).
    install -d -m 0755 ${IMAGE_ROOTFS}/etc/systemd/system/nfs-server.service.d
    cat > ${IMAGE_ROOTFS}/etc/systemd/system/nfs-server.service.d/override.conf <<EOF
[Unit]
RequiresMountsFor=/srv/media /srv/services

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
    rsync \
    curl \
    wget \
    htop \
    nano \
    vim \
    tmux \
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
