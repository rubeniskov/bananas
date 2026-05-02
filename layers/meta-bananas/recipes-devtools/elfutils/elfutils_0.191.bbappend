do_compile:prepend:class-native() {
    sed -i "s/struct known_csrs \*found = bsearch/const struct known_csrs \*found = bsearch/g" ${S}/libcpu/riscv_disasm.c
    find ${B} -name "Makefile" -exec sed -i "s/-Werror//g" {} +
}
