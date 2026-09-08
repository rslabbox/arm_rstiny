#!/usr/bin/env python3
"""Kernel hardware integration checks using QEMU's GDB stub (Python stdlib only).

No test-result UART or guest shutdown syscall: check silent builds via debugger.
Fault probes exist only in kernel-test images. Each probe boots a fresh machine.
"""
import argparse
import os
from pathlib import Path
import socket
import struct
import subprocess
import tempfile
import time
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[1]
TARGET = "aarch64-unknown-none-softfloat"


class Gdb:
    def __init__(self, path, process):
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        deadline = time.monotonic() + 5
        while True:
            try:
                self.sock.connect(str(path))
                break
            except (FileNotFoundError, ConnectionRefusedError):
                if process.poll() is not None or time.monotonic() > deadline:
                    raise RuntimeError("QEMU GDB socket did not become ready")
                time.sleep(0.01)
        self.sock.settimeout(10)
        self.command("qSupported")
        self.command("Hg0")
        self.registers = {}
        self._next_reg = 0
        self._load_registers("target.xml")

    def byte(self):
        b = self.sock.recv(1)
        if not b:
            raise RuntimeError("QEMU disconnected")
        return b

    def command(self, command):
        payload = command.encode()
        self.sock.sendall(b"$" + payload + b"#" + f"{sum(payload) & 255:02x}".encode())
        while self.byte() != b"$":
            pass
        wire = bytearray()
        while True:
            b = self.byte()
            if b == b"#":
                break
            wire += b
        checksum = int(self.byte() + self.byte(), 16)
        assert sum(wire) & 255 == checksum, "GDB checksum mismatch"
        try:
            self.sock.sendall(b"+")
        except BrokenPipeError:
            # QEMU may close immediately after sending the exit packet.
            if not wire.startswith(b"W"):
                raise
        result = bytearray()
        i = 0
        while i < len(wire):
            if wire[i] == ord("}"):
                i += 1
                result.append(wire[i] ^ 0x20)
            elif wire[i] == ord("*"):
                i += 1
                result.extend([result[-1]] * (wire[i] - 29))
            else:
                result.append(wire[i])
            i += 1
        return result.decode()

    def _load_registers(self, annex):
        offset = 0
        data = ""
        while True:
            reply = self.command(f"qXfer:features:read:{annex}:{offset:x},800")
            assert reply[:1] in ("m", "l"), reply
            data += reply[1:]
            offset += len(reply[1:])
            if reply[0] == "l":
                break
        # QEMU's target.xml uses xi:include without declaring the namespace.
        for element in ET.fromstring(data.replace("xi:include", "include")):
            tag = element.tag.split("}")[-1]
            if tag == "include":
                self._load_registers(element.attrib["href"])
            elif tag == "reg":
                number = int(element.attrib.get("regnum", self._next_reg))
                self.registers[element.attrib["name"].lower()] = (
                    number, int(element.attrib["bitsize"]) // 8
                )
                self._next_reg = number + 1

    def reg(self, name):
        # Some QEMU versions expose AArch32 aliases for shared EL1 registers.
        if name.lower() not in self.registers:
            name = {"sctlr_el1": "sctlr", "vbar_el1": "vbar"}.get(name, name)
        number, _ = self.registers[name.lower()]
        return int.from_bytes(bytes.fromhex(self.command(f"p{number:x}")), "little")

    def write_reg(self, name, value):
        number, size = self.registers[name.lower()]
        assert self.command(f"P{number:x}={value.to_bytes(size, 'little').hex()}") == "OK"

    def memory(self, address, size):
        result = bytearray()
        for offset in range(0, size, 1024):
            length = min(size - offset, 1024)
            reply = self.command(f"m{address + offset:x},{length:x}")
            assert not reply.startswith("E"), f"cannot read {address + offset:#x}: {reply}"
            result += bytes.fromhex(reply)
        return bytes(result)

    def word(self, address):
        return int.from_bytes(self.memory(address, 8), "little")

    def run_to(self, address):
        assert self.command(f"Z1,{address:x},4") == "OK"
        reply = self.command("c")
        assert reply.startswith(("T05", "S05")), f"unexpected stop: {reply}"
        assert self.reg("pc") == address, f"wrong PC {self.reg('pc'):#x}"
        assert self.command(f"z1,{address:x},4") == "OK"


