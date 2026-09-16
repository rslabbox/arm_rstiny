#!/usr/bin/env python3
"""GPU display stack acceptance (docs/gui-display.md section 6, D0-D4).

D0 platform: attaching virtio-gpu + virtio-keyboard must not change the
platform contract - the VirtIO MMIO window keeps its 32 contiguous slots and
slot lines, so no new device capability has to be published.

D1 driver: gpu-server initialises the device, bands the screen and logs a
word checksum; the expected value is recomputed here from the server's own
palette table (GPU_TEST=1 build).

D2/D3 lease: `./gui <scene>` leases the framebuffer capabilities, draws a
scene with the 8x8 font and flushes; the checksum is recomputed from the
raster model plus the font table in projects/libs/gui/src/font.rs. Running
the scene twice also proves the RELEASE handed every cap back.

D4 input: `./gui keys 3` logs key presses injected through the QEMU monitor
(`sendkey`); the virtio-keyboard event must reach the client via
INPUT_READ.
"""
import argparse
import os
import re
import select
import socket
import subprocess
import tempfile
import time
from pathlib import Path

from check_kernel import boot_image

BOOT_TIMEOUT = 600.0
TARGET = 'aarch64-unknown-none-softfloat'
PROMPT = b'[rstiny ~]$: '
ROOT = Path(__file__).resolve().parent.parent
WIDTH, HEIGHT = 640, 480
FB_PAGES = WIDTH * HEIGHT * 4 // 4096


def rgb(red, green, blue):
    return 0xFF00_0000 | red << 16 | green << 8 | blue


def load_palette(source, name):
    """Parse a Rust `[(u8, u8, u8); N]` const out of a source file."""
    match = re.search(rf'{name}\s*:\s*\[\(u8, u8, u8\); \d+\] = \[(.*?)\];', source, re.S)
    assert match, f'{name} not found'
    return [
        (int(r, 0), int(g, 0), int(b, 0))
        for r, g, b in re.findall(r'\((0x[0-9a-fA-F]+), (0x[0-9a-fA-F]+), (0x[0-9a-fA-F]+)\)', match.group(1))
    ]


def load_font(path):
    """Parse projects/libs/gui/src/font.rs: char -> 8 row bytes, LSB = left."""
    glyphs = {}
    for match in re.finditer(r'\[([0-9a-fx, ]+)\], // (.) U\+([0-9A-F]{4})', path.read_text()):
        rows = [int(value, 0) for value in match.group(1).split(',')]
        assert len(rows) == 8, match.group(0)
        glyphs[int(match.group(3), 16)] = rows
    assert len(glyphs) >= 64, f'font table too small: {len(glyphs)}'
    return glyphs


FONT = None
QUESTION_MARK = None


class Canvas:
    """The host-side mirror of rstiny-gui's drawing rules."""

    def __init__(self, width, height):
        self.width = width
        self.height = height
        self.pixels = [0] * (width * height)

    def fill_rect(self, x, y, w, h, color):
        for row in range(y, min(y + h, self.height)):
            base = row * self.width
            for column in range(x, min(x + w, self.width)):
                self.pixels[base + column] = color

    def draw_text(self, text, x, y, color):
        pen = x
        for character in text:
            glyph = FONT.get(ord(character), QUESTION_MARK)
            for row, bits in enumerate(glyph):
                for column in range(8):
                    if bits & (1 << column):
                        px, py = pen + column, y + row
                        if px < self.width and py < self.height:
                            self.pixels[py * self.width + px] = color
            pen += 8

    def scroll_up(self, rows, background):
        keep = self.height - rows
        self.pixels = self.pixels[rows * self.width:] + [background] * (rows * self.width)
        assert keep == self.height - rows

    def checksum(self):
        return sum(self.pixels) & 0xFFFFFFFF


