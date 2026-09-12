/* minic: C userland smoke test (interpreter-app.md 决策 F).
 *
 * Proves the four pre-dependencies of the MicroPython port end to end:
 *   1. rstiny-alloc: malloc/realloc/free with Runtime::Map heap growth,
 *   2. the parameter page ArgvBlock (argv parsing),
 *   3. the fs capability at slot 53 for arg-taking programs,
 *   4. the C cross-compilation pipeline (this ELF itself).
 *
 * Freestanding: no libc at all. All helpers are hand-rolled so the object
 * files never reference memcpy/strlen/../ and the link stays clean without
 * compiler runtime stubs beyond the Rust staticlib's builtins.
 */

typedef unsigned char u8;
typedef unsigned long long u64;
typedef long long i64;
typedef unsigned int u32;

/* ---- wire layout (rstiny_protocol, docs/sel4-abi.md) --------------------- */

#define SPAWN_MAGIC 0x0000525354494e49ULL
#define SPAWN_VERSION 2ULL
#define INFO_CONSOLE_EP 0
#define INFO_DEP_BASE 5

/* Control endpoint protocol. */
#define CONTROL_EXIT 0x202ULL

/* Console protocol: mr0 = byte count, mr1.. pack the bytes. */
#define CONSOLE_WRITE 0x101ULL

/* fs protocol (docs/disk-driver.md section 8.2). */
#define FS_PROTOCOL_VERSION 1ULL
#define FS_BIND 0x500ULL
#define FS_OPEN 0x501ULL
#define FS_CLOSE 0x503ULL
#define STATUS_OK 0ULL

#define ARGV_MAGIC 0x4152564100000002ULL
#define INIT_CNODE 2
#define FS_RECV_SLOT 60 /* landing slot for the BIND buffer grant */
#define CSLOT_DEPTH 64
#define MAX_ARGV 8

struct spawn_info {
    u64 magic;
    u64 version;
    u64 control_ep;
    u64 command_ep;
    u64 untyped;
    u64 rom_start;
    u64 rom_count;
    u64 extra[12];
};
struct argv_block {
    u64 magic;
    u64 argc;
    u64 total;
};

static u64 g_console_ep;
static u64 g_fs_ep;
static u64 g_control_ep;
static int g_argc;
static const char *g_argv[MAX_ARGV];

/* ---- C ABI from rstiny-alloc (interpreter-app.md 决策 B) ------------------ */
void *malloc(u64 size);
void *realloc(void *ptr, u64 size);
void free(void *ptr);
u64 malloc_usable_size(void *ptr);

/* ---- syscalls ------------------------------------------------------------- */

#define SYS_CALL 0xFFFFFFFFFFFFFFFFULL /* -1: Call */