def symbols(elf):
    text = subprocess.check_output(["rust-nm", "--defined-only", str(elf)], text=True)
    result = {}
    for line in text.splitlines():
        fields = line.split()
        if len(fields) >= 3:
            result[fields[2]] = int(fields[0], 16)
    return result


def mappings(gdb):
    physical_mask = (1 << 40) - 4096
    root = gdb.reg("ttbr0_el1") & physical_mask
    assert root % 4096 == 0
    found = {}

    def visit(table, level, prefix):
        assert table % 4096 == 0
        entries = struct.unpack("<512Q", gdb.memory(table, 4096))
        for index, entry in enumerate(entries):
            if not entry & 1:
                continue
            assert entry & 2, f"unexpected block at level {level}"
            va = prefix | (index << (39 - level * 9))
            if level < 3:
                visit(entry & physical_mask, level + 1, va)
            else:
                assert entry & physical_mask == va, "unexpected physical alias"
                found[va] = entry
    visit(root, 0, 0)
    return found


def check_layout(gdb, syms):
    assert gdb.reg("cpsr") & 0xF == 5, "not EL1h"
    assert gdb.reg("cpsr") & 0x3C0 == 0x3C0, "interrupts unexpectedly enabled"
    required = 1 | (1 << 2) | (1 << 12) | (1 << 19)
    assert gdb.reg("sctlr_el1") & required == required
    assert gdb.reg("tcr_el1") & (1 << 23), "TTBR1 alias still enabled"
    assert gdb.reg("ttbr1_el1") == 0
    assert gdb.reg("vbar_el1") == syms["exception_vector_base"]
    assert syms["exception_vector_base"] % 2048 == 0
    assert syms["boot_stack"] <= gdb.reg("sp") <= syms["boot_stack_top"]
    assert gdb.reg("sp") % 16 == 0
    pages = mappings(gdb)
    expected = set(range(syms["skernel"], syms["ekernel"], 4096))
    expected.remove(syms["stack_guard"])
    expected.add(0x09000000) # Panic console is always mapped.
    assert pages.keys() == expected, "unexpected mapped/missing pages"
    for va, entry in pages.items():
        assert not entry & (1 << 6), f"EL0-accessible kernel page {va:#x}"
        assert entry & (1 << 54), "UXN missing"
        assert entry & (1 << 10), "access flag missing"
        if va == 0x09000000:
            assert (entry >> 2) & 7 == 0, "UART must use Device memory"
            assert entry & (1 << 53) and not entry & (1 << 7)
        else:
            assert (entry >> 2) & 7 == 1, "RAM must use normal memory"
            writable = va >= syms["erodata"]
            executable = va < syms["etext"]
            assert bool(entry & (1 << 7)) == (not writable)
            assert bool(entry & (1 << 53)) == (not executable)


