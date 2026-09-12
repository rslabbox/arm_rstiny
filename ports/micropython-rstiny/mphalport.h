#ifndef MICROPY_INCLUDED_PORTS_RSTINY_MPHALPORT_H
#define MICROPY_INCLUDED_PORTS_RSTINY_MPHALPORT_H

#include <stdint.h>
#include <stddef.h>

// Console service (WRITE/READ endpoint IPC, docs/sel4-abi.md). The console
// service is polling-only, so stdin polls and sleeps between attempts.
mp_uint_t mp_hal_stdout_tx_strn(const char *str, size_t len);
int mp_hal_stdin_rx_chr(void);

// Kernel runtime extension calls (Clock/Sleep).
mp_uint_t mp_hal_ticks_ms(void);
void mp_hal_delay_ms(mp_uint_t ms);

// No interrupt-driven Ctrl-C on this port yet: the REPL polls raw bytes and
// sees 0x03 in the stream; there is nothing to install.
static inline void mp_hal_set_interrupt_char(char c) {
    (void)c;
}

#endif // MICROPY_INCLUDED_PORTS_RSTINY_MPHALPORT_H