#!/bin/bash
# Container entrypoint for the bananas-devtool harness.
#
# Brings up engine (root) + router + webadmin + every plugin daemon
# inside this container, with /etc/shadow populated so /api/login
# works against username=root, password=bananas-test (hash baked at
# image-build time so the container is reproducible).

set -euo pipefail

# Same hash bananas-devtool's tmpdir harness uses ($6$ SHA-512 of
# "bananas-test"). Computed once with `mkpasswd -m sha-512 -S
# bananas bananas-test` so the value is stable across container
# rebuilds. If you rotate it here, also bump TEST_PASSWORD in
# crates/devtool/src/harness.rs.
readonly ROOT_HASH='$6$bananas$zScD/8UQfcuW/F3BT4AhO2VNv4EO7GVsl./Ihfi8qOnuBXW09WM8yf8xn4zQZ93vehe6hLJZhm70N3wRyr8u50'

# Today is "20240" days since the epoch — close enough; field 3 just
# needs to be non-zero so authenticate() doesn't surface
# `password_expired`.
echo "root:${ROOT_HASH}:20000:0:99999:7:::" > /etc/shadow.bananas
echo "nobody:!:18000:0:99999:7:::" >> /etc/shadow.bananas
chmod 0640 /etc/shadow.bananas

# Common env block — every daemon reads these. Sockets all sit
# under /run/bananas/ which the Dockerfile pre-creates with the
# right ownership.
export BANANAS_EXTENSIONS_DIR=/etc/bananas/extensions.d
export BANANAS_ROUTER_SOCKET=/run/bananas/router.sock
export BANANAS_WEBADMIN_SOCKET=/run/bananas/webadmin.sock
export BANANAS_ENGINE_SOCKET=/run/bananas/engine.sock
export BANANAS_CLOUD_SOCKET=/run/bananas/cloud.sock
export BANANAS_EXPORTS_SOCKET=/run/bananas/exports.sock
export BANANAS_STORAGE_SOCKET=/run/bananas/storage.sock
export BANANAS_USERS_SOCKET=/run/bananas/users.sock
export BANANAS_STATS_WEB_SOCKET=/run/bananas/stats-web.sock
export BANANAS_DASHBOARD_WEB_SOCKET=/run/bananas/dashboard-web.sock
export BANANAS_STATS_LIVE_SOCKET=/run/bananas/stats-live.sock
export BANANAS_SESSION_KEY=/var/lib/bananas/session.key
export BANANAS_LISTEN_ADDR=0.0.0.0:8080
export BANANAS_EXPORTS_PATH=/etc/exports
export BANANAS_SHADOW_PATH=/etc/shadow.bananas
export BANANAS_STATS_DB=/var/lib/bananas/stats.db
export BANANAS_OPERATIONS_JOURNAL=/var/lib/bananas/operations.json
export RUST_LOG=warn

touch /etc/exports

run_as_bananas() {
    # `setpriv` would be nicer than `su` but archlinux base doesn't
    # ship it. Bash login isn't required; just exec the binary.
    su -s /bin/bash -c "BANANAS_EXTENSIONS_DIR='$BANANAS_EXTENSIONS_DIR' \
        BANANAS_ROUTER_SOCKET='$BANANAS_ROUTER_SOCKET' \
        BANANAS_WEBADMIN_SOCKET='$BANANAS_WEBADMIN_SOCKET' \
        BANANAS_ENGINE_SOCKET='$BANANAS_ENGINE_SOCKET' \
        BANANAS_CLOUD_SOCKET='$BANANAS_CLOUD_SOCKET' \
        BANANAS_EXPORTS_SOCKET='$BANANAS_EXPORTS_SOCKET' \
        BANANAS_STORAGE_SOCKET='$BANANAS_STORAGE_SOCKET' \
        BANANAS_USERS_SOCKET='$BANANAS_USERS_SOCKET' \
        BANANAS_STATS_WEB_SOCKET='$BANANAS_STATS_WEB_SOCKET' \
        BANANAS_DASHBOARD_WEB_SOCKET='$BANANAS_DASHBOARD_WEB_SOCKET' \
        BANANAS_STATS_LIVE_SOCKET='$BANANAS_STATS_LIVE_SOCKET' \
        BANANAS_SESSION_KEY='$BANANAS_SESSION_KEY' \
        BANANAS_LISTEN_ADDR='$BANANAS_LISTEN_ADDR' \
        BANANAS_EXPORTS_PATH='$BANANAS_EXPORTS_PATH' \
        BANANAS_SHADOW_PATH='$BANANAS_SHADOW_PATH' \
        BANANAS_STATS_DB='$BANANAS_STATS_DB' \
        BANANAS_OPERATIONS_JOURNAL='$BANANAS_OPERATIONS_JOURNAL' \
        RUST_LOG='$RUST_LOG' \
        exec '$1'" bananas &
}

# Engine: ROOT. The whole point of the container harness — the
# privileged ops (writing /etc/exports, useradd, opkg) need real
# root to succeed.
"/usr/bin/bananas-engine" &
ENGINE_PID=$!

# Router + webadmin + plugin daemons run as the bananas user.
run_as_bananas /usr/bin/bananas-router
run_as_bananas /usr/bin/bananas-webadmin
run_as_bananas /usr/bin/bananas-cloud
run_as_bananas /usr/bin/bananas-exports
run_as_bananas /usr/bin/bananas-storage
run_as_bananas /usr/bin/bananas-users
run_as_bananas /usr/bin/bananas-stats-web
run_as_bananas /usr/bin/bananas-dashboard-web

# Reap on SIGTERM so `docker stop` doesn't have to fall back to
# SIGKILL after 10 s.
trap 'kill 0' TERM INT

# The engine's lifetime is the container's lifetime; if it dies we
# tear everything down. The other daemons restart-on-failure in
# production via systemd; in this container we just exit and let
# the test infra report a failure.
wait $ENGINE_PID
