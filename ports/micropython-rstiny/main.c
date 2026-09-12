/*
 * main.c — MicroPython for RSTiny entry logic.
 *
 * Startup sequence (docs/micropython-port.md §6):
 *   _start -> rstiny_start (parameter page, rstinyhal.c) -> port_main
 *     -> gc_init -> mp_init
 *     -> argc > 0 ? run script from fs (argv[0]) : friendly REPL
 *     -> mp_deinit -> EXIT via control endpoint
 *
 * The REPL is pyexec_friendly_repl(): it only needs mp_hal_stdout_tx_strn
 * and mp_hal_stdin_rx_chr (mphalport.h). Ctrl-D ends the REPL, the process
 * exits, and mysh regains the prompt.
 */

#include <stdint.h>
#include <stddef.h>
#include <string.h>

#include "py/compile.h"
#include "py/gc.h"
#include "py/runtime.h"
#include "py/repl.h"
#include "shared/runtime/pyexec.h"

#include "mphalport.h"

extern int g_argc;
extern char *g_argv[];
extern void free(void *ptr);
void gc_set_stack_top(void *top);
char *fs_read_file(const char *name, size_t *out_size);
void rstiny_deinit_exit(int code);



#if MICROPY_ENABLE_GC
static char heap[MICROPY_HEAP_SIZE];
#endif

/* Execute `src` with the same machinery as a file import. */
static void do_str(const char *src, mp_parse_input_kind_t input_kind) {
    nlr_buf_t nlr;
    if (nlr_push(&nlr) == 0) {
        mp_lexer_t *lex = mp_lexer_new_from_str_len(MP_QSTR__lt_stdin_gt_, src, strlen(src), 0);
        qstr source_name = lex->source_name;
        mp_parse_tree_t parse_tree = mp_parse(lex, input_kind);
        mp_obj_t module_fun = mp_compile(&parse_tree, source_name, true);
        mp_call_function_0(module_fun);
        nlr_pop();
    } else {
        /* uncaught exception */
        mp_obj_print_exception(&mp_plat_print, (mp_obj_t)nlr.ret_val);
    }
}

static int try_name(const char *candidate, mp_parse_input_kind_t kind) {
    size_t size = 0;
    char *src = fs_read_file(candidate, &size);
    if (src == NULL) {
        return -1;
    }
    do_str(src, kind);
    free(src);
    return 0;
}

/* `./python app` should find APP.PY on the FAT volume: try the name as
 * given, then with ".py", then the upper-cased 8.3 form "APP.PY". */
static int run_script(void) {
    if (try_name(g_argv[0], MP_PARSE_FILE_INPUT) == 0) {
        return 0;
    }
    char with_ext[24];
    size_t m = 0;
    while (g_argv[0][m] && m < sizeof(with_ext) - 4) {
        with_ext[m] = g_argv[0][m];
        m++;
    }
    with_ext[m] = '.';
    with_ext[m + 1] = 'p';
    with_ext[m + 2] = 'y';
    with_ext[m + 3] = '\0';
    if (try_name(with_ext, MP_PARSE_FILE_INPUT) == 0) {
        return 0;
    }
    char upper[24];
    size_t n = 0;
    while (g_argv[0][n] && n < sizeof(upper) - 4) {
        char c = g_argv[0][n];
        upper[n] = (c >= 'a' && c <= 'z') ? (char)(c - 32) : c;
        n++;
    }
    upper[n] = '.';
    upper[n + 1] = 'P';
    upper[n + 2] = 'Y';
    upper[n + 3] = '\0';
    if (try_name(upper, MP_PARSE_FILE_INPUT) == 0) {
        return 0;
    }
    mp_printf(&mp_plat_print, "Traceback: cannot open '%s' from fs\n", g_argv[0]);
    return 1;
}

void port_main(void) {
    int stack_dummy;
    gc_set_stack_top(&stack_dummy);

    #if MICROPY_ENABLE_GC
    gc_init(heap, heap + sizeof(heap));
    #endif
    mp_init();

    int code = 0;
    #if MICROPY_ENABLE_COMPILER
    if (g_argc > 0) {
        code = run_script();
    } else {
        pyexec_friendly_repl();
    }
    #else
    mp_printf(&mp_plat_print, "no compiler: precompiled scripts only\n");
    code = 1;
    #endif
    mp_deinit();
    rstiny_deinit_exit(code);
    /* not reached */
}