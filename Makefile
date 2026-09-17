# Default to release: QEMU TCG is slow and code is small; debug is ~46x
# slower to boot and only needed when tracing (make MODE=debug).
MODE ?= release
KERNEL_TEST ?= 0
BOOT_TEST ?= 0
BLK_TEST ?= 0
# Transitional managed-runtime services (Runtime::Create/Start/.../Map). Only
# the managed-API regression harnesses build them in; production images and
# every loader-based userland check run with the feature compiled out (C3).
MANAGED ?= 0
LOG ?= info
KERNEL_LOAD_MIN ?= 0
DISK ?= 1
# GPU_TEST=1 builds the gpu-server D1 self-test hook (docs/gui-display.md §6),
# mirroring BLK_TEST.
GPU_TEST ?= 0
TARGET := aarch64-unknown-none-softfloat
HOST_TARGET ?= $(shell rustc -vV | sed -n 's/^host: //p')
QEMU ?= qemu-system-aarch64
GDB_PORT ?= 1234
# How QEMU presents the guest display (docs/gui-display.md): `none` keeps the
# boot headless (serial logs only); `gtk` or `sdl` open a window on the
# virtio-gpu framebuffer with live keyboard/mouse input for `./gui wm`.
# Not named DISPLAY: that is the X11 session variable, and env DISPLAY=:0
# would otherwise leak in as `-display :0` on every desktop machine.
QEMU_DISPLAY ?= none

# Vendored upstream sources are not committed (too large, see .gitignore);
# fetch them automatically on first use. Override URL/version for mirrors.
MICROPYTHON_VERSION ?= v1.24.1
MICROPYTHON_URL ?= https://github.com/micropython/micropython.git
MICROPYTHON_DIR := third_party/micropython
# mkenv.mk is the first file py.mk includes: its presence means the tree is
# usable, its absence triggers a (re)clone even after a partial checkout.
MICROPYTHON_MARKER := $(MICROPYTHON_DIR)/py/mkenv.mk

ifeq ($(filter $(MODE),debug release),)
$(error MODE must be debug or release)
endif
ifeq ($(filter $(LOG),off error warn info debug trace),)
$(error LOG must be off, error, warn, info, debug or trace)
endif
ifeq ($(filter $(KERNEL_TEST),0 1),)
$(error KERNEL_TEST must be 0 or 1)
endif

# Separate log levels and test configurations to avoid reusing stale images.
BUILD_DIR := target/kernel/$(MODE)-log$(LOG)-test$(KERNEL_TEST)$(if $(filter 1,$(MANAGED)),-managed,)
KERNEL_ELF := $(BUILD_DIR)/$(TARGET)/$(MODE)/kernel
KERNEL_BIN := $(KERNEL_ELF).bin
PLATFORM_DIR := $(abspath target/platform/qemu-arm-virt)
APP_DIR := target/apps/$(MODE)
USERBOOT_ELF := $(APP_DIR)/$(TARGET)/$(MODE)/userboot
INIT_ELF := $(APP_DIR)/$(TARGET)/$(MODE)/init
CONSOLE_ELF := $(APP_DIR)/$(TARGET)/$(MODE)/console
BLOCK_ELF := $(APP_DIR)/$(TARGET)/$(MODE)/block-server
FS_ELF := $(APP_DIR)/$(TARGET)/$(MODE)/fs-server
GPU_ELF := $(APP_DIR)/$(TARGET)/$(MODE)/gpu-server
APPMGR_ELF := $(APP_DIR)/$(TARGET)/$(MODE)/appmgr
MYSH_ELF := $(APP_DIR)/$(TARGET)/$(MODE)/mysh
HELLO_ELF := $(APP_DIR)/$(TARGET)/$(MODE)/hello
GUI_ELF := $(APP_DIR)/$(TARGET)/$(MODE)/gui
MINIC_ELF := $(APP_DIR)/$(TARGET)/$(MODE)/minic
PYTHON_ELF := $(APP_DIR)/$(TARGET)/$(MODE)/python
DISK_IMG := $(APP_DIR)/disk.img
MODULES := $(INIT_ELF) $(CONSOLE_ELF)
IMAGE_DIR := $(BUILD_DIR)/image
BOOT_IMAGE := $(IMAGE_DIR)/bootloader
CARGO_FLAGS := -p kernel --target $(TARGET) --no-default-features
ifeq ($(MODE),release)
CARGO_FLAGS += --release
endif
ifeq ($(KERNEL_TEST),1)
CARGO_FLAGS += --features kernel-test
endif
ifeq ($(MANAGED),1)
CARGO_FLAGS += --features managed-runtime
endif

# The restart acceptance (KILL_FS) exercises the fs -> appmgr chain. The
# restart=never mysh leaf is excluded there: its live/dead state legitimately
# differs before and after the crash, which would perturb the frame-budget
# comparison (docs/disk-driver.md section 12). The BOOT_TEST supervision
# drill runs a console-only topology: it tests the console crash, the logger
# rebuild and the level-1 init restart, none of which involve the data
# services, whose absence keeps the drill's timing free of failure loops.
INIT_CFG := configs/init.cfg
ifdef KILL_FS
INIT_CFG := configs/init-appmgr.cfg
endif
ifeq ($(BOOT_TEST),1)
INIT_CFG := configs/init-drill.cfg
endif

# Application manifest baked into the disk. The default is empty (the shell
# runs apps on demand); the appmgr/restart/services acceptances point this at
# configs/APPS-hello.CFG, which lists hello.
APPS_CFG ?= configs/APPS.CFG

# Default MicroPython script on the disk (`./python app` reads it; the
# python acceptance overrides this with its own APP.PY, see check_python.py).
APPS_PY ?= configs/APP.PY
# Root filesystem format: fat32 (default) or ext4 (read-only through lwext4,
# docs/disk-driver.md). `make disk FS_TYPE=ext4` builds the ext4 image.
FS_TYPE ?= fat32

# Fixed platform contract; no network backends. The VirtIO block device and
# its FAT32 image back the userland disk stack (docs/disk-driver.md), the
# VirtIO GPU + keyboard back the display stack (docs/gui-display.md §2).
# Device ordinals follow command-line order: blk = virtio-mmio-0 (window
# slot 31), gpu = virtio-mmio-1 (slot 30), keyboard = virtio-mmio-2 (slot 29).
# The drive options live in their own variable: commas inside $(if ...) split
# its arguments, which would silently drop everything after the first one.
DISK_ARGS := -drive file=$(DISK_IMG),if=none,format=raw,id=hd0,readonly=on \
	-device virtio-blk-device,drive=hd0
GPU_ARGS := -device virtio-gpu-device,xres=640,yres=480 \
	-device virtio-keyboard-device -device virtio-mouse-device
QEMU_ARGS := -machine virt,gic-version=3,virtualization=off -cpu cortex-a72 \
	-smp 1 -m 128M -display $(QEMU_DISPLAY) -monitor none -serial stdio -nic none \
	-global virtio-mmio.force-legacy=false \
	$(if $(filter 1,$(DISK)),$(DISK_ARGS)) \
	$(GPU_ARGS) \
	-kernel $(BOOT_IMAGE)
export LOG QEMU KERNEL_LOAD_MIN BOOT_TEST BLK_TEST GPU_TEST

.PHONY: all build platform userboot init console block-server fs-server appmgr mysh hello disk run run-kernel run-root run-userboot debug check manifest fmt clean
all: build

platform:
	python3 tools/build_platform.py $(PLATFORM_DIR) --qemu $(QEMU)

build: userboot init console block-server fs-server gpu-server appmgr mysh platform
	# Frame pointers for the kernel only: the panic-path backtrace walks the
	# x29 chain, and user code must not pay for frames it cannot use.
	PLATFORM_DIR=$(PLATFORM_DIR) RUSTFLAGS="-C force-frame-pointers=yes" \
	  cargo build $(CARGO_FLAGS) --target-dir $(BUILD_DIR)
	rust-objcopy -O binary $(KERNEL_ELF) $(KERNEL_BIN)
	for app in init console; do rust-objcopy --strip-all $(APP_DIR)/$(TARGET)/$(MODE)/$$app $(APP_DIR)/$$app.elf; done
	rust-objcopy --strip-all $(BLOCK_ELF) $(APP_DIR)/block.elf
	rust-objcopy --strip-all $(FS_ELF) $(APP_DIR)/fs.elf
	rust-objcopy --strip-all $(GPU_ELF) $(APP_DIR)/gpu.elf
	rust-objcopy --strip-all $(APPMGR_ELF) $(APP_DIR)/appmgr.elf
	rust-objcopy --strip-all $(MYSH_ELF) $(APP_DIR)/mysh.elf
	python3 tools/build_image.py $(KERNEL_ELF) $(USERBOOT_ELF) $(IMAGE_DIR) --platform $(PLATFORM_DIR) --mode $(MODE) \
	  --module init.cfg=$(INIT_CFG) --module $(APP_DIR)/init.elf --module $(APP_DIR)/console.elf --module $(APP_DIR)/block.elf \
	  --module $(APP_DIR)/fs.elf --module $(APP_DIR)/gpu.elf --module $(APP_DIR)/appmgr.elf --module $(APP_DIR)/mysh.elf

userboot:
	python3 tools/build_app.py userboot --mode $(MODE) $(if $(ROOT_IMAGE_BASE),--image-base $(ROOT_IMAGE_BASE))

init console hello block-server fs-server gpu-server appmgr mysh gui:
	python3 tools/build_app.py $@ --mode $(MODE)

# C applications (interpreter-app.md 决策 F): cross gcc + rstiny-alloc
# staticlib; the output is already stripped, so disk keeps it as-is.
minic:
	python3 tools/build_app.py minic --mode $(MODE) --lang c
	cp $(MINIC_ELF) $(APP_DIR)/minic.elf

# MicroPython interpreter (docs/micropython-port.md): the port Makefile
# compiles the py core; build_app.py ensures rstiny-alloc and strips.
python: $(MICROPYTHON_MARKER)
	python3 tools/build_app.py python --mode $(MODE) --lang python
	cp $(PYTHON_ELF) $(APP_DIR)/python.elf

# Upstream MicroPython at a pinned tag (ports/micropython-rstiny/README.md).
# Cloned on demand so a fresh checkout builds without a manual step. The
# recipe only runs when the py core is unusable (marker absent), so a working
# tree at any version is never touched; a partial/foreign directory is wiped.
$(MICROPYTHON_MARKER):
	@echo ">> fetching vendored micropython $(MICROPYTHON_VERSION) (first build only)"
	@rm -rf $(MICROPYTHON_DIR)
	@git clone --depth 1 --branch $(MICROPYTHON_VERSION) $(MICROPYTHON_URL) $(MICROPYTHON_DIR)

# The application disk: bare FAT32 with the app manifest and its ELFs.
disk: hello gui minic python
	rust-objcopy --strip-all $(HELLO_ELF) $(APP_DIR)/hello.elf
	rust-objcopy --strip-all $(GUI_ELF) $(APP_DIR)/gui.elf
	python3 tools/make_disk.py $(DISK_IMG) --fs-type $(FS_TYPE) \
	  --file hello=$(APP_DIR)/hello.elf --file gui=$(APP_DIR)/gui.elf --file minic=$(APP_DIR)/minic.elf \
	  --file python=$(APP_DIR)/python.elf --file APP.PY=$(APPS_PY) --file APPS.CFG=$(APPS_CFG)

# `run` builds the application disk too, so the guest finds a virtio-blk
# device and the FAT32 image the services need.
RUN_DEPS := build $(if $(filter 1,$(DISK)),disk)
run run-kernel run-root run-userboot: $(RUN_DEPS)
	$(QEMU) $(QEMU_ARGS)

debug: $(RUN_DEPS)
	$(QEMU) $(QEMU_ARGS) -gdb tcp::$(GDB_PORT) -S

manifest:
	python3 tools/build_manifest.py --generate

check:
	python3 tools/build_manifest.py --check
	cargo test -p bootloader --no-default-features --test images --target $(HOST_TARGET)
	cargo test -p kernel-abi -p rstiny-runtime-macros -p rstiny-elf -p rstiny-newc -p rstiny-protocol --target $(HOST_TARGET)
	python3 -m unittest discover -s tools -p 'test_*.py'
	python3 tools/check_bootloader.py --qemu $(QEMU)
	python3 tools/check_kernel.py --qemu $(QEMU)
	python3 tools/check_userboot.py --qemu $(QEMU)
	BOOT_TEST=1 python3 tools/check_fault_handler.py --qemu $(QEMU)
	python3 tools/check_capabilities.py --qemu $(QEMU)
	python3 tools/check_untyped.py --qemu $(QEMU)
	python3 tools/check_ipc.py --qemu $(QEMU)
	python3 tools/check_tasks.py --qemu $(QEMU)
	python3 tools/check_fpu.py --qemu $(QEMU)
	python3 tools/check_user_context.py --qemu $(QEMU)
	python3 tools/check_relocation.py --qemu $(QEMU)
	python3 tools/check_block.py --qemu $(QEMU)
	python3 tools/check_fat32.py --qemu $(QEMU)
	python3 tools/check_ext4.py --qemu $(QEMU)
	python3 tools/check_fs2.py --qemu $(QEMU)
	python3 tools/check_appmgr.py --qemu $(QEMU)
	python3 tools/check_mysh.py --qemu $(QEMU)
	python3 tools/check_python.py --qemu $(QEMU)
	python3 tools/check_services.py --qemu $(QEMU)
	python3 tools/check_restart.py --qemu $(QEMU)
	python3 tools/check_gpu.py --qemu $(QEMU)
	python3 tools/build_manifest.py --check
	python3 tools/build_manifest.py --check-elfs --mode release
	python3 tools/build_manifest.py --check-elfs --mode debug

fmt:
	cargo fmt --all --check

clean:
	cargo clean
