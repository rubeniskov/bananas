# Test container for bananas-devtool's engine-as-root harness. Mirrors
# the production rootfs's user / dir layout, but slim — only the eight
# daemons + their config dirs.
#
# Base: archlinux. The host-built binaries are dynamically linked
# against the host's glibc (2.43+), so the container needs a glibc at
# least as new as the build host. archlinux ships rolling glibc that
# matches Arch hosts; for non-Arch developers, retarget by editing
# `FROM` to a base whose glibc >= the build host's.

FROM archlinux:base

# Bananas service user — same name as in the Yocto recipes' useradd.
# UID stays default; tests don't depend on a specific uid.
RUN pacman -Sy --noconfirm --needed glibc curl iproute2 \
 && useradd -r -m -d /home/bananas -s /usr/sbin/nologin bananas \
 && mkdir -p /run/bananas /var/lib/bananas /etc/bananas/extensions.d /etc/bananas \
 && chown -R bananas:bananas /var/lib/bananas /run/bananas \
 && chmod 0755 /run/bananas /var/lib/bananas

# Daemon binaries are copied from the host's `target/debug/`. The
# Pixi `build-test-container` task pre-builds them; the Dockerfile
# only knows how to consume them.
COPY target/debug/bananas-router /usr/bin/bananas-router
COPY target/debug/bananas-webadmin /usr/bin/bananas-webadmin
COPY target/debug/bananas-engine /usr/bin/bananas-engine
COPY target/debug/bananas-cloud /usr/bin/bananas-cloud
COPY target/debug/bananas-exports /usr/bin/bananas-exports
COPY target/debug/bananas-storage /usr/bin/bananas-storage
COPY target/debug/bananas-users /usr/bin/bananas-users
COPY target/debug/bananas-stats-web /usr/bin/bananas-stats-web
COPY target/debug/bananas-dashboard-web /usr/bin/bananas-dashboard-web
RUN chmod +x /usr/bin/bananas-*

# Extension manifests. Same shape as /etc/bananas/extensions.d/ on
# the BPI; sockets point at the well-known /run/bananas/ paths.
COPY tests/container/extensions.d/ /etc/bananas/extensions.d/

# Entrypoint: write the test /etc/shadow, spawn engine as root,
# spawn the rest as the bananas user, drop into a foreground wait
# so the container stays alive.
COPY tests/container/entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh

# webadmin's public TCP listener.
EXPOSE 8080

CMD ["/entrypoint.sh"]
