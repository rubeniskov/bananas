#!/bin/sh
# BanaNAS welcome banner — sourced by /etc/profile from /etc/profile.d/.
# Modeled on Cockpit's "Web console" motd: ASCII art + the live admin URL
# resolved from the current hostname / first non-loopback IPv4.

# Only run for interactive logins. Skip rsync/scp/sftp/non-tty sessions.
case $- in
    *i*) ;;
    *) return 2>/dev/null || exit 0 ;;
esac
[ -t 1 ] || { return 2>/dev/null || exit 0; }

# ANSI: yellow for the banana art, cyan for the URL line.
_b_yellow=$(printf '\033[33m')
_b_cyan=$(printf '\033[36m')
_b_reset=$(printf '\033[0m')

printf '%s' "$_b_yellow"
cat <<'BANANAS_BANNER'

 ███████████                                 ██████   █████   █████████    █████████
░░███░░░░░███                               ░░██████ ░░███   ███░░░░░███  ███░░░░░███
 ░███    ░███  ██████   ████████    ██████   ░███░███ ░███  ░███    ░███ ░███    ░░░
 ░██████████  ░░░░░███ ░░███░░███  ░░░░░███  ░███░░███░███  ░███████████ ░░█████████
 ░███░░░░░███  ███████  ░███ ░███   ███████  ░███ ░░██████  ░███░░░░░███  ░░░░░░░░███
 ░███    ░███ ███░░███  ░███ ░███  ███░░███  ░███  ░░█████  ░███    ░███  ███    ░███
 ███████████ ░░████████ ████ █████░░████████ █████  ░░█████ █████   █████░░█████████
░░░░░░░░░░░   ░░░░░░░░ ░░░░ ░░░░░  ░░░░░░░░ ░░░░░    ░░░░░ ░░░░░   ░░░░░  ░░░░░░░░░

BANANAS_BANNER
printf '%s' "$_b_reset"

_b_host=$(hostname 2>/dev/null || echo banana)
_b_ip=$(ip -4 -o addr show scope global 2>/dev/null | awk '{print $4}' | cut -d/ -f1 | head -1)

if [ -n "$_b_ip" ]; then
    printf 'Web console: %shttp://%s:8080/%s  ·  %shttp://%s:8080/%s\n' \
        "$_b_cyan" "$_b_host" "$_b_reset" \
        "$_b_cyan" "$_b_ip" "$_b_reset"
else
    printf 'Web console: %shttp://%s:8080/%s\n' \
        "$_b_cyan" "$_b_host" "$_b_reset"
fi
echo

unset _b_yellow _b_cyan _b_reset _b_host _b_ip
