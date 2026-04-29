FILESEXTRAPATHS:prepend := "${THISDIR}/files:"

# Override poky's "Yocto Project" splash with the BanaNAS logo. The
# `outsuffix=poky` tail is mandatory: psplash_git.bb's do_compile passes
# "$outsuffix" to make-image-header.sh, which writes
# `psplash-poky-img.h` — the name psplash.c #includes. Renaming the
# suffix would mean patching psplash itself, so we keep it as `poky`
# and just substitute the PNG content.
SPLASH_IMAGES = "file://psplash-bananas-img.png;outsuffix=poky"

# Custom progress-bar + background colors. Yocto-default is a cream
# background with grey bar; the bananas image has a tropical
# yellow/teal palette so we match it.
SRC_URI += "file://0001-bananas-colors.patch"
