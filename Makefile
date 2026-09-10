MODE ?= debug
KERNEL_TEST ?= 0
BOOT_TEST ?= 0
LOG ?= info
KERNEL_LOAD_MIN ?= 0
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

# Fixed platform contract; no disks or network backends.
QEMU_ARGS := -machine virt,gic-version=3,virtualization=off -cpu cortex-a72 \
	-smp 1 -m 128M -display none -monitor none -serial stdio -nic none \
	-kernel $(BOOT_IMAGE)
export LOG QEMU KERNEL_LOAD_MIN BOOT_TEST

.PHONY: all build platform userboot init console run run-kernel run-root run-userboot debug check fmt clean
all: build

platform:
	python3 tools/build_platform.py $(PLATFORM_DIR) --qemu $(QEMU)

build: userboot init console platform
	PLATFORM_DIR=$(PLATFORM_DIR) cargo build $(CARGO_FLAGS) --target-dir $(BUILD_DIR)
	rust-objcopy -O binary $(KERNEL_ELF) $(KERNEL_BIN)
	for app in init console; do rust-objcopy --strip-all $(APP_DIR)/$(TARGET)/$(MODE)/$$app $(APP_DIR)/$$app.elf; done
	python3 tools/build_image.py $(KERNEL_ELF) $(USERBOOT_ELF) $(IMAGE_DIR) --platform $(PLATFORM_DIR) --mode $(MODE) \
	  --module $(APP_DIR)/init.elf --module $(APP_DIR)/console.elf --module init.cfg=apps/init.cfg

userboot:
	python3 tools/build_app.py userboot --mode $(MODE) $(if $(ROOT_IMAGE_BASE),--image-base $(ROOT_IMAGE_BASE))

init console:
	python3 tools/build_app.py $@ --mode $(MODE)

run run-kernel run-root run-userboot: build
	$(QEMU) $(QEMU_ARGS)

debug: build
	$(QEMU) $(QEMU_ARGS) -gdb tcp::$(GDB_PORT) -S

check:
	cargo test -p bootloader --no-default-features --test images --target $(HOST_TARGET)
	cargo test -p kernel-abi -p rstiny-runtime-macros -p rstiny-elf -p rstiny-newc --target $(HOST_TARGET)
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

fmt:
	cargo fmt --all --check

clean:
	cargo clean
