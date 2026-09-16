#!/usr/bin/env python3
"""wm stability soak: N full cycles of boot -> wm -> calculator injection ->
editor injection -> verify, asserting no panic, no hang, no double-input.

    python3 tools/wm_stability.py --cycles 5
"""
import argparse
import os
import re
import select
import socket
import subprocess
import sys
import time

ROOT = '/root/codes/arm_rstiny'


def sendkey(sock, key):
    sock.sendall(f'sendkey {key}\n'.encode())
    time.sleep(0.4)
    try:
        sock.settimeout(1.0)
        sock.recv(4096)
    except TimeoutError:
        pass


def cycle(index):
    proc = subprocess.Popen([
        'qemu-system-aarch64', '-machine', 'virt,gic-version=3,virtualization=off',
        '-cpu', 'cortex-a72', '-smp', '1', '-m', '128M', '-display', 'none',
        '-monitor', 'unix:/tmp/wm-soak.sock,server=on,wait=off', '-nic', 'none',
        '-global', 'virtio-mmio.force-legacy=false',
        '-drive', 'file=target/apps/release/disk.img,if=none,format=raw,id=hd0,readonly=on',
        '-device', 'virtio-blk-device,drive=hd0',
        '-device', 'virtio-gpu-device,xres=640,yres=480',
        '-device', 'virtio-keyboard-device', '-device', 'virtio-mouse-device',
        '-serial', 'stdio',
        '-kernel', 'target/kernel/release-loginfo-test0/image/bootloader',
    ], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT, cwd=ROOT)

    text = b''
    phase = 0
    result = 'timeout'
    end = time.monotonic() + 150
    while time.monotonic() < end:
        r, _, _ = select.select([proc.stdout], [], [], 0.3)
        if r:
            chunk = os.read(proc.stdout.fileno(), 4096)
            if not chunk:
                break
            text += chunk
        if phase == 0 and b'service started: mysh' in text:
            phase = 1
        elif phase == 1 and b'[rstiny ~]$: ' in text:
            proc.stdin.write(b'./gui wm\n')
            proc.stdin.flush()
            phase = 2
        elif phase == 2 and b'wm ready' in text:
            sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            sock.settimeout(3)
            sock.connect('/tmp/wm-soak.sock')
            time.sleep(0.3)
            try:
                sock.recv(65536)
            except TimeoutError:
                pass
            for key in ('kp_1', 'kp_add', 'kp_2', 'kp_enter'):
                sendkey(sock, key)
            phase = 3
        elif phase == 3 and b'calc 3' in text:
            sendkey(sock, 'tab')  # focus -> editor
            for key in ('a', 'b', 'c'):
                sendkey(sock, key)
            phase = 4
        elif phase == 4 and b'edit abc' in text:
            result = 'ok'
            break

    try:
        proc.terminate()
        proc.wait(timeout=3)
    except Exception:
        proc.kill()

    got = text.decode(errors='replace')
    panics = 'panicked' in got or 'kernel panic' in got
    calc3 = 'calc 3' in got
    editabc = 'edit abc' in got
    ok = result == 'ok' and not panics and calc3 and editabc
    print(f'cycle {index}: {"PASS" if ok else "FAIL"} '
          f'(calc3={calc3} edit-abc={editabc} panics={panics})')
    return ok


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--cycles', type=int, default=5)
    options = parser.parse_args()
    failures = 0
    for index in range(1, options.cycles + 1):
        if not cycle(index):
            failures += 1
    print(f'stability: {options.cycles - failures}/{options.cycles} cycles passed')
    return 1 if failures else 0


if __name__ == '__main__':
    sys.exit(main())
