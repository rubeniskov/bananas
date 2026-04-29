# BanaNAS U-Boot bbappend — splash screen support.
#
# Wires our config fragment + the splash bitmap into the meta-sunxi
# u-boot recipe (priority 10). Splash bitmap is staged into
# DEPLOYDIR alongside u-boot-sunxi-with-spl.bin / boot.scr; the
# bananapro machine config picks it up via `IMAGE_BOOT_FILES +=
# splash.bmp` so the wic image lands it on the FAT boot partition
# at /splash.bmp.
#
# To activate the splash on a running board (one-time):
#
#   setenv splashimage 0x46000000      # any DRAM address above the FB
#   setenv splashsource mmc_fs
#   setenv splashfile splash.bmp
#   setenv splashpos m,m
#   saveenv
#
# Future first-boot images can avoid this by having a `boot.scr` set
# the env up before `bootcmd_net`; see notes in CLAUDE.md's
# "Iteration loop" section if/when we move netboot defaults into a
# proper boot.scr.

FILESEXTRAPATHS:prepend:bananapro := "${THISDIR}/files:"

SRC_URI:append:bananapro = " \
    file://bananapro-splash.cfg \
    file://splash.bmp \
"

# The kconfig fragment is automatically merged by meta-sunxi's
# u-boot.inc via cml1 / kconfig support — files matching `*.cfg` in
# SRC_URI are picked up by the kernel-style merge_config flow. No
# explicit do_configure hook needed for our case.

# Ship splash.bmp into DEPLOYDIR so the bananapro IMAGE_BOOT_FILES
# entry can include it on the FAT boot partition.
do_deploy:append:bananapro() {
    install -m 0644 ${WORKDIR}/splash.bmp ${DEPLOYDIR}/splash.bmp
}
