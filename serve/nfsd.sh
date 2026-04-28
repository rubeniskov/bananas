#!/bin/sh
# Replacement for itsthenetwork/nfs-server-alpine's /usr/bin/nfsd.sh.
# The upstream script disables NFSv2 + NFSv3 with --no-nfs-version flags;
# U-Boot's NFS root and the kernel's nfsroot= use NFSv3, so we re-enable them.
set -e

# Reap children on signals so docker stop is clean
trap 'exportfs -uav; rpc.nfsd 0; exit 0' INT TERM

# rpcbind must be running before any rpc.* program registers
/sbin/rpcbind -w
sleep 0.3

# Kernel-side nfsd needs the procfs mount
mount -t nfsd nfsd /proc/fs/nfsd 2>/dev/null || true

# Start the kernel server with default version set (v3+v4) and 8 threads
rpc.nfsd 8

# Reload exports from /etc/exports (we bind-mount our own)
exportfs -arv

# Mountd in foreground; default exposes v3 and v4 to portmapper
exec rpc.mountd --debug all --foreground --no-udp
