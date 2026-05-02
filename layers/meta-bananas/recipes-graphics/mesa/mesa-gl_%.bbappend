# meta-sunxi's mesa-gl_%.bbappend hard-codes
#   PACKAGECONFIG:class-target = "opengl x11 gallium"
# which forces a virtual/libx11 dep into the build even though the
# bananas distro doesn't ship x11 (DISTRO_FEATURES:remove = "x11 …").
#
# We can't override that line with another `=` assignment because
# meta-sunxi has higher priority (10) than meta-bananas (8) — the later
# assignment wins. Use `:remove` to strip x11 from the merged flag
# list, then `:append` to add the bits Slint's `linuxkms-femtovg`
# backend actually needs:
#   * `egl`   → libEGL.so (without this Slint logs "Error creating
#               EGL display: not found" and crash-loops — exactly
#               what we observed before this fix landed)
#   * `gles`  → libGLESv2.so (femtovg's GL backend uses GLES2)
#   * `gbm`   → libgbm.so (GBM buffer allocation between CPU + GPU;
#               linuxkms-noseat reaches the panel through it)
# `gallium` is already in meta-sunxi's defaults, which builds the lima
# userspace Gallium driver for the Mali-400 alongside the rest of the
# Gallium stack. Without `gallium` the GLES2 paint path falls back to
# llvmpipe (CPU softpipe) and we lose the perf win that justified
# enabling femtovg in the first place.
PACKAGECONFIG:remove:class-target = "x11"
PACKAGECONFIG:append:class-target = " egl gles gbm"
