/* Included before every C file of a wasm32-unknown-unknown build. The
 * libc tree-sitter ships for that target leaves out a few things some
 * grammars' scanners use. */
#ifndef DIFFR_WASM_COMPAT_H
#define DIFFR_WASM_COMPAT_H
#ifndef __cplusplus
/* tree-sitter-cpp, tree-sitter-xml */
typedef __WCHAR_TYPE__ wchar_t;
#ifndef static_assert
#define static_assert _Static_assert
#endif
#endif
/* tree-sitter-bash */
static inline int diffr_isdigit(int c) { return c >= '0' && c <= '9'; }
#define isdigit diffr_isdigit
/* The vendored scanners: a scanner that gives up traps. */
static inline __attribute__((noreturn)) void diffr_exit(int status) {
    (void)status;
    __builtin_trap();
}
#define exit diffr_exit
#endif
