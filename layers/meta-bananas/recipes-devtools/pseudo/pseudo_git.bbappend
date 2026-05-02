do_compile:prepend:class-native() {
    # Fix openat2 declaration in ALL generated and source files
    find ${S} -name "*.c" -o -name "*.h" -o -name "*.in" | xargs sed -i "s/struct open_how \*how/const struct open_how \*how/g"
    if [ -f pseudo_wrapfuncs.c ]; then
        sed -i "s/struct open_how \*how/const struct open_how \*how/g" pseudo_wrapfuncs.c
    fi
}
