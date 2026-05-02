# BanaNAS Raspberry Pi boot dispatcher.
#
# RPi's U-Boot exposes ${board_name} populated from the firmware's revision
# code (see https://www.raspberrypi.com/documentation/computers/raspberry-pi.html#raspberry-pi-revision-codes).
# We match it to the upstream broadcom/*.dtb file shipped by linux-raspberrypi
# and load the matching device tree before booti. If the board reports an
# unknown model (e.g. a hardware revision newer than this image's kernel
# supports), we fall back to the RPi 4 DTB — the closest match for the
# BCM2711-and-up family — and surface a warning on the console.

if test "${board_name}" = "3 Model B" ; then
    setenv fdtfile broadcom/bcm2837-rpi-3-b.dtb
elif test "${board_name}" = "3 Model B Plus" ; then
    setenv fdtfile broadcom/bcm2837-rpi-3-b-plus.dtb
elif test "${board_name}" = "4 Model B" ; then
    setenv fdtfile broadcom/bcm2711-rpi-4-b.dtb
elif test "${board_name}" = "5 Model B" ; then
    setenv fdtfile broadcom/bcm2712-rpi-5-b.dtb
else
    echo "[bananas] unknown RPi board_name='${board_name}' — falling back to RPi 4 DTB"
    setenv fdtfile broadcom/bcm2711-rpi-4-b.dtb
fi

# Common kernel cmdline. /dev/mmcblk0p2 is the rpi-sdimg layout's rootfs.
# rootwait covers the SD enumeration race; firstboot-resize grows the
# partition on first boot.
setenv bootargs "console=${console} root=/dev/mmcblk0p2 rootwait rw"

fatload mmc 0:1 ${kernel_addr_r} ${kernel_image}
fatload mmc 0:1 ${fdt_addr_r} ${fdtfile}
booti ${kernel_addr_r} - ${fdt_addr_r}
