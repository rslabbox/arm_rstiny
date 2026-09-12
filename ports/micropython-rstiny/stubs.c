/*
 * stubs.c — platform-wide callbacks the py core requires from a freestanding
 * port. Script sourcing goes through fs_read_file (main.c), so the from-file
 * lexer and import hook only need to exist; external import is disabled.
 */

#include <stdint.h>
#include <stddef.h>

#include "py/mpconfig.h"
#include "py/lexer.h"
#include "py/builtin.h"
#include "py/obj.h"
#include "py/mperrno.h"
#include "py/runtime.h"

void nlr_jump_fail(void *val) {
    (void)val;
    for (;;) {
    }
}

void NORETURN __fatal_error(const char *msg) {
    (void)msg;
    for (;;) {
    }
}

#ifndef NDEBUG
void MP_WEAK __assert_func(const char *file, int line, const char *func, const char *expr) {
    (void)file;
    (void)line;
    (void)func;
    (void)expr;
    __fatal_error("assert failed");
}
#endif

mp_lexer_t *mp_lexer_new_from_file(qstr filename) {
    (void)filename;
    mp_raise_OSError(MP_ENOENT);
}

mp_import_stat_t mp_import_stat(const char *path) {
    (void)path;
    return MP_IMPORT_STAT_NO_EXIST;
}