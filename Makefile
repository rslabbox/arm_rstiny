MODE ?= debug
KERNEL_TEST ?= 0
BOOT_TEST ?= 0
BLK_TEST ?= 0
LOG ?= info
KERNEL_LOAD_MIN ?= 0
DISK ?= 1
TARGET := aarch64-unknown-none-softfloat
HOST_TARGET ?= $(shell rustc -vV | sed -n 's/^host: //p')
QEMU ?= qemu-system-aarch64
GDB_PORT ?= 1234

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
BUILD_DIR := target/kernel/$(MODE)-log$(LOG)-test$(KERNEL_TEST)
KERNEL_ELF := $(BUILD_DIR)/$(TARGET)/$(MODE)/kernel
KERNEL_BIN := $(KERNEL_ELF).bin
PLATFORM_DIR := $(abspath target/platform/qemu-arm-virt)
APP_DIR := target/apps/$(MODE)
USERBOOT_ELF := $(APP_DIR)/$(TARGET)/$(MODE)/userboot
INIT_ELF := $(APP_DIR)/$(TARGET)/$(MODE)/init
CONSOLE_ELF := $(APP_DIR)/$(TARGET)/$(MODE)/console
BLOCK_ELF := $(APP_DIR)/$(TARGET)/$(MODE)/block-server
FS_ELF := $(APP_DIR)/$(TARGET)/$(MODE)/fs-server
APPMGR_ELF := $(APP_DIR)/$(TARGET)/$(MODE)/appmgr
MYSH_ELF := $(APP_DIR)/$(TARGET)/$(MODE)/mysh
HELLO_ELF := $(APP_DIR)/$(TARGET)/$(MODE)/hello
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

# Fixed platform contract; no network backends. The VirtIO block device and
# its FAT32 image back the userland disk stack (docs/disk-driver.md). The drive
# options live in their own variable: commas inside $(if ...) split its
# arguments, which would silently drop everything after the first one.
DISK_ARGS := -drive file=$(DISK_IMG),if=none,format=raw,id=hd0,readonly=on \
	-device virtio-blk-device,drive=hd0
QEMU_ARGS := -machine virt,gic-version=3,virtualization=off -cpu cortex-a72 \
	-smp 1 -m 128M -display none -monitor none -serial stdio -nic none \
	-global virtio-mmio.force-legacy=false \
	$(if $(filter 1,$(DISK)),$(DISK_ARGS)) \
	-kernel $(BOOT_IMAGE)
export LOG QEMU KERNEL_LOAD_MIN BOOT_TEST BLK_TEST

.PHONY: all build platform userboot init console block-server fs-server appmgr mysh hello disk run run-kernel run-root run-userboot debug check fmt clean
all: build

platform:
	python3 tools/build_platform.py $(PLATFORM_DIR) --qemu $(QEMU)

build: userboot init console block-server fs-server appmgr mysh platform
	PLATFORM_DIR=$(PLATFORM_DIR) cargo build $(CARGO_FLAGS) --target-dir $(BUILD_DIR)
	rust-objcopy -O binary $(KERNEL_ELF) $(KERNEL_BIN)
	for app in init console; do rust-objcopy --strip-all $(APP_DIR)/$(TARGET)/$(MODE)/$$app $(APP_DIR)/$$app.elf; done
	rust-objcopy --strip-all $(BLOCK_ELF) $(APP_DIR)/block.elf
	rust-objcopy --strip-all $(FS_ELF) $(APP_DIR)/fs.elf
	rust-objcopy --strip-all $(APPMGR_ELF) $(APP_DIR)/appmgr.elf
	rust-objcopy --strip-all $(MYSH_ELF) $(APP_DIR)/mysh.elf
	python3 tools/build_image.py $(KERNEL_ELF) $(USERBOOT_ELF) $(IMAGE_DIR) --platform $(PLATFORM_DIR) --mode $(MODE) \
	  --module $(APP_DIR)/init.elf --module $(APP_DIR)/console.elf --module $(APP_DIR)/block.elf \
	  --module $(APP_DIR)/fs.elf --module $(APP_DIR)/appmgr.elf --module $(APP_DIR)/mysh.elf --module init.cfg=apps/init.cfg

userboot:
	python3 tools/build_app.py userboot --mode $(MODE) $(if $(ROOT_IMAGE_BASE),--image-base $(ROOT_IMAGE_BASE))

init console hello block-server fs-server appmgr mysh:
	python3 tools/build_app.py $@ --mode $(MODE)

# The application disk: bare FAT32 with the app manifest, the shell script and
# the app ELF.
disk: hello
	rust-objcopy --strip-all $(HELLO_ELF) $(APP_DIR)/hello.elf
	python3 tools/make_disk.py $(DISK_IMG) \
	  --file HELLO.ELF=$(APP_DIR)/hello.elf --file APPS.CFG=apps/APPS.CFG \
	  --file SH.CFG=apps/SH.CFG

# `run` builds the application disk too, so the guest finds a virtio-blk
# device and the FAT32 image the services need.
RUN_DEPS := build $(if $(filter 1,$(DISK)),disk)
run run-kernel run-root run-userboot: $(RUN_DEPS)
	$(QEMU) $(QEMU_ARGS)

debug: $(RUN_DEPS)
	$(QEMU) $(QEMU_ARGS) -gdb tcp::$(GDB_PORT) -S

check:
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
	python3 tools/check_user_context.py --qemu $(QEMU)
	python3 tools/check_relocation.py --qemu $(QEMU)
	python3 tools/check_block.py --qemu $(QEMU)
	python3 tools/check_fat32.py --qemu $(QEMU)
	python3 tools/check_appmgr.py --qemu $(QEMU)
	python3 tools/check_mysh.py --qemu $(QEMU)
	python3 tools/check_services.py --qemu $(QEMU)
	python3 tools/check_restart.py --qemu $(QEMU)

fmt:
	cargo fmt --all --check

clean:
	cargo clean
