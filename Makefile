MODE ?= debug
KERNEL_TEST ?= 0
LOG ?= info
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
APP_DIR := target/apps/$(MODE)
FATBOOT_ELF := $(APP_DIR)/$(TARGET)/$(MODE)/fatboot
ROOT_IMAGE := $(abspath $(APP_DIR)/fatboot.boot)
APP_FLAGS := -p fatboot --target $(TARGET) --target-dir $(APP_DIR)
CARGO_FLAGS := -p kernel --target $(TARGET) --target-dir $(BUILD_DIR) --no-default-features
ifeq ($(MODE),release)
CARGO_FLAGS += --release
APP_FLAGS += --release
endif
ifeq ($(KERNEL_TEST),1)
CARGO_FLAGS += --features kernel-test
endif

# Fixed platform contract; no disks or network backends.
QEMU_ARGS := -machine virt,gic-version=2,virtualization=on -cpu cortex-a72 \
	-smp 1 -m 128M -display none -monitor none -serial stdio -nic none \
	-kernel $(KERNEL_BIN)
export LOG ROOT_IMAGE

.PHONY: all build fatboot run run-kernel run-root debug check fmt clean
all: build

build: fatboot
	cargo build $(CARGO_FLAGS)
	rust-objcopy -O binary $(KERNEL_ELF) $(KERNEL_BIN)

fatboot:
	cargo build $(APP_FLAGS)
	python3 tools/pack_root.py $(FATBOOT_ELF) $(ROOT_IMAGE)

run run-kernel run-root: build
	$(QEMU) $(QEMU_ARGS)

debug: build
	$(QEMU) $(QEMU_ARGS) -gdb tcp::$(GDB_PORT) -S

check:
	cargo test -p rstiny-runtime-macros --target $(HOST_TARGET)
	python3 -m unittest discover -s tools -p 'test_*.py'
	python3 tools/check_kernel.py --qemu $(QEMU)
	python3 tools/check_fatboot.py --qemu $(QEMU)

fmt:
	cargo fmt --all --check

clean:
	cargo clean
