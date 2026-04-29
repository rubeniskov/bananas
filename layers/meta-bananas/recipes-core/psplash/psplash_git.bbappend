FILESEXTRAPATHS:prepend := "${THISDIR}/files:"

# Override poky's "Yocto Project" splash with the BanaNAS logo.
#
# `outsuffix=default` matters: psplash_git.bb special-cases the suffix
# `default` and auto-RDEPENDS the main `psplash` package on the
# generated `psplash-default` sub-package. Without that, a rootfs
# pulled in via `IMAGE_FEATURES = "splash"` (which adds `psplash`)
# installs the systemd units + helper binaries but NOT the actual
# `/usr/bin/psplash` executable, and the splash silently never
# starts (psplash-start.service has
# `ConditionFileIsExecutable=/usr/bin/psplash` so it skips quietly).
#
# Note: the C header that psplash.c #includes is always called
# `psplash-poky-img.h` regardless of this suffix — do_compile copies
# our generated header into that filename. So we don't need to keep
# the suffix as `poky` to satisfy the include.
SPLASH_IMAGES = "file://psplash-bananas-img.png;outsuffix=default"

# Custom progress-bar + background colors. Yocto-default is a cream
# background with grey bar; the bananas image has a tropical
# yellow/teal palette so we match it.
SRC_URI += "file://0001-bananas-colors.patch"
