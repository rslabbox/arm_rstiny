/*
 * rstinyhal.c — RSTiny service bindings for the MicroPython port.
 *
 * Spans the platform glue the interpreter needs:
 *   - parameter page (SpawnInfo + ArgvBlock) parsing, decision H;
 *   - seL4 user-syscall ABI: fixed-register `svc #0` (x7 syscall number);
 *   - console WRITE/READ endpoint IPC (docs/sel4-abi.md) with IPC-buffer
 *     payload for long writes;
 *   - kernel runtime extension calls (Sleep 0x1006 / Clock 0x1008) against
 *     the fixed INIT_RUNTIME slot 5;
 *   - fs client (BIND/OPEN/READ/CLOSE) for `./python app.py` (决策 I);
 *   - stack-scanning gc_collect (no register roots, as in ports/minimal).
 *
 * Freestanding: no libc; helpers are hand-rolled or come from
 * shared/libc/string0.c.
 */

#include <stdint.h>
#include <stddef.h>

#include "py/mpconfig.h"
#include "py/runtime.h"
#include "py/gc.h"
#include "mphalport.h"

/* ---- wire layout (rstiny_protocol, docs/sel4-abi.md) ---------------------- */

#define SPAWN_MAGIC   0x0000525354494e49ULL
#define SPAWN_VERSION 2ULL
#define INFO_CONSOLE_EP 0
#define INFO_DEP_BASE 5

#define CONTROL_EXIT 0x202ULL

#define CONSOLE_WRITE 0x101ULL
#define CONSOLE_READ 0x102ULL

#define FS_PROTOCOL_VERSION 1ULL
#define FS_BIND 0x500ULL
#define FS_OPEN 0x501ULL
#define FS_READ 0x502ULL
#define FS_CLOSE 0x503ULL
#define STATUS_OK 0ULL

#define ARGV_MAGIC 0x4152564100000002ULL
#define INIT_CNODE 2
#define CSLOT_DEPTH 64
#define FS_RECV_SLOT 60  /* landing slot for the BIND buffer grant */
#define MAX_ARGV 8
#define MAX_SCRIPT_BYTES (64 * 1024)

#define SYS_CALL 0xFFFFFFFFFFFFFFFFULL /* -1: Call */
#define CAP_INIT_RUNTIME 5ULL

struct spawn_info {
    uint64_t magic;
    uint64_t version;
    uint64_t control_ep;
    uint64_t command_ep;
    uint64_t untyped;
    uint64_t rom_start;
    uint64_t rom_count;
    uint64_t extra[12];
};
struct argv_block {
    uint64_t magic;
    uint64_t argc;
    uint64_t total;
};

static uint64_t g_console_ep;
static uint64_t g_fs_ep;
static uint64_t g_control_ep;
int g_argc;
char *g_argv[MAX_ARGV];

/* ---- C ABI from rstiny-alloc (interpreter-app.md 决策 B) ------------------ */
extern void *malloc(size_t size);
extern void *realloc(void *ptr, size_t size);
extern void free(void *ptr);

/* ---- syscalls ------------------------------------------------------------- */

/* invoke with capability transfer: `caps` are written into the IPC buffer
 * caps area (offset 976) and `ncap` is encoded in the MessageInfo (bits 7-8,
 * docs/sel4-abi.md). */
static uint64_t ipc_buf(void);

static void invoke_ex(uint64_t cap, uint64_t label, const uint64_t mr[4],
                      unsigned word_len, const uint64_t caps[], unsigned ncap,
                      uint64_t *reply_label, uint64_t reply[4]) {
    if (ncap > 0) {
        uint64_t base = ipc_buf() + 976;
        for (unsigned i = 0; i < ncap; i++) {
            *(volatile uint64_t *)(base + i * 8) = caps[i];
        }
    }
    /* Fixed-register ABI: x7 carries the syscall number; generic "r"
     * constraints let the compiler pick another register and the kernel
     * faults the call as unknown. */
    register uint64_t r0 __asm__("x0") = cap;
    register uint64_t r1 __asm__("x1") = (label << 12) | ((uint64_t)ncap << 7) | word_len;
    register uint64_t r2 __asm__("x2") = mr[0];
    register uint64_t r3 __asm__("x3") = mr[1];
    register uint64_t r4 __asm__("x4") = mr[2];
    register uint64_t r5 __asm__("x5") = mr[3];
    register uint64_t r7 __asm__("x7") = SYS_CALL;
    __asm__ volatile("svc #0"
                     : "+r"(r0), "+r"(r1), "+r"(r2), "+r"(r3), "+r"(r4),
                       "+r"(r5)
                     : "r"(r7)
                     : "memory");
    /* kernel reply layout: x0 = badge, x1 = MessageInfo, x2..x5 = words. */
    *reply_label = r1 >> 12;
    reply[0] = r2;
    reply[1] = r3;
    reply[2] = r4;
    reply[3] = r5;
}

static void invoke(uint64_t cap, uint64_t label, const uint64_t mr[4],
                   unsigned word_len, uint64_t *reply_label, uint64_t reply[4]) {
    invoke_ex(cap, label, mr, word_len, NULL, 0, reply_label, reply);
}