def expected_d1(palette):
    """gpu-server's GPU_TEST bands: row r gets PALETTE[min(r, 15)]."""
    canvas = Canvas(WIDTH, HEIGHT)
    for row in range(HEIGHT):
        red, green, blue = palette[min(row, len(palette) - 1)]
        canvas.fill_rect(0, row, WIDTH, 1, rgb(red, green, blue))
    return canvas.checksum()


def expected_bars(bars):
    canvas = Canvas(WIDTH, HEIGHT)
    canvas.fill_rect(0, 0, WIDTH, HEIGHT, rgb(0x10, 0x10, 0x30))
    band_top = HEIGHT // 8
    band_bottom = HEIGHT - band_top
    for index, (red, green, blue) in enumerate(bars):
        left = index * WIDTH // len(bars)
        right = (index + 1) * WIDTH // len(bars)
        canvas.fill_rect(left, band_top, right - left, band_bottom - band_top,
                         rgb(red, green, blue))
    return canvas.checksum()


def expected_text():
    canvas = Canvas(WIDTH, HEIGHT)
    canvas.fill_rect(0, 0, WIDTH, HEIGHT, rgb(0x00, 0x00, 0x40))
    canvas.draw_text('RSTINY GUI', 16, 16, rgb(0xFF, 0xFF, 0xFF))
    canvas.draw_text('HELLO FROM THE FRAMEBUFFER', 16, 32, rgb(0x00, 0xFF, 0x00))
    canvas.draw_text('0123456789 !?:-/', 16, 48, rgb(0xFF, 0xFF, 0x00))
    canvas.draw_text('LEASING WORKS', 16, 64, rgb(0xFF, 0x40, 0x40))
    return canvas.checksum()


def expected_scroll():
    canvas = Canvas(WIDTH, HEIGHT)
    background = rgb(0x00, 0x00, 0x00)
    canvas.fill_rect(0, 0, WIDTH, HEIGHT, background)
    for line in range(20):
        canvas.draw_text(f'LINE {line // 10}{line % 10}', 8, 8 + line * 16,
                         rgb(0xFF, 0xFF, 0xFF))
    canvas.scroll_up(64, background)
    canvas.scroll_up(64, background)
    canvas.draw_text('SCROLLED', 8, 16, rgb(0xFF, 0xFF, 0x00))
    return canvas.checksum()


