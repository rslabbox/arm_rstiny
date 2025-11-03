# xtask 工具

这是一个用于构建和打包 rstiny_arm 项目的自定义构建工具。

## 功能

### mkimage

将编译生成的二进制文件打包成 uImage 格式，适用于 U-Boot 引导加载器。

## 使用方法

```bash
# 构建项目并生成 uImage
cargo xtask mkimage
```

## 要求

- 系统需要安装 `mkimage` 工具（通常包含在 u-boot-tools 包中）
  - Ubuntu/Debian: `sudo apt install u-boot-tools`
  - Arch Linux: `sudo pacman -S uboot-tools`
  - macOS: `brew install u-boot-tools`

## 输出

生成的 uImage 文件位于：`target/rstiny_arm.uimage`

## mkimage 参数说明

- **架构 (-A)**: arm64
- **操作系统 (-O)**: linux
- **镜像类型 (-T)**: kernel
- **压缩类型 (-C)**: none
- **加载地址 (-a)**: 0x40080000
- **入口地址 (-e)**: 0x40080000
- **镜像名称 (-n)**: RSTiny ARM Kernel

你可以根据需要在 `xtask/src/main.rs` 中修改这些参数。