def boot(qemu, elf, printing, tests=False, probe=None, el2=True, layout=False, quiet_boot=False):
    syms = symbols(elf)
    with tempfile.TemporaryDirectory(prefix="rstiny-kernel-") as directory:
        directory = Path(directory)
        serial = directory / "serial.log"
        error = directory / "qemu.log"
        with error.open("wb") as stderr:
            process = subprocess.Popen([
                qemu, "-machine", f"virt,gic-version=2,virtualization={'on' if el2 else 'off'}",
                "-cpu", "cortex-a72", "-smp", "1", "-m", "128M", "-display", "none",
                "-monitor", "none", "-serial", f"file:{serial}", "-nic", "none",
                "-kernel", str(elf) + ".bin", "-S", "-gdb",
                f"unix:{directory / 'gdb.sock'},server=on,wait=off",
            ], stdout=subprocess.DEVNULL, stderr=stderr)
            gdb = None
            try:
                gdb = Gdb(directory / "gdb.sock", process)
                gdb.run_to(syms["start_root"])
                assert gdb.word(syms["BOOT_ENTRY_EL_VALUE"]) == (2 if el2 else 1)
                if tests:
                    assert gdb.word(syms["SELF_TEST_PASSED"]) == 1
                if layout:
                    check_layout(gdb, syms)
                if probe:
                    gdb.write_reg("pc", syms[probe])
                    gdb.run_to(syms["kernel_shutdown"])
                    if not probe.startswith("probe_panic"):
                        record = struct.unpack("<38Q", gdb.memory(syms["LAST_FAULT"], 38 * 8))
                        kind, source, esr, far = record[:4]
                        assert (kind, source) == (0, 1)
                        ec = esr >> 26
                        if probe == "probe_brk":
                            assert ec == 0x3C and esr & 0xFFFF == 0x123
                        elif probe == "probe_execute_stack":
                            assert ec == 0x21 and esr & 0x3F == 0xF
                            assert syms["boot_stack"] <= far < syms["boot_stack_top"]
                        else:
                            assert ec == 0x25, f"wrong EC: {esr:#x}"
                            if probe == "probe_write_text":
                                assert far == syms["_start"] and esr & (1 << 6)
                                assert esr & 0x3F == 0xF, "not a page permission fault"
                            else:
                                expected_far = {"probe_read_guard": syms["stack_guard"],
                                                "probe_read_unmapped": 0}[probe]
                                assert far == expected_far
                                assert 4 <= esr & 0x3F <= 7, "not a translation fault"
                        if probe != "probe_execute_stack":
                            assert syms[probe] <= record[36] < syms[probe] + 32
                if not probe:
                    # This suite isolates kernel bootstrap; check_fatboot runs EL0.
                    gdb.write_reg("pc", syms["kernel_shutdown"])
                # After inspection, execute the real PSCI call, not host termination.
                reply = gdb.command("c")
                assert reply.startswith("W00"), f"guest did not shut down cleanly: {reply}"
                assert process.wait(timeout=3) == 0
                output = serial.read_bytes()
                if probe and probe.startswith("probe_panic"):
                    assert b"Kernel injected panic" in output
                    assert b"kernel panic:" in output
                    if not printing or quiet_boot:
                        assert b"Kernel ready" not in output
                elif not printing:
                    assert output == b"", f"silent kernel wrote UART: {output!r}"
                elif not quiet_boot:
                    assert b"Kernel ready" in output
                    assert b"\x1b[92mINFO\x1b[0m" in output
                    assert b"\x1b[32mKernel ready" in output
                    if probe and probe.startswith("probe_panic"):
                        assert b"Kernel injected panic" in output
                    elif probe:
                        assert b"fatal exception" in output
                        assert b"\x1b[91mERROR\x1b[0m" in output
                else:
                    assert b"Kernel ready" not in output
                    if probe and probe.startswith("probe_panic"):
                        assert b"Kernel injected panic" in output
                    elif not probe:
                        assert output == b"", "filtered info logs were emitted"
            except Exception:
                print(error.read_text(), flush=True)
                if serial.exists():
                    print(serial.read_text(errors="replace"), flush=True)
                raise
            finally:
                if gdb:
                    gdb.sock.close()
                process.terminate()
                try:
                    process.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()


def build(mode, level, tests):
    subprocess.run(["make", "build", f"MODE={mode}",
                    f"KERNEL_TEST={int(tests)}", f"LOG={level}"], cwd=ROOT, check=True)
    return ROOT / f"target/kernel/{mode}-log{level}-test{int(tests)}/{TARGET}/{mode}/kernel"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--qemu", default="qemu-system-aarch64")
    args = parser.parse_args()
    os.chdir(ROOT)
    for mode in ("debug", "release"):
        for level in ("off", "info"):
            printing = level != "off"
            print(f"CHECK {mode}, LOG={level}", flush=True)
            elf = build(mode, level, False)
            assert not any(s.startswith("probe_") for s in symbols(elf))
            boot(args.qemu, elf, printing, layout=True)
            boot(args.qemu, elf, printing, el2=False)
            elf = build(mode, level, True)
            probes = ["probe_brk", "probe_write_text", "probe_execute_stack",
                      "probe_read_guard", "probe_read_unmapped", "probe_panic", "probe_panic_locked"]
            for probe in probes:
                print(f"  {probe}", flush=True)
                boot(args.qemu, elf, printing, tests=True, probe=probe)
    for level in ("error", "warn", "debug", "trace"):
        elf = build("debug", level, False)
        boot(args.qemu, elf, True, quiet_boot=level in ("error", "warn"))
    # Error filtering must retain fatal diagnostics while suppressing info.
    elf = build("debug", "error", True)
    boot(args.qemu, elf, True, tests=True, probe="probe_panic", quiet_boot=True)
    print("PASS: dev/release x LOG=off/info; EL1/EL2; page permissions; allocator; "
          "disabled log arguments; faults/panic; UART silence; all log levels.", flush=True)


if __name__ == "__main__":
    main()
