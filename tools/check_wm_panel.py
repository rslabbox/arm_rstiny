#!/usr/bin/env python3
"""wm panel acceptance: the top status bar renders, and clicking the
shutdown button at its top-right powers the machine off (PSCI via the
kernel Runtime) - QEMU terminates and the log carries the request.

    python3 tools/check_wm_panel.py
"""
import os
import re
import select
import socket
import subprocess
import threading
import time

ROOT = '/root/codes/arm_rstiny'
MON = '/tmp/wm-panel-check.sock'


def main():
    os.path.exists(MON) and os.unlink(MON)
    proc = subprocess.Popen([
        'qemu-system-aarch64', '-machine', 'virt,gic-version=3,virtualization=off',
        '-cpu', 'cortex-a72', '-smp', '1', '-m', '128M', '-display', 'none',
        '-monitor', f'unix:{MON},server=on,wait=off', '-nic', 'none',
        '-global', 'virtio-mmio.force-legacy=false',
        '-drive', 'file=target/apps/release/disk.img,if=none,format=raw,id=hd0,readonly=on',
        '-device', 'virtio-blk-device,drive=hd0',
        '-device', 'virtio-gpu-device,xres=640,yres=480',
        '-device', 'virtio-keyboard-device', '-device', 'virtio-mouse-device',
        '-serial', 'stdio',
        '-kernel', 'target/kernel/release-loginfo-test0/image/bootloader',
    ], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT, cwd=ROOT)

    chunks = []
    def reader():
        while True:
            r, _, _ = select.select([proc.stdout], [], [], 0.2)
            if r:
                chunk = os.read(proc.stdout.fileno(), 65536)
                if not chunk:
                    return
                chunks.append(chunk)
    threading.Thread(target=reader, daemon=True).start()

    def wait_for(marker, timeout=120):
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            if marker in b''.join(chunks):
                return True
            time.sleep(0.1)
        return False

    def pump():
        r, _, _ = select.select([proc.stdout], [], [], 0.05)
        if r:
            chunk = os.read(proc.stdout.fileno(), 65536)
            if chunk:
                chunks.append(chunk)

    wait_for(b'service started: mysh')
    try:
        proc.stdin.write(b'./gui wm\n')
        proc.stdin.flush()
    except BrokenPipeError:
        pass
    assert wait_for(b'wm ready: calculator', 90), 'the wm never became ready'

    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    sock.settimeout(2)
    sock.connect(MON)
    time.sleep(0.5)
    try:
        sock.recv(65536)
    except TimeoutError:
        pass

    # walk the cursor from the screen center (320, 240) to the shutdown
    # button (576..632, 3..19): 32 relative steps of (+8, -7)
    for _ in range(32):
        sock.sendall(b'mouse_move 8 -7\n')
        time.sleep(0.03)
        pump()
    sock.sendall(b'mouse_button 1\n')
    time.sleep(0.4)
    pump()
    try:
        # the machine may already have powered off mid-click
        sock.sendall(b'mouse_button 0\n')
    except BrokenPipeError:
        pass

    # the machine must power off within a few seconds
    end = time.monotonic() + 15
    exited = False
    while time.monotonic() < end:
        if proc.poll() is not None:
            exited = True
            break
        time.sleep(0.2)
        pump()

    text = re.sub(r'\x1b\[[0-9;]*m', '', b''.join(chunks).decode(errors='replace'))
    requested = '[gui] shutdown requested' in text
    print(f'shutdown requested: {requested}, QEMU powered off: {exited}')
    try:
        proc.terminate()
        proc.wait(timeout=3)
    except Exception:
        proc.kill()
    assert requested and exited, 'the shutdown button did not power the machine off'
    print('PASS: panel shutdown button powers the machine off (PSCI SYSTEM_OFF).')


if __name__ == '__main__':
    main()
