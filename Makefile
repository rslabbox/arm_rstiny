# ARM RSTiny - Rust Bare Metal OS Makefile

# 项目配置
PROJECT_NAME = arm_rstiny
MODE := debug
TARGET = aarch64-unknown-none-softfloat
BINARY = target/$(TARGET)/$(MODE)/$(PROJECT_NAME)
KERNEL_BIN = $(PROJECT_NAME).bin
LOG := info

ifeq ($(MODE), release)
	MODE_ARG := --release
endif

# QEMU 配置
QEMU = qemu-system-aarch64
QEMU_ARGS = -machine virt \
            -cpu cortex-a72 \
            -smp 1 \
            -m 128M \
            -nographic \
            -kernel $(BINARY)

# 编译选项
CARGO_FLAGS = $(MODE_ARG) --target $(TARGET)

export LOG

.PHONY: all build run clean help install-target

# 默认目标
all: build

# 编译项目
build:
	@echo "Building $(PROJECT_NAME)..."
	cargo build $(CARGO_FLAGS)
	@echo "Build completed: $(BINARY)"

# 运行项目
run: build
	@echo "Starting $(PROJECT_NAME) in QEMU..."
	@echo "Press Ctrl+A then X to exit QEMU"
	$(QEMU) $(QEMU_ARGS)

# 调试模式运行
debug: build
	@echo "Starting $(PROJECT_NAME) in QEMU with GDB support..."
	@echo "Connect with: gdb-multiarch -ex 'target remote :1234' $(BINARY)"
	@echo "Press Ctrl+A then X to exit QEMU"
	$(QEMU) $(QEMU_ARGS) -s -S

# 清理编译产物
clean:
	@echo "Cleaning build artifacts..."
	cargo clean
	rm -f $(KERNEL_BIN)

# 安装目标架构
install-target:
	@echo "Installing Rust target: $(TARGET)"
	rustup target add $(TARGET)

# 检查依赖
check-deps:
	@echo "Checking dependencies..."
	@which $(QEMU) > /dev/null || (echo "Error: $(QEMU) not found. Please install QEMU." && exit 1)
	@which cargo > /dev/null || (echo "Error: cargo not found. Please install Rust." && exit 1)
	@rustup target list --installed | grep -q $(TARGET) || (echo "Target $(TARGET) not installed. Run 'make install-target'" && exit 1)
	@echo "All dependencies satisfied."

# 显示帮助信息
help:
	@echo "ARM RSTiny - Rust Bare Metal OS"
	@echo "================================="
	@echo ""
	@echo "Available targets:"
	@echo "  build         - Compile the project"
	@echo "  run           - Build and run in QEMU"
	@echo "  debug         - Build and run in QEMU with GDB support"
	@echo "  clean         - Clean build artifacts"
	@echo "  install-target- Install required Rust target"
	@echo "  check-deps    - Check if all dependencies are installed"
	@echo "  help          - Show this help message"
	@echo ""
	@echo "QEMU controls:"
	@echo "  Ctrl+A, X     - Exit QEMU"
	@echo "  Ctrl+A, C     - QEMU monitor console"

# 显示项目信息
info:
	@echo "Project: $(PROJECT_NAME)"
	@echo "Target: $(TARGET)"
	@echo "Binary: $(BINARY)"
	@echo "QEMU: $(QEMU)"
