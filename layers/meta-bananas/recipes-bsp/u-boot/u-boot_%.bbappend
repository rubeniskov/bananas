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

# Both splash bitmaps live in files/ — splash.bmp is the white-bg
# original, splash-black.bmp is the black-bg variant generated from
# assets/splashscreen.png. We ship the black variant by default
# because it looks cleaner against the LCD's natural off-state and
# avoids a bright flash when the panel comes on. To switch back to
# the white version, swap which file the do_deploy hook installs.
SRC_URI:append:bananapro = " \
    file://bananapro-splash.cfg \
    file://splash.bmp \
    file://splash-black.bmp \
"

# The kconfig fragment is automatically merged by meta-sunxi's
# u-boot.inc via cml1 / kconfig support — files matching `*.cfg` in
# SRC_URI are picked up by the kernel-style merge_config flow. No
# explicit do_configure hook needed for our case.

# Ship splash-black.bmp as the on-card splash. The deployed filename
# stays `splash.bmp` so the U-Boot env (`splashfile=splash.bmp`)
# doesn't have to be re-set on the running board.
do_deploy:append:bananapro() {
    install -m 0644 ${WORKDIR}/splash-black.bmp ${DEPLOYDIR}/splash.bmp
}
