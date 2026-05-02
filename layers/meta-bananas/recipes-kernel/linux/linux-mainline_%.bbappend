FILESEXTRAPATHS:prepend := "${THISDIR}/files:"

SRC_URI:append:bananas-bpi = " \
    file://0001-bananapro-cpu-clock.patch \
    file://0002-bananapro-lcd-panel.patch \
    file://drm-sun4i.cfg \
    file://nfsd.cfg \
    file://temp-sensors.cfg \
"
