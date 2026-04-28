# Fix compilation errors with GCC 14+ on Arch Linux
do_compile:prepend:class-native() {
    # Fix discarded const qualifier errors
    sed -i 's/sep = memchr(fixup_str/sep = (char *)memchr(fixup_str/g' ${S}/libfdt/fdt_overlay.c
    sed -i 's/sep = memchr(name/sep = (char *)memchr(name/g' ${S}/libfdt/fdt_overlay.c
}

# Also try to disable Werror just in case
EXTRA_OEMESON:append:class-native = " -Dwerror=false"
