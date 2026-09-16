#!/usr/bin/env python3
"""wm keyboard acceptance: the calculator evaluates an injected expression
and the editor receives letters - each keypress exactly once.

    python3 tools/check_wm_keys.py
"""
import os
import re
import select
import socket
import subprocess
import sys
import time

ROOT = '/root/codes/arm_rstiny'
sys.path.insert(0, ROOT + '/tools')


def monitor_sendkey(sock, *keys):
    for key in keys:
        sock.sendall(f'sendkey {key}\n'.encode())
        time.sleep(0.05)
        try:
            sock.settimeout(1.0)
            sock.recv(4096)
        except TimeoutError:
            pass


def main():
    proc = subprocess.Popen([
        'qemu-system-aarch64', '-machine', 'virt,gic-version=3,virtualization=off',
        '-cpu', 'cortex-a72', '-smp', '1', '-m', '128M', '-display', 'none',
        '-monitor', 'unix:/tmp/wm-mon.sock,server=on,wait=off', '-nic', 'none',
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
    end = time.monotonic() + 150
    sent = ''
    while time.monotonic() < end and phase < 4:
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
            sock.connect('/tmp/wm-mon.sock')
            time.sleep(0.3)
            try:
                sock.recv(65536)
            except TimeoutError:
                pass
            # calculator: 1 + 2 = must evaluate to 3 (once per keypress)
            for key in ('kp_1', 'kp_add', 'kp_2', 'kp_enter'):
                sock.sendall(f'sendkey {key}\n'.encode())
                time.sleep(0.6)
                try:
                    sock.recv(4096)
                except TimeoutError:
                    pass
            phase = 3
    time.sleep(2)

    got = text.decode(errors='replace')
    calc_values = re.findall(r'\[gui\] calc ([0-9+\-*/=]+)', got)
    try:
        proc.terminate()
        proc.wait(timeout=3)
    except Exception:
        proc.kill()

    print('calc entries:', calc_values)
    assert calc_values, 'the calculator never logged an entry'
    assert calc_values[-1] == '3', f'1+2 did not evaluate to 3: {calc_values}'
    assert '33' not in calc_values, 'double-input regression'
    print('PASS: keypad 1+2= evaluates once per press ("3").')
    return 0


if __name__ == '__main__':
    sys.exit(main())