static uint64_t ipc_buf(void) {
    uint64_t addr;
    __asm__ volatile("mrs %0, tpidrro_el0" : "=r"(addr));
    return addr;
}

/* Publish the receive landing slot at IPC buffer offset 1000. */
static void set_receive_spec(uint64_t index, uint64_t depth) {
    volatile uint64_t *spec = (volatile uint64_t *)(ipc_buf() + 1000);
    spec[0] = INIT_CNODE;
    spec[1] = index;
    spec[2] = depth;
}

/* ---- tiny string helpers (no libc) ---------------------------------------- */

static size_t str_len(const char *s) {
    size_t n = 0;
    while (s[n]) n++;
    return n;
}

/* ---- console output -------------------------------------------------------- */

#define CONSOLE_MAX_WRITE 112 /* protocol MAX_WRITE = 14 registers * 8 */

mp_uint_t mp_hal_stdout_tx_strn(const char *text, size_t n) {
    while (n > 0) {
        size_t chunk = n < CONSOLE_MAX_WRITE ? n : CONSOLE_MAX_WRITE;
        uint64_t words[15] = {0}; /* mr0 = length, mr1.. = payload */
        words[0] = chunk;
        for (size_t i = 0; i < chunk; i++) {
            words[1 + i / 8] |= (uint64_t)(uint8_t)text[i] << (8 * (i % 8));
        }
        unsigned wlen = 1 + (unsigned)((chunk + 7) / 8);
        if (wlen > 4) {
            /* Payload beyond mr3 travels through the IPC buffer. */
            uint64_t buf = ipc_buf();
            for (unsigned i = 4; i < wlen; i++) {
                *(volatile uint64_t *)(buf + 8 + i * 8) = words[i];
            }
        }
        uint64_t reply[4], reply_label;
        invoke(g_console_ep, CONSOLE_WRITE, words, wlen, &reply_label, reply);
        (void)reply_label;
        (void)reply;
        text += chunk;
        n -= chunk;
    }
    return n;
}

/* ---- console input (polling; no RX interrupt yet) -------------------------- */

int mp_hal_stdin_rx_chr(void) {
    uint64_t reply[4], reply_label;
    uint64_t mr[4] = {0, 0, 0, 0};
    invoke(g_console_ep, CONSOLE_READ, mr, 0, &reply_label, reply);
    (void)reply_label;
    if (reply[0] != 0) {
        return (int)reply[1]; /* a byte was consumed */
    }
    mp_hal_delay_ms(5);
    return mp_hal_stdin_rx_chr();
}

/* ---- kernel runtime extension: Clock / Sleep ------------------------------- */

mp_uint_t mp_hal_ticks_ms(void) {
    uint64_t reply[4], reply_label;
    uint64_t mr[4] = {0, 0, 0, 0};
    invoke(CAP_INIT_RUNTIME, 0x1008 /* Clock */, mr, 0, &reply_label, reply);
    (void)reply_label;
    return (mp_uint_t)reply[0];
}

void mp_hal_delay_ms(mp_uint_t ms) {
    uint64_t reply[4], reply_label;
    uint64_t mr[4] = {ms, 0, 0, 0};
    invoke(CAP_INIT_RUNTIME, 0x1006 /* Sleep */, mr, 1, &reply_label, reply);
    (void)reply_label;
    (void)reply;
}

/* ---- fs client (only granted to arg-taking programs, 决策 I) --------------- */

/* The OPEN wire format packs the file name as consecutive bytes from MR1
 * (docs/disk-driver.md §8.2): mr0 = length, then the raw bytes of the FAT
 * directory string; no 8.3 separator logic is involved. */
static unsigned pack_name_bytes(const char *name, size_t len, uint64_t words[2]) {
    words[0] = 0;
    words[1] = 0;
    for (size_t i = 0; i < len; i++) {
        words[i / 8] |= (uint64_t)(uint8_t)name[i] << (8 * (i % 8));
    }
    return 1 + (unsigned)((len + 7) / 8);
}

#define FS_BUF_VA 0x00300000ULL /* fs shared buffer VA (2MiB block 1, its L2 table already exists from the ELF mapping) */
#define ARM_PAGE_MAP 40ULL
#define VM_CACHEABLE 1ULL
#define RIGHTS_READ 2ULL
#define VM_VA 3ULL

/* Read `name` from the fs into a malloc'd buffer (<= 64 KiB).
 * Returns NULL on any failure; *out_size receives the byte count. */