def platform_contract(qemu):
    """D0: the GPU devices must not change the VirtIO MMIO window contract."""
    with tempfile.TemporaryDirectory(prefix='rstiny-gpu-platform-') as temporary:
        raw = Path(temporary) / 'gpu.dtb'
        subprocess.run([qemu,
                        '-machine', f'virt,gic-version=3,virtualization=off,dumpdtb={raw}',
                        '-cpu', 'cortex-a72', '-smp', '1', '-m', '128M', '-display', 'none',
                        '-nic', 'none',
                        '-device', 'virtio-gpu-device,xres=640,yres=480',
                        '-device', 'virtio-keyboard-device',
                        '-device', 'virtio-mouse-device'],
                       check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        nodes = subprocess.check_output(
            ['fdtget', '-l', str(raw), '/'], text=True).split()
        virtio = [node for node in nodes if node.startswith('virtio_mmio')]
        assert len(virtio) == 32, f'expected 32 virtio-mmio slots, found {len(virtio)}'
        bases = []
        for node in virtio:
            reg = subprocess.check_output(
                ['fdtget', '-t', 'x', str(raw), f'/{node}', 'reg'], text=True).split()
            words = [int(value, 16) for value in reg]
            bases.append(words[0] << 32 | words[1])
        assert bases == [0x0a000000 + index * 0x200 for index in range(32)], \
            'virtio-mmio slots are not the contiguous platform window'
    print('PASS: D0 platform - virtio-gpu/keyboard need no new device publication.',
          flush=True)


class Monitor:
    """QEMU monitor on a unix socket, for `sendkey` (D4)."""

    def __init__(self, path):
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.connect(path)
        self.sock.setblocking(False)
        self._drain()

    def _drain(self):
        while True:
            try:
                if not self.sock.recv(4096):
                    break
            except BlockingIOError:
                break

    def sendkey(self, key):
        self._drain()
        self.sock.sendall(f'sendkey {key}\n'.encode())
        time.sleep(0.2)
        self._drain()


def run_boot(qemu, kernel, disk, monitor_path=None):
    args = [
        qemu, '-machine', 'virt,gic-version=3,virtualization=off', '-cpu', 'cortex-a72',
        '-smp', '1', '-m', '128M', '-display', 'none', '-monitor', 'none', '-nic', 'none',
        '-global', 'virtio-mmio.force-legacy=false',
        '-drive', f'file={disk},if=none,format=raw,id=hd0,readonly=on',
        '-device', 'virtio-blk-device,drive=hd0',
        '-device', 'virtio-gpu-device,xres=640,yres=480',
        '-device', 'virtio-keyboard-device',
        '-device', 'virtio-mouse-device',
    ]
    if monitor_path:
        args += ['-monitor', f'unix:{monitor_path},server=on,wait=off']
    else:
        args += ['-monitor', 'none']
    args += ['-serial', 'stdio', '-kernel', str(boot_image(kernel))]
    return subprocess.Popen(args, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                            stderr=subprocess.STDOUT)


def run_d1(qemu, mode):
    """GPU_TEST self-test: the banded screen checksum must match the model."""
    subprocess.run(['make', 'build', f'MODE={mode}', 'LOG=info', 'GPU_TEST=1', 'DISK=1'],
                   cwd=ROOT, check=True, stdout=subprocess.DEVNULL)
    subprocess.run(['make', 'disk', f'MODE={mode}'], cwd=ROOT, check=True,
                   stdout=subprocess.DEVNULL)
    kernel = ROOT / f'target/kernel/{mode}-loginfo-test0/{TARGET}/{mode}/kernel'
    disk = ROOT / f'target/apps/{mode}/disk.img'
    palette = load_palette((ROOT / 'projects/apps/gpu-server/src/main.rs').read_text(),
                           'PALETTE')
    expected = expected_d1(palette)
    proc = run_boot(qemu, kernel, disk)
    try:
        deadline = time.monotonic() + BOOT_TIMEOUT
        text = ''
        while time.monotonic() < deadline:
            text += read_serial(proc)
            if f'[gpu] test flush {WIDTH}x{HEIGHT} pages={FB_PAGES} sum={expected:#x}' in text:
                break
            assert proc.poll() is None, 'system exited before the self-test'
        else:
            print(text, flush=True)
            raise AssertionError('gpu-server never completed its test flush')
        assert proc.poll() is None, 'system exited early'
        print(f'PASS: D1 driver - self-test flush checksum matches ({expected:#x}).',
              flush=True)
    finally:
        if proc.poll() is None:
            proc.terminate()
            proc.wait(timeout=5)


def read_serial(proc):
    text = ''
    while True:
        ready, _, _ = select.select([proc.stdout], [], [], 0.2)
        if not ready:
            break
        chunk = os.read(proc.stdout.fileno(), 4096)
        if not chunk:
            break
        text += chunk.decode(errors='replace')
    return text


def run_scenarios(qemu, mode, level):
    """D2-D4 in one booted shell: bars twice, text, scroll, then keys."""
    subprocess.run(['make', 'build', f'MODE={mode}', f'LOG={level}', 'DISK=1'],
                   cwd=ROOT, check=True, stdout=subprocess.DEVNULL)
    subprocess.run(['make', 'disk', f'MODE={mode}'], cwd=ROOT, check=True,
                   stdout=subprocess.DEVNULL)
    kernel = ROOT / f'target/kernel/{mode}-log{level}-test0/{TARGET}/{mode}/kernel'
    disk = ROOT / f'target/apps/{mode}/disk.img'
    bars = load_palette((ROOT / 'projects/apps/gui/src/main.rs').read_text(), 'BARS')
    with tempfile.TemporaryDirectory(prefix='rstiny-gpu-') as temporary:
        monitor = str(Path(temporary) / 'monitor')
        proc = run_boot(qemu, kernel, disk, monitor_path=monitor)
        try:
            deadline = time.monotonic() + BOOT_TIMEOUT
            text = ''
            while time.monotonic() < deadline:
                text += read_serial(proc)
                if '[mysh] ready' in text:
                    break
            else:
                print(text[-3000:], flush=True)
            assert '[mysh] ready' in text, 'mysh never became ready'

            bars_sum = expected_bars(bars)
            text_sum = expected_text()
            scroll_sum = expected_scroll()

            commands = [
                (f'./gui bars', [
                    f'[gui] bars {WIDTH}x{HEIGHT} sum={bars_sum:#x}',
                    '[gui] flushed',
                    '[gui] released',
                    '[mysh] ./gui exited: 0',
                ]),
                # Second lease proves RELEASE handed every cap back.
                (f'./gui bars', [
                    f'[gpu] client 0x0 leased {FB_PAGES} pages',
                    f'[gui] bars {WIDTH}x{HEIGHT} sum={bars_sum:#x}',
                    '[mysh] ./gui exited: 0',
                ]),
                (f'./gui text', [
                    f'[gui] text {WIDTH}x{HEIGHT} sum={text_sum:#x}',
                    '[mysh] ./gui exited: 0',
                ]),
                (f'./gui scroll', [
                    f'[gui] scroll {WIDTH}x{HEIGHT} sum={scroll_sum:#x}',
                    '[mysh] ./gui exited: 0',
                ]),
            ]
            for command, expectations in commands:
                start = len(text)
                proc.stdin.write(command.encode() + b'\n')
                proc.stdin.flush()
                while time.monotonic() < deadline:
                    text += read_serial(proc)
                    if all(expectation in text[start:] for expectation in expectations):
                        break
                    assert 'kernel panic' not in text and 'panicked' not in text
                else:
                    print(text, flush=True)
                    raise AssertionError(f'./gui never completed: {command}')
                assert '[mysh] ready' in text

            # D4: the keys scene waits for three presses over INPUT_READ.
            start = len(text)
            proc.stdin.write(b'./gui keys 3\n')
            proc.stdin.flush()
            monitor_sock = Monitor(monitor)
            while time.monotonic() < deadline:
                text += read_serial(proc)
                if 'leased' in text[start:]:
                    for key in ('a', 'b', 'c'):
                        monitor_sock.sendkey(key)
                    break
            while time.monotonic() < deadline:
                text += read_serial(proc)
                if f'[mysh] ./gui exited: 0' in text[start:]:
                    break
            else:
                print(text, flush=True)
                raise AssertionError('the keys scene never completed')
            for key in ('a', 'b', 'c'):
                assert f'[gui] key: {key}' in text[start:], f'keypress {key} never arrived'
            assert 'kernel panic' not in text and 'panicked' not in text
            assert proc.poll() is None, 'system exited early'
            print('PASS: D2/D3 lease - bars/text/scroll checksums match, lease is '
                  're-acquired after RELEASE.', flush=True)
            print('PASS: D4 input - three sendkey presses reached INPUT_READ.',
                  flush=True)
        finally:
            if proc.poll() is None:
                proc.terminate()
                proc.wait(timeout=5)


def main():
    global FONT, QUESTION_MARK
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--qemu', default='qemu-system-aarch64')
    args = parser.parse_args()
    FONT = load_font(ROOT / 'projects/libs/gui/src/font.rs')
    QUESTION_MARK = FONT[0x3F]
    platform_contract(args.qemu)
    for mode in ('debug', 'release'):
        run_d1(args.qemu, mode)
    for mode in ('debug', 'release'):
        for level in ('off', 'info'):
            print(f'CHECK gpu {mode} LOG={level}: ./gui scenes', flush=True)
            run_scenarios(args.qemu, mode, level)
    print('PASS: gpu display stack verified across debug/release and LOG levels.',
          flush=True)


if __name__ == '__main__':
    main()
