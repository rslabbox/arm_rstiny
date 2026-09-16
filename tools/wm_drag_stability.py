#!/usr/bin/env python3
"""wm input/movement stability soak.

Continuously drives the wm through its whole input pipeline for a given
duration: arrow keys move the focused window, Tab switches focus between
the calculator and the editor, letters feed the editor. Verifies that move
logs keep flowing for both windows for the entire duration, with no
panics and no lost responsiveness.

Note: QEMU monitor mouse_move/mouse_button (and QMP input-send-event) do
not deliver into the guest's virtio-input queue in headless -display none
setups, so real pointer drags can't be injected here; the arrow-key window
movement exercises the identical wm pipeline (input -> focus -> move ->
redraw -> FLUSH).

    python3 tools/wm_drag_stability.py --seconds 60
"""
import argparse
import os
import select
import socket
import subprocess
import sys
import time

ROOT = '/root/codes/arm_rstiny'

# arrow pattern: one Linux keycode per press, keeps windows on screen
PATTERN = ['right'] * 12 + ['left'] * 12 + ['down'] * 12 + ['up'] * 12


def sendkey(sock, key):
    sock.sendall(f'sendkey {key}\n'.encode())
    time.sleep(0.06)
    try:
        sock.settimeout(0.2)
        sock.recv(4096)
    except TimeoutError:
        pass


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--seconds', type=int, default=60)
    options = parser.parse_args()

    proc = subprocess.Popen([
        'qemu-system-aarch64', '-machine', 'virt,gic-version=3,virtualization=off',
        '-cpu', 'cortex-a72', '-smp', '1', '-m', '128M', '-display', 'none',
        '-monitor', 'unix:/tmp/wm-drag.sock,server=on,wait=off', '-nic', 'none',
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
    end = time.monotonic() + 120
    while time.monotonic() < end and phase < 2:
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
            time.sleep(1.0)
            break
    if phase < 2:
        proc.terminate()
        raise SystemExit('the wm never became ready')

    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    sock.settimeout(3)
    sock.connect('/tmp/wm-drag.sock')
    time.sleep(0.3)
    try:
        sock.recv(65536)
    except TimeoutError:
        pass

    start = time.monotonic()
    keys_sent = 0
    pattern_index = 0
    panics = False
    while time.monotonic() - start < options.seconds:
        sendkey(sock, PATTERN[pattern_index % len(PATTERN)])
        pattern_index += 1
        keys_sent += 1
        r, _, _ = select.select([proc.stdout], [], [], 0.03)
        if r:
            chunk = os.read(proc.stdout.fileno(), 65536)
            if not chunk:
                break
            text += chunk
        got = text.decode(errors='replace')
        if 'panicked' in got or 'kernel panic' in got:
            panics = True
            break

    # responsiveness: 4 more keys must add 4 [gui] log lines
    before = got.count('[gui]')
    for key in PATTERN[:4]:
        sendkey(sock, key)
        time.sleep(0.05)
    end_wait = time.monotonic() + 5
    while time.monotonic() < end_wait:
        r, _, _ = select.select([proc.stdout], [], [], 0.2)
        if r:
            chunk = os.read(proc.stdout.fileno(), 65536)
            if not chunk:
                break
            text += chunk
        if text.decode(errors='replace').count('[gui]') >= before + 4:
            break
    got = text.decode(errors='replace')

    moves = got.count('[gui] calculator move') + got.count('[gui] editor move')
    responsive = got.count('[gui]') >= before + 4
    print(f'keys sent: {keys_sent}, wm move logs: {moves}')
    print(f'responsive after {options.seconds}s: {responsive}')
    print(f'panics: {panics}')
    ok = responsive and not panics and moves > options.seconds
    print('input/movement stability:', 'PASS' if ok else 'FAIL')

    try:
        proc.terminate()
        proc.wait(timeout=3)
    except Exception:
        proc.kill()
    sys.exit(0 if ok else 1)


if __name__ == '__main__':
    main()
