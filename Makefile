# ARM RSTiny - Rust Bare Metal OS Makefile

# 项目配置
PROJECT_NAME = arm_rstiny
MODE := release
TARGET = aarch64-unknown-none-softfloat
LOG := info
DISK_IMG := test.img

TOOL_PATH = tools/orangepi5

kernel_elf = target/$(TARGET)/$(MODE)/$(PROJECT_NAME)
kernel_bin = $(kernel_elf).bin
kernel_img = $(kernel_elf).img
kernel_uimg = $(kernel_elf).uimg
kernel_asm = $(kernel_elf)_asm.txt

ifeq ($(MODE), release)
	MODE_ARG := --release
endif

# 编译选项
CARGO_FLAGS = $(MODE_ARG) --target $(TARGET)

export LOG

.PHONY: all build run clean

# 默认目标
all: build

# 编译项目
build: 
	@echo "Building $(PROJECT_NAME)..."
	cargo build $(CARGO_FLAGS)
	@echo "Build completed: $(kernel_elf)"
	
	@echo "Generating $(kernel_bin)..."
	@rust-objcopy -O binary $(kernel_elf) $(kernel_bin)

	@echo "Dump $(kernel_asm)"
	@rust-objdump -d --print-imm-hex $(kernel_elf) > $(kernel_asm)

	@mkimage -A arm64 -O linux -T kernel -C none -a 0x400000 -e 0x400000 -n "$(PROJECT_NAME)" -d $(kernel_bin) $(kernel_uimg)
	@echo "Generated: $(kernel_uimg)"

# 调试模式运行
debug: build
	@echo "Starting $(PROJECT_NAME) in QEMU with GDB support..."
	@echo "Connect with: gdb-multiarch -ex 'target remote :1234' $(kernel_elf)"
	@echo "Press Ctrl+A then X to exit QEMU"
	$(QEMU) $(QEMU_ARGS) -s -S

# 清理编译产物
clean:
	@echo "Cleaning build artifacts..."
	cargo clean

flash: $(kernel_bin)
	@echo "Flash $(PROJECT_NAME) to Orange Pi 5..."
	sudo bash $(TOOL_PATH)/make_flash.sh uimg=$(kernel_uimg)