static void invoke(u64 cap, u64 label, const u64 mr[4], unsigned word_len,
                   u64 *reply_label, u64 reply[4]) {
    (void)word_len;
    /* Fixed-register ABI: x7 carries the syscall number; a generic "r"
     * constraint lets the compiler pick another register and the kernel
     * faults the call as unknown.  The syscall number register is fixed
     * too, so nothing else is allocated to x7. */
    register u64 r0 __asm__("x0") = cap;
    register u64 r1 __asm__("x1") = (label << 12) | word_len;
    register u64 r2 __asm__("x2") = mr[0];
    register u64 r3 __asm__("x3") = mr[1];
    register u64 r4 __asm__("x4") = mr[2];
    register u64 r5 __asm__("x5") = mr[3];
    register u64 r7 __asm__("x7") = SYS_CALL;
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

static u64 ipc_buf(void) {
    u64 addr;
    __asm__ volatile("mrs %0, tpidrro_el0" : "=r"(addr));
    return addr;
}

/* Publish the receive landing slot at IPC buffer offset 1000. */
static void set_receive_spec(u64 index, u64 depth) {
    volatile u64 *spec = (volatile u64 *)(ipc_buf() + 1000);
    spec[0] = INIT_CNODE;
    spec[1] = index;
    spec[2] = depth;
}

/* ---- tiny string helpers (no libc) ---------------------------------------- */

static u64 str_len(const char *s) {
    u64 n = 0;
    while (s[n]) n++;
    return n;
}

static void u64_to_dec(char *out, u64 value) {
    char tmp[24];
    int n = 0;
    if (value == 0) {
        out[0] = '0';
        out[1] = 0;
        return;
    }
    while (value) {
        tmp[n++] = '0' + (char)(value % 10);
        value /= 10;
    }
    for (int i = 0; i < n; i++) out[i] = tmp[n - 1 - i];
    out[n] = 0;
}

/* ---- console output -------------------------------------------------------- */

static void console_write(const char *text) {
    u64 n = str_len(text);
    u64 words[15] = {0}; /* mr0 = length, mr1.. = payload */
    words[0] = n;
    for (u64 i = 0; i < n; i++)
        words[1 + i / 8] |= (u64)(u8)text[i] << (8 * (i % 8));
    unsigned wlen = 1 + (unsigned)((n + 7) / 8);
    if (wlen > 4) {
        /* Payload beyond mr3 travels through the IPC buffer. */
        u64 buf = ipc_buf();
        for (unsigned i = 4; i < wlen; i++) *(volatile u64 *)(buf + 8 + i * 8) = words[i];
    }
    u64 reply[4], reply_label;
    invoke(g_console_ep, CONSOLE_WRITE, words, wlen, &reply_label, reply);
    (void)reply_label;
    (void)reply;
}

static void log_line(const char *text) {
    console_write(text);
    console_write("\n");
}

static void log_u64(const char *prefix, u64 value) {
    char number[24];
    u64_to_dec(number, value);
    console_write(prefix);
    console_write(number);
    console_write("\n");
}

/* ---- allocator exercise ----------------------------------------------------- */

static int alloc_smoke(void) {
    /* Small alloc: usable size must be at least the request; write/read back. */
    u8 *p = (u8 *)malloc(64);
    if (!p || malloc_usable_size(p) < 64) return -1;
    for (u64 i = 0; i < 64; i++) p[i] = (u8)(i ^ 0xa5);
    for (u64 i = 0; i < 64; i++)
        if (p[i] != (u8)(i ^ 0xa5)) return -1;
    u8 *grown = (u8 *)realloc(p, 128);
    if (!grown) return -1;
    /* The first 64 bytes survive the move; the tail is fresh memory. */
    for (u64 i = 0; i < 64; i++)
        if (grown[i] != (u8)(i ^ 0xa5)) return -1;
    for (u64 i = 64; i < 128; i++) grown[i] = (u8)(0x80 | i);
    for (u64 i = 64; i < 128; i++)
        if (grown[i] != (u8)(0x80 | i)) return -1;
    free(grown);

    /* Collision check across a handful of small blocks. */
    u8 *block[8] = {0};
    for (int i = 0; i < 8; i++) block[i] = (u8 *)malloc(32);
    for (int i = 0; i < 8; i++) {
        if (!block[i]) return -1;
        for (u64 j = 0; j < 32; j++) block[i][j] = (u8)(i + j);
    }
    for (int i = 0; i < 8; i++)
        for (u64 j = 0; j < 32; j++)
            if (block[i][j] != (u8)(i + j)) return -1;
    for (int i = 0; i < 8; i++) free(block[i]);
    return 0;
}

static int alloc_grow(void) {
    /* 40 KiB forces Runtime::Map growth (pool is 32 KiB; page-rounded). */
    u8 *big = (u8 *)malloc(40 * 1024);
    if (!big) return -1;
    big[0] = 0xaa;
    big[40 * 1024 - 1] = 0x55;
    if (big[0] != 0xaa || big[40 * 1024 - 1] != 0x55) return -1;
    free(big);
    return 0;
}

/* ---- fs probe (only when argv present, decision I) -------------------------- */

static void fs_probe(void) {
    if (g_fs_ep == 0) {
        log_line("[minic] fs not granted");
        return;
    }
    u64 reply[4], reply_label;

    set_receive_spec(FS_RECV_SLOT, CSLOT_DEPTH);
    u64 bind_mr[4] = {FS_PROTOCOL_VERSION, 0, 0, 0};
    invoke(g_fs_ep, FS_BIND, bind_mr, 1, &reply_label, reply);
    if (reply_label != STATUS_OK || reply[0] != FS_PROTOCOL_VERSION) {
        log_line("[minic] fs bind failed");
        return;
    }
    log_line("[minic] fs bound");

    /* OPEN "APPS.CFG": len in mr0, packed 8.3 short name in mr1.
     * Little-endian: 'A' 'P' 'P' 'S' '.' 'C' 'F' 'G' -> 0x4746432E53505041. */
    u64 open_mr[4] = {8, 0x4746432E53505041ULL, 0, 0};
    invoke(g_fs_ep, FS_OPEN, open_mr, 2, &reply_label, reply);
    if (reply_label != STATUS_OK) {
        log_line("[minic] fs open failed");
        return;
    }
    log_u64("[minic] fs APPS.CFG size=", reply[1]);

    u64 close_mr[4] = {reply[0], 0, 0, 0};
    invoke(g_fs_ep, FS_CLOSE, close_mr, 1, &reply_label, reply);
    (void)reply_label;
}

/* ---- entry ---------------------------------------------------------------- */

static void main(void);

void port_main(u64 info_va) {
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
        (const struct argv_block *)((const u8 *)info + sizeof(struct spawn_info));
    if (ab->magic == ARGV_MAGIC && (i64)ab->argc > 0) {
        int count = (int)ab->argc;
        if (count > MAX_ARGV) count = MAX_ARGV;
        const char *cursor = (const char *)(ab + 1);
        for (int i = 0; i < count; i++) {
            g_argv[i] = cursor;
            while (*cursor) cursor++;
            cursor++; /* NUL */
        }
        g_argc = count;
    }
    main();
}

static void main(void) {
    log_line("[minic] ready");
    log_u64("[minic] argc=", (u64)g_argc);
    for (int i = 0; i < g_argc; i++) {
        console_write("[minic] argv[");
        char index[8];
        u64_to_dec(index, (u64)i);
        console_write(index);
        console_write("]=");
        console_write(g_argv[i]);
        console_write("\n");
    }
    if (alloc_smoke() == 0) {
        log_line("[minic] alloc ok");
    } else {
        log_line("[minic] alloc FAILED");
    }
    if (alloc_grow() == 0) {
        log_line("[minic] grow ok");
    } else {
        log_line("[minic] grow FAILED");
    }
    if (g_argc > 0) {
        fs_probe();
    }
    log_line("[minic] exit 0");
    u64 mr[4] = {0, 0, 0, 0};
    u64 reply[4], reply_label;
    invoke(g_control_ep, CONTROL_EXIT, mr, 1, &reply_label, reply);
    (void)reply_label;
    for (;;) { /* supervisor tears this task down */
    }
}