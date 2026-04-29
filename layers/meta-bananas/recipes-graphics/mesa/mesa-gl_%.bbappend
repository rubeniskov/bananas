# meta-sunxi's mesa-gl_%.bbappend hard-codes
#   PACKAGECONFIG:class-target = "opengl x11 gallium"
# which forces a virtual/libx11 dep into the build even though the
# bananas distro doesn't ship x11 (DISTRO_FEATURES:remove = "x11 …").
#
# We can't override that line with another `=` assignment because
# meta-sunxi has higher priority (10) than meta-bananas (8) — the later
# assignment wins. Use `:remove` instead, which strips the named
# whitespace-separated token from PACKAGECONFIG after both layers'
# assignments are merged. End result: PACKAGECONFIG:class-target =
# "opengl gallium" — same flags meta-sunxi wanted, minus x11.
PACKAGECONFIG:remove:class-target = "x11"