char *fs_read_file(const char *name, size_t *out_size) {
    uint64_t reply[4], reply_label;
    static int bound = 0;

    if (!bound) {
        set_receive_spec(FS_RECV_SLOT, CSLOT_DEPTH);
        uint64_t bind_mr[4] = {FS_PROTOCOL_VERSION, 0, 0, 0};
        invoke(g_fs_ep, FS_BIND, bind_mr, 1, &reply_label, reply);
        if (reply_label != STATUS_OK || reply[0] != FS_PROTOCOL_VERSION) {
            return NULL;
        }
        bound = 1;
        /* The BIND reply transferred the shared frame into slot 60; map it
         * at FS_BUF_VA so READ payloads land in our address space. */
        uint64_t map_args[4] = {FS_BUF_VA, RIGHTS_READ, VM_CACHEABLE, 0};
        uint64_t vspace_cap = VM_VA;
        invoke_ex(FS_RECV_SLOT, ARM_PAGE_MAP, map_args, 3, &vspace_cap, 1,
                  &reply_label, reply);
        if (reply_label != STATUS_OK) {
            return NULL;
        }
    }

    size_t len = str_len(name);
    uint64_t name_words[2];
    unsigned wlen = pack_name_bytes(name, len, name_words);
    uint64_t open_mr[4] = {len, name_words[0], name_words[1], 0};
    invoke(g_fs_ep, FS_OPEN, open_mr, wlen, &reply_label, reply);
    if (reply_label != STATUS_OK) {
        return NULL;
    }
    uint64_t file_id = reply[0];
    uint64_t size = reply[1];
    if (size > MAX_SCRIPT_BYTES) {
        uint64_t close_mr[4] = {file_id, 0, 0, 0};
        invoke(g_fs_ep, FS_CLOSE, close_mr, 1, &reply_label, reply);
        return NULL;
    }

    char *buf = (char *)malloc((size_t)(size + 1));
    if (buf == NULL) {
        uint64_t close_mr[4] = {file_id, 0, 0, 0};
        invoke(g_fs_ep, FS_CLOSE, close_mr, 1, &reply_label, reply);
        return NULL;
    }
    uint64_t offset = 0;
    while (offset < size) {
        uint64_t length = size - offset;
        if (length > 0x1000) length = 0x1000;
        uint64_t read_mr[4] = {file_id, offset, length, 0};
        invoke(g_fs_ep, FS_READ, read_mr, 3, &reply_label, reply);
        if (reply_label != STATUS_OK) {
            free(buf);
            uint64_t close_mr[4] = {file_id, 0, 0, 0};
            invoke(g_fs_ep, FS_CLOSE, close_mr, 1, &reply_label, reply);
            return NULL;
        }
        uint64_t got = reply[0]; /* fs READ replies [count] (docs/disk-driver.md §8.2) */
        if (got == 0) break;
        /* READ payload lands in the fs-shared buffer mapped at FS_BUF_VA
         * (the BIND grant), not in the IPC buffer (docs/disk-driver.md §8.2). */
        volatile uint8_t *payload = (volatile uint8_t *)FS_BUF_VA;
        for (uint64_t i = 0; i < got; i++) {
            buf[offset + i] = payload[i];
        }
        offset += got;
    }

    uint64_t close_mr[4] = {file_id, 0, 0, 0};
    invoke(g_fs_ep, FS_CLOSE, close_mr, 1, &reply_label, reply);
    (void)reply_label;

    buf[offset] = '\0';
    *out_size = (size_t)offset;
    return buf;
}

/* ---- startup: parameter page ---------------------------------------------- */

void port_main(void);

void rstiny_start(uint64_t info_va) {
    const struct spawn_info *info = (const struct spawn_info *)info_va;
    if (info->magic != SPAWN_MAGIC || info->version != SPAWN_VERSION) {
        for (;;) { /* parameter page not what the loader promised */
        }
    }
    g_console_ep = info->extra[INFO_CONSOLE_EP];
    g_fs_ep = info->extra[INFO_DEP_BASE];
    g_control_ep = info->control_ep;
    /* Parse the optional ArgvBlock (decision H); zero gap = no arguments. */
    const struct argv_block *ab =
        (const struct argv_block *)((const uint8_t *)info + sizeof(struct spawn_info));
    g_argc = 0;
    if (ab->magic == ARGV_MAGIC && (int64_t)ab->argc > 0) {
        int count = (int)ab->argc;
        if (count > MAX_ARGV) count = MAX_ARGV;
        const char *cursor = (const char *)(ab + 1);
        for (int i = 0; i < count; i++) {
            g_argv[i] = (char *)cursor;
            while (*cursor) cursor++;
            cursor++; /* NUL */
        }
        g_argc = count;
    }
    port_main();
}

/* Exit through the control endpoint; the supervisor reaps this task. */
void rstiny_deinit_exit(int code) {
    uint64_t reply[4], reply_label;
    uint64_t mr[4] = {(uint64_t)code, 0, 0, 0};
    invoke(g_control_ep, CONTROL_EXIT, mr, 1, &reply_label, reply);
    (void)reply_label;
    (void)reply;
    for (;;) { /* supervisor tears this task down */
    }
}

/* ---- GC ------------------------------------------------------------------- */

static char *stack_top;
void *gc_heap;
size_t gc_heap_size;

void gc_collect(void) {
    /* No register roots: only scan the C stack (same caveat as minimal). */
    void *dummy;
    gc_collect_start();
    gc_collect_root(&dummy, ((mp_uint_t)stack_top - (mp_uint_t)&dummy) / sizeof(mp_uint_t));
    gc_collect_end();
}

void gc_set_stack_top(void *top) {
    stack_top = (char *)top;
}