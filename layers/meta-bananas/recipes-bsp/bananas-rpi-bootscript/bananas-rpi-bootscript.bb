SUMMARY = "U-Boot dispatcher script that auto-detects the RPi model and picks the matching DTB"
DESCRIPTION = "Compiles boot.cmd into boot.scr (mkimage'd legacy-format script). RPi's U-Boot \
loads boot.scr from the FAT firmware partition at boot, evaluates it, reads \\${board_name}, \
sets fdtfile to the DTB matching the running board (RPi 3 / 4 / 5), and booti's the kernel \
with the right device tree. Same Raspbian-style auto-detect across the RPi family."

LICENSE = "MIT"
LIC_FILES_CHKSUM = "file://${COMMON_LICENSE_DIR}/MIT;md5=0835ade698e0bcf8506ecda2f7b4f302"

SRC_URI = "file://boot.cmd"

# u-boot-mkimage-native provides the mkimage tool used below.
DEPENDS = "u-boot-tools-native"

inherit deploy nopackages

S = "${WORKDIR}"

do_compile() {
    mkimage -A arm64 -O linux -T script -C none -a 0 -e 0 \
        -n "BanaNAS RPi auto-detect" \
        -d ${WORKDIR}/boot.cmd ${B}/boot.scr
}

do_deploy() {
    install -d ${DEPLOYDIR}
    install -m 0644 ${B}/boot.scr ${DEPLOYDIR}/boot.scr
}

addtask deploy after do_compile before do_build

# Only the bananas-rpi MACHINE pulls in this recipe. Banana Pro uses its
# own sunxi U-Boot env (saveenv'd at first flash) and never reads boot.scr.
COMPATIBLE_MACHINE = "bananas-rpi"
