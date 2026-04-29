# Override meta-poky's /etc/motd ("WARNING: Poky is a reference Yocto Project
# distribution…") with an empty file. The BanaNAS welcome banner lives in
# /etc/profile.d/bananas-motd.sh (installed by bananas-server.bb), so the
# file-level motd does not need to print anything itself.
#
# FILESEXTRAPATHS:prepend wins the lookup race against
# meta-poky/recipes-core/base-files/files/poky/motd because OE searches
# layer paths in the order they appear in BBFILES — meta-bananas's higher
# priority (8 vs core's 5) puts us first.
FILESEXTRAPATHS:prepend := "${THISDIR}/files:"
