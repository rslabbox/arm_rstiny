MODE ?= debug
KERNEL_TEST ?= 0
LOG ?= info
TARGET := aarch64-unknown-none-softfloat
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
CARGO_FLAGS := -p kernel --target $(TARGET) --target-dir $(BUILD_DIR) --no-default-features
ifeq ($(MODE),release)
CARGO_FLAGS += --release
endif
ifeq ($(KERNEL_TEST),1)
CARGO_FLAGS += --features kernel-test
endif

# Fixed platform contract; no disks or network backends.
QEMU_ARGS := -machine virt,gic-version=2,virtualization=on -cpu cortex-a72 \
	-smp 1 -m 128M -display none -monitor none -serial stdio -nic none \
	-kernel $(KERNEL_BIN)
export LOG

.PHONY: all build run run-kernel debug check fmt clean
all: build

build:
	cargo build $(CARGO_FLAGS)
	rust-objcopy -O binary $(KERNEL_ELF) $(KERNEL_BIN)

run run-kernel: build
	$(QEMU) $(QEMU_ARGS)

debug: build
	$(QEMU) $(QEMU_ARGS) -gdb tcp::$(GDB_PORT) -S

check:
	python3 tools/check_kernel.py --qemu $(QEMU)

fmt:
	cargo fmt --all --check

clean:
	cargo clean
