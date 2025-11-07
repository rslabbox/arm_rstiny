# PCIe 设备探测详细分析

## 问题背景

在你的日志中看到：
```
[ 10.414259] rk-pcie fe180000.pcie: 🎉 PCIe device detected! bus=31 dev=00 func=0
[ 10.414265] rk-pcie fe180000.pcie: ECAM virtual addr: 00000000c5a1b94b
[ 10.414267] rk-pcie fe180000.pcie: ECAM physical base: 0xf3000000
[ 10.414269] rk-pcie fe180000.pcie: ECAM busdev offset: 0x31000000
[ 10.414271] rk-pcie fe180000.pcie: Vendor ID (byte 0-1): 0x10ec
[ 10.414273] rk-pcie fe180000.pcie: Device ID (byte 2-3): 0x8125
[ 10.414275] rk-pcie fe180000.pcie: Full DWORD: 0x812510ec
```

关键点：**虚拟地址 `00000000c5a1b94b` 不是简单的 `0xf3000000 + 0x31000000`**

## PCIe 配置空间访问流程

### 1. 调用链路

```
pci_scan_bus
  └─> pci_scan_child_bus
       └─> pci_scan_slot
            └─> pci_scan_single_device
                 └─> pci_bus_read_config_dword (读取 Vendor ID)
                      └─> pci_bus_read_config_xxx
                           └─> dw_pcie_rd_other_conf  (DW PCIe 驱动实现)
```

### 2. 关键函数：`dw_pcie_rd_other_conf`

位置：`pci/controller/dwc/pcie-designware-host.c` 第 475 行

```c
static int dw_pcie_rd_other_conf(struct pci_bus *bus, unsigned int devfn,
                                 int where, int size, u32 *val)
{
    int ret;
    struct pcie_port *pp = bus->sysdata;
    struct dw_pcie *pci = to_dw_pcie_from_pp(pp);
    void __iomem *ecam_addr;

    // 第一步：调用 map_bus 获取虚拟地址
    void __iomem *addr = bus->ops->map_bus(bus, devfn, where);

    // 第二步：使用通用的配置空间读取函数
    ret = pci_generic_config_read(bus, devfn, where, size, val);

    // ... 日志打印代码 ...
    
    return ret;
}
```

### 3. 核心机制：`dw_pcie_other_conf_map_bus`

位置：`pci/controller/dwc/pcie-designware-host.c` 第 441 行

这是 **关键函数**，它实现了地址转换：

```c
static void __iomem *dw_pcie_other_conf_map_bus(struct pci_bus *bus,
                                                unsigned int devfn, int where)
{
    int type;
    u32 busdev;
    struct pcie_port *pp = bus->sysdata;
    struct dw_pcie *pci = to_dw_pcie_from_pp(pp);

    // 检查链路是否 up
    if (!dw_pcie_link_up(pci))
        return NULL;

    // 构造 busdev：编码 bus、device、function
    busdev = PCIE_ATU_BUS(bus->number) | 
             PCIE_ATU_DEV(PCI_SLOT(devfn)) |
             PCIE_ATU_FUNC(PCI_FUNC(devfn));

    // 确定配置空间类型
    if (pci_is_root_bus(bus->parent))
        type = PCIE_ATU_TYPE_CFG0;  // Type 0 配置事务
    else
        type = PCIE_ATU_TYPE_CFG1;  // Type 1 配置事务

    // 🔥🔥🔥 关键步骤：编程 iATU（内部地址转换单元）
    dw_pcie_prog_outbound_atu(pci, 0, type, pp->cfg0_base, busdev, pp->cfg0_size);

    // 返回虚拟地址：固定的基地址 + 配置空间偏移
    return pp->va_cfg0_base + where;
}
```

## iATU（内部地址转换单元）机制

### 什么是 iATU？

iATU (internal Address Translation Unit) 是 Synopsys DesignWare PCIe 控制器的硬件特性，用于：
- **将 CPU 侧的物理地址映射到 PCIe 总线地址**
- 支持配置空间、内存空间、I/O 空间的地址转换

### iATU 工作原理

```
CPU 访问地址           iATU 转换              PCIe 总线地址
  (cpu_addr)    ─────────────>           (pci_addr)
  
  0xf3000000    ────> iATU #0 ────>    Bus 31, Dev 0, Func 0
    (固定窗口)        (动态配置)         (目标设备)
```

### iATU 编程函数：`dw_pcie_prog_outbound_atu`

位置：`pci/controller/dwc/pcie-designware.c` 第 313 行

```c
void dw_pcie_prog_outbound_atu(struct dw_pcie *pci, int index, int type,
                               u64 cpu_addr, u64 pci_addr, u32 size)
{
    __dw_pcie_prog_outbound_atu(pci, 0, index, type,
                                cpu_addr, pci_addr, size);
}
```

实际执行函数（第 268 行）：

```c
static void __dw_pcie_prog_outbound_atu(struct dw_pcie *pci, u8 func_no,
                                       int index, int type, u64 cpu_addr,
                                       u64 pci_addr, u32 size)
{
    u32 retries, val;

    // CPU 地址修正（如果需要）
    if (pci->ops->cpu_addr_fixup)
        cpu_addr = pci->ops->cpu_addr_fixup(pci, cpu_addr);

    // 使用 Unroll 模式（大多数现代 IP）
    if (pci->iatu_unroll_enabled & DWC_IATU_UNROLL_EN) {
        dw_pcie_prog_outbound_atu_unroll(pci, func_no, index, type,
                                         cpu_addr, pci_addr, size);
        return;
    }

    // 旧模式：通过 viewport 寄存器访问
    dw_pcie_writel_dbi(pci, PCIE_ATU_VIEWPORT,
                       PCIE_ATU_REGION_OUTBOUND | index);
    
    // 配置源地址范围（CPU 侧）
    dw_pcie_writel_dbi(pci, PCIE_ATU_LOWER_BASE, lower_32_bits(cpu_addr));
    dw_pcie_writel_dbi(pci, PCIE_ATU_UPPER_BASE, upper_32_bits(cpu_addr));
    dw_pcie_writel_dbi(pci, PCIE_ATU_LIMIT, 
                       lower_32_bits(cpu_addr + size - 1));
    
    // 配置目标地址（PCIe 侧）
    dw_pcie_writel_dbi(pci, PCIE_ATU_LOWER_TARGET, lower_32_bits(pci_addr));
    dw_pcie_writel_dbi(pci, PCIE_ATU_UPPER_TARGET, upper_32_bits(pci_addr));
    
    // 配置事务类型和使能
    dw_pcie_writel_dbi(pci, PCIE_ATU_CR1, type | PCIE_ATU_FUNC_NUM(func_no));
    dw_pcie_writel_dbi(pci, PCIE_ATU_CR2, PCIE_ATU_ENABLE);

    // 等待 iATU 使能生效
    for (retries = 0; retries < LINK_WAIT_MAX_IATU_RETRIES; retries++) {
        val = dw_pcie_readl_dbi(pci, PCIE_ATU_CR2);
        if (val & PCIE_ATU_ENABLE)
            return;
        mdelay(LINK_WAIT_IATU);
    }
    dev_err(pci->dev, "Outbound iATU is not being enabled\n");
}
```

### Unroll 模式（第 228 行）

```c
static void dw_pcie_prog_outbound_atu_unroll(struct dw_pcie *pci, u8 func_no,
                                             int index, int type,
                                             u64 cpu_addr, u64 pci_addr,
                                             u32 size)
{
    u32 retries, val;
    u64 limit_addr = cpu_addr + size - 1;

    // 直接访问 iATU 寄存器（不需要 viewport）
    dw_pcie_writel_ob_unroll(pci, index, PCIE_ATU_UNR_LOWER_BASE,
                             lower_32_bits(cpu_addr));
    dw_pcie_writel_ob_unroll(pci, index, PCIE_ATU_UNR_UPPER_BASE,
                             upper_32_bits(cpu_addr));
    dw_pcie_writel_ob_unroll(pci, index, PCIE_ATU_UNR_LOWER_LIMIT,
                             lower_32_bits(limit_addr));
    dw_pcie_writel_ob_unroll(pci, index, PCIE_ATU_UNR_UPPER_LIMIT,
                             upper_32_bits(limit_addr));
    dw_pcie_writel_ob_unroll(pci, index, PCIE_ATU_UNR_LOWER_TARGET,
                             lower_32_bits(pci_addr));
    dw_pcie_writel_ob_unroll(pci, index, PCIE_ATU_UNR_UPPER_TARGET,
                             upper_32_bits(pci_addr));
    dw_pcie_writel_ob_unroll(pci, index, PCIE_ATU_UNR_REGION_CTRL1,
                             type | PCIE_ATU_FUNC_NUM(func_no));
    dw_pcie_writel_ob_unroll(pci, index, PCIE_ATU_UNR_REGION_CTRL2,
                             PCIE_ATU_ENABLE);

    // 验证使能
    for (retries = 0; retries < LINK_WAIT_MAX_IATU_RETRIES; retries++) {
        val = dw_pcie_readl_ob_unroll(pci, index, PCIE_ATU_UNR_REGION_CTRL2);
        if (val & PCIE_ATU_ENABLE)
            return;
        mdelay(LINK_WAIT_IATU);
    }
    dev_err(pci->dev, "Outbound iATU is not being enabled\n");
}
```

## 你的日志详细解析

### 输入参数

从日志中可以推断：
- **Bus Number**: `31` (0x1f)
- **Device Number**: `0`
- **Function Number**: `0`
- **Register Offset (where)**: `0` (读取 Vendor ID)

### busdev 编码

```c
busdev = PCIE_ATU_BUS(bus->number) | 
         PCIE_ATU_DEV(PCI_SLOT(devfn)) |
         PCIE_ATU_FUNC(PCI_FUNC(devfn));
```

从 `pcie-designware.h` 第 101-103 行：
```c
#define PCIE_ATU_BUS(x)     FIELD_PREP(GENMASK(31, 24), x)
#define PCIE_ATU_DEV(x)     FIELD_PREP(GENMASK(23, 19), x)
#define PCIE_ATU_FUNC(x)    FIELD_PREP(GENMASK(18, 16), x)
```

计算过程：
```
busdev = (31 << 24) | (0 << 19) | (0 << 16)
       = 0x1f000000 | 0x00000000 | 0x00000000
       = 0x1f000000
```

**等等！你的日志显示 `0x31000000`？**

实际上应该是：
```
31 (decimal) = 0x1f
busdev = 0x1f << 24 = 0x1f000000
```

但你的日志显示 `0x31000000`，这表明 bus number 是 `0x31` (49 decimal)，而不是 31 decimal。

### iATU 配置

调用：
```c
dw_pcie_prog_outbound_atu(pci, 0, PCIE_ATU_TYPE_CFG0, 
                         pp->cfg0_base, busdev, pp->cfg0_size);
```

参数：
- **index**: `0` (iATU 窗口 0)
- **type**: `PCIE_ATU_TYPE_CFG0` = `0x4` (Type 0 配置事务)
- **cpu_addr**: `pp->cfg0_base` = `0xf3000000` (CPU 侧物理地址)
- **pci_addr**: `busdev` = `0x31000000` (PCIe 总线地址，编码 BDF)
- **size**: `pp->cfg0_size` (配置空间窗口大小)

**iATU 配置效果**：
```
当 CPU 访问 [0xf3000000, 0xf3000000 + size) 范围时，
iATU 将其转换为 Type 0 配置事务，目标为 Bus 0x31, Dev 0, Func 0
```

### 虚拟地址映射

```c
return pp->va_cfg0_base + where;
```

- **pp->va_cfg0_base**: 这是 `0xf3000000` 物理地址对应的**虚拟地址**
- **where**: `0` (Vendor ID 寄存器偏移)

Linux 内核通过 `ioremap` 或 `devm_pci_remap_cfgspace` 将物理地址 `0xf3000000` 映射到虚拟地址空间。你看到的 `00000000c5a1b94b` 就是这个虚拟地址。

### 为什么虚拟地址是随机的？

Linux 内核出于安全考虑，使用 **KASLR (Kernel Address Space Layout Randomization)**，每次启动时虚拟地址都是随机的。

- **物理地址**: `0xf3000000` (固定，来自设备树)
- **虚拟地址**: `0xc5a1b94b` (随机，内核分配)

## 完整流程图

```
1. PCI 子系统扫描
   └─> pci_scan_slot(bus=31, devfn=0)
       └─> pci_bus_read_config_dword(where=0x00)  // 读 Vendor ID

2. 调用驱动的读函数
   └─> dw_pcie_rd_other_conf(bus, devfn=0, where=0)

3. 获取虚拟地址
   └─> dw_pcie_other_conf_map_bus(bus, devfn=0, where=0)
       ├─> busdev = 0x31000000  // Bus 0x31, Dev 0, Func 0
       ├─> dw_pcie_prog_outbound_atu:
       │   ├─> CPU 地址范围: 0xf3000000 ~ 0xf3000000 + size
       │   └─> PCIe 目标: Type 0 CFG, BDF = 0x31:0.0
       └─> return va_cfg0_base + 0 = 0xc5a1b94b

4. 读取配置空间
   └─> pci_generic_config_read()
       └─> readl(0xc5a1b94b)  // CPU 读虚拟地址
           └─> MMU 转换为物理地址 0xf3000000
               └─> PCIe 控制器 iATU 捕获
                   └─> 生成 Type 0 配置 TLP
                       └─> 目标: Bus 0x31, Dev 0, Func 0, Reg 0x00

5. 设备响应
   └─> RTL8125 返回 Vendor ID = 0x10ec, Device ID = 0x8125
       └─> 完成 TLP 返回数据 0x812510ec

6. 结果
   └─> *val = 0x812510ec
```

## 关键数据结构

### pcie_port (pcie-designware.h 第 188 行)

```c
struct pcie_port {
    u64             cfg0_base;      // 配置空间物理基地址 (0xf3000000)
    void __iomem    *va_cfg0_base;  // 配置空间虚拟基地址 (0xc5a1b94b...)
    u32             cfg0_size;      // 配置空间大小
    // ...
};
```

### iATU 寄存器定义 (pcie-designware.h)

```c
// 通过 viewport 访问
#define PCIE_ATU_VIEWPORT       0x900
#define PCIE_ATU_CR1            0x904
#define PCIE_ATU_CR2            0x908
#define PCIE_ATU_LOWER_BASE     0x90C
#define PCIE_ATU_UPPER_BASE     0x910
#define PCIE_ATU_LIMIT          0x914
#define PCIE_ATU_LOWER_TARGET   0x918
#define PCIE_ATU_UPPER_TARGET   0x91C

// Unroll 模式（直接访问）
#define PCIE_ATU_UNR_REGION_CTRL1    0x00
#define PCIE_ATU_UNR_REGION_CTRL2    0x04
#define PCIE_ATU_UNR_LOWER_BASE      0x08
#define PCIE_ATU_UNR_UPPER_BASE      0x0C
#define PCIE_ATU_UNR_LOWER_LIMIT     0x10
#define PCIE_ATU_UNR_LOWER_TARGET    0x14
#define PCIE_ATU_UNR_UPPER_TARGET    0x18
```

## 为什么不直接使用 ECAM 地址？

标准 ECAM (Enhanced Configuration Access Mechanism) 定义：
```
ECAM_ADDR = ECAM_BASE + (Bus << 20) + (Dev << 15) + (Func << 12) + Reg
```

但 DesignWare PCIe 控制器**不支持标准 ECAM**，原因：
1. **硬件限制**: 没有足够大的连续地址空间
2. **灵活性**: iATU 允许动态映射，同一个窗口可以访问不同总线
3. **效率**: 可以复用少量 iATU 窗口访问大量设备

因此，Linux 驱动使用**动态 iATU 编程**：
- 每次访问前，重新配置 iATU 指向目标设备
- 使用固定的虚拟地址窗口 (pp->va_cfg0_base)
- 通过 iATU 将访问路由到不同的 BDF

## ⚠️ 关键发现：为什么直接访问 0xf3000000 读不到数据？

### 问题现象

**直接访问物理地址 `0xf3000000` 无法读取到任何有效数据！**

### 根本原因

`0xf3000000` **不是真实的 PCIe 配置空间物理地址**，它只是一个 **iATU 窗口的基地址**。

```
❌ 错误理解：
0xf3000000 是设备的配置空间 → 直接读取就能得到数据

✅ 正确理解：
0xf3000000 是 iATU 的输入窗口 → 需要先配置 iATU → iATU 生成 PCIe TLP
```

### 详细解释

#### 1. iATU 是硬件地址转换单元

```
┌─────────────────────────────────────────────────────────┐
│                    CPU 内存总线                           │
└─────────────────┬───────────────────────────────────────┘
                  │
                  │ 访问 0xf3000000 + offset
                  ↓
┌─────────────────────────────────────────────────────────┐
│              PCIe 控制器 (DW PCIe Core)                  │
│  ┌───────────────────────────────────────────────┐      │
│  │         iATU (地址转换逻辑)                    │      │
│  │                                                │      │
│  │  IF (地址在 [0xf3000000, 0xf3000000+size))    │      │
│  │    AND (iATU 已配置)                           │      │
│  │  THEN                                          │      │
│  │    生成 PCIe Configuration TLP                 │      │
│  │    目标 = busdev (从 iATU 寄存器读取)          │      │
│  │  ELSE                                          │      │
│  │    返回全 F (0xFFFFFFFF)                       │      │
│  └───────────────────────────────────────────────┘      │
└─────────────────┬───────────────────────────────────────┘
                  │
                  │ PCIe TLP (Type 0/1 Config)
                  ↓
┌─────────────────────────────────────────────────────────┐
│                  PCIe 链路 / 设备                        │
└─────────────────────────────────────────────────────────┘
```

#### 2. iATU 必须被正确配置

在访问 `0xf3000000` **之前**，必须先配置 iATU 寄存器：

```c
// 伪代码示例
void configure_iatu_before_access() {
    // 1. 设置源地址范围（CPU 侧）
    writel(0xf3000000, DBI_BASE + PCIE_ATU_LOWER_BASE);
    writel(0xf3000000 + size - 1, DBI_BASE + PCIE_ATU_LIMIT);
    
    // 2. 设置目标地址（PCIe 总线地址，编码 BDF）
    u32 busdev = (bus << 24) | (dev << 19) | (func << 16);
    writel(busdev, DBI_BASE + PCIE_ATU_LOWER_TARGET);
    
    // 3. 设置事务类型
    writel(PCIE_ATU_TYPE_CFG0, DBI_BASE + PCIE_ATU_CR1);
    
    // 4. 使能 iATU
    writel(PCIE_ATU_ENABLE, DBI_BASE + PCIE_ATU_CR2);
    
    // 5. 等待 iATU 生效
    while (!(readl(DBI_BASE + PCIE_ATU_CR2) & PCIE_ATU_ENABLE));
}

// 现在才能访问
u32 vendor_device_id = readl(0xf3000000);  // ✅ 这时才有效
```

#### 3. 如果不配置 iATU 会怎样？

```rust
// ❌ 错误做法
let ecam_base = 0xf3000000 as *const u32;
let value = unsafe { ptr::read_volatile(ecam_base) };
// 结果: value = 0xFFFFFFFF (无效数据)

// ✅ 正确做法
// 1. 先配置 iATU
program_iatu(0, bus, dev, func);

// 2. 再访问相同地址
let value = unsafe { ptr::read_volatile(ecam_base) };
// 结果: value = 0x812510ec (正确的 Vendor/Device ID)
```

#### 4. iATU 寄存器在哪里？

iATU 寄存器在 **DBI (DesignWare Bus Interface)** 空间：

```
DBI 基地址 (从设备树获取): 0xfe180000  (RK3588 的 PCIe 控制器)

iATU 寄存器偏移:
  0x900: PCIE_ATU_VIEWPORT
  0x904: PCIE_ATU_CR1
  0x908: PCIE_ATU_CR2
  0x90C: PCIE_ATU_LOWER_BASE
  0x910: PCIE_ATU_UPPER_BASE
  0x914: PCIE_ATU_LIMIT
  0x918: PCIE_ATU_LOWER_TARGET
  0x91C: PCIE_ATU_UPPER_TARGET

实际寄存器地址 = 0xfe180000 + 偏移
例如: PCIE_ATU_CR2 = 0xfe180908
```

#### 5. 设备树中的配置

```dts
pcie@fe180000 {
    compatible = "rockchip,rk3588-pcie", "snps,dw-pcie";
    reg = <0x0 0xfe180000 0x0 0x10000>,    /* DBI 空间 */
          <0x9 0x00000000 0x0 0x100000>,   /* 配置空间窗口 (CPU 侧) */
          <0x9 0x00100000 0x0 0x100000>;   /* IO/MEM 窗口 */
    reg-names = "dbi", "config", "apb";
    
    ranges = <0x01000000 0x0 0xf0100000 0x9 0xf0100000 0x0 0x00100000>,
             <0x02000000 0x0 0xf0200000 0x9 0xf0200000 0x0 0x0fe00000>,
             <0x03000000 0x0 0x40000000 0x9 0x40000000 0x0 0xb0000000>;
};
```

从这里可以看到：
- **DBI 基地址**: `0xfe180000` (用于配置 iATU)
- **配置空间窗口**: `0x900000000` (CPU 侧，iATU 输入)

但你的日志显示 `pp->cfg0_base = 0xf3000000`，这可能是经过某种转换后的地址。

### 实际测试验证

```rust
// 测试代码
fn test_pcie_access() {
    let dbi_base = 0xfe180000;
    let cfg_base = 0xf3000000;
    
    // ❌ 测试1: 不配置 iATU 直接访问
    info!("Test 1: 直接访问配置空间 (未配置 iATU)");
    let val1 = unsafe { ptr::read_volatile(cfg_base as *const u32) };
    info!("  读取结果: 0x{:08x}", val1);  // 预期: 0xFFFFFFFF
    
    // ✅ 测试2: 配置 iATU 后访问
    info!("Test 2: 配置 iATU 后访问");
    program_outbound_atu(dbi_base, 0, 0x04, cfg_base, 0x31000000, 0x100000);
    let val2 = unsafe { ptr::read_volatile(cfg_base as *const u32) };
    info!("  读取结果: 0x{:08x}", val2);  // 预期: 0x812510ec
}

fn program_outbound_atu(
    dbi_base: usize,
    index: u32,
    cfg_type: u32,
    cpu_addr: usize,
    pci_addr: u32,
    size: usize,
) {
    // 选择 iATU 区域
    unsafe {
        ptr::write_volatile((dbi_base + 0x900) as *mut u32, index);
        
        // 配置源地址
        ptr::write_volatile((dbi_base + 0x90C) as *mut u32, cpu_addr as u32);
        ptr::write_volatile((dbi_base + 0x910) as *mut u32, (cpu_addr >> 32) as u32);
        ptr::write_volatile((dbi_base + 0x914) as *mut u32, (cpu_addr + size - 1) as u32);
        
        // 配置目标地址
        ptr::write_volatile((dbi_base + 0x918) as *mut u32, pci_addr);
        ptr::write_volatile((dbi_base + 0x91C) as *mut u32, 0);
        
        // 配置类型和使能
        ptr::write_volatile((dbi_base + 0x904) as *mut u32, cfg_type);
        ptr::write_volatile((dbi_base + 0x908) as *mut u32, 0x8000_0000); // Enable
        
        // 等待使能生效
        loop {
            let cr2 = ptr::read_volatile((dbi_base + 0x908) as *const u32);
            if cr2 & 0x8000_0000 != 0 {
                break;
            }
        }
    }
}
```

### 为什么 Linux 驱动可以工作？

因为 Linux 驱动在 `dw_pcie_other_conf_map_bus()` 中，**每次访问前都会调用 `dw_pcie_prog_outbound_atu()`** 配置 iATU！

```c
// 这是 Linux 的正确流程
static void __iomem *dw_pcie_other_conf_map_bus(...) {
    // 1. 计算 busdev
    busdev = PCIE_ATU_BUS(bus->number) | ...;
    
    // 2. 🔥 关键！配置 iATU
    dw_pcie_prog_outbound_atu(pci, 0, type, pp->cfg0_base, busdev, pp->cfg0_size);
    
    // 3. 返回固定虚拟地址
    return pp->va_cfg0_base + where;
}
```

### 总结

| 地址类型 | 地址值 | 作用 | 能否直接读取 |
|---------|--------|------|-------------|
| DBI 基地址 | `0xfe180000` | 配置 PCIe 控制器寄存器 | ✅ 可以 |
| iATU 窗口基地址 | `0xf3000000` | iATU 输入地址范围 | ❌ 需先配置 iATU |
| 虚拟地址 | `0xc5a1b94b` | 内核映射的虚拟地址 | ❌ 需先配置 iATU |
| PCIe 总线地址 | `0x31000000` (BDF编码) | iATU 输出目标 | N/A |

**关键教训**：
1. `0xf3000000` 是 iATU 的**触发地址**，不是数据存储地址
2. 必须先配置 iATU，才能通过这个地址访问 PCIe 设备
3. 每次访问不同设备时，都需要重新配置 iATU
4. iATU 配置寄存器在 DBI 空间 (`0xfe180000 + 0x900~0x91C`)

## 总结

1. **不是直接地址计算**：虚拟地址 `0xc5a1b94b` 不是 `0xf3000000 + 0x31000000`

2. **iATU 动态转换**：每次配置空间访问前，驱动动态配置 iATU 窗口

3. **固定窗口，动态目标**：
   - CPU 始终访问固定地址范围 (pp->va_cfg0_base)
   - iATU 将其映射到不同的 PCIe 设备

4. **三层地址转换**：
   ```
   虚拟地址 ─MMU─> 物理地址 ─iATU─> PCIe 配置空间 TLP
   0xc5a1b94b     0xf3000000      Bus 0x31, Dev 0, Func 0
   ```

5. **为什么这么设计**：
   - 节省地址空间（只需要一个小窗口）
   - 支持大量设备（iATU 动态映射）
   - 符合 PCIe 协议（生成正确的 TLP）

## 你的 Rust 实现建议

在你的 `rstiny_arm` 中实现 PCIe 设备探测时，需要：

### 1. 定义 iATU 寄存器常量

```rust
// PCIe 控制器寄存器地址
const DBI_BASE: usize = 0xfe180000;          // DBI 基地址 (从设备树获取)
const CFG_WINDOW_BASE: usize = 0xf3000000;   // 配置空间窗口基地址

// iATU 寄存器偏移
const PCIE_ATU_VIEWPORT: usize = 0x900;
const PCIE_ATU_CR1: usize = 0x904;
const PCIE_ATU_CR2: usize = 0x908;
const PCIE_ATU_LOWER_BASE: usize = 0x90C;
const PCIE_ATU_UPPER_BASE: usize = 0x910;
const PCIE_ATU_LIMIT: usize = 0x914;
const PCIE_ATU_LOWER_TARGET: usize = 0x918;
const PCIE_ATU_UPPER_TARGET: usize = 0x91C;

// iATU 类型
const PCIE_ATU_TYPE_CFG0: u32 = 0x4;
const PCIE_ATU_TYPE_CFG1: u32 = 0x5;
const PCIE_ATU_TYPE_MEM: u32 = 0x0;
const PCIE_ATU_TYPE_IO: u32 = 0x2;

// iATU 控制位
const PCIE_ATU_ENABLE: u32 = 1 << 31;
```

### 2. 实现 iATU 配置函数

```rust
/// 配置 outbound iATU
fn program_outbound_atu(
    dbi_base: usize,
    index: u32,
    atu_type: u32,
    cpu_addr: u64,
    pci_addr: u64,
    size: u64,
) -> Result<(), &'static str> {
    unsafe {
        // 1. 选择 iATU 区域 (region/viewport)
        ptr::write_volatile(
            (dbi_base + PCIE_ATU_VIEWPORT) as *mut u32,
            index & 0xF  // 选择 outbound region
        );
        
        // 2. 配置源地址范围 (CPU 侧物理地址)
        let lower_base = cpu_addr as u32;
        let upper_base = (cpu_addr >> 32) as u32;
        let limit = ((cpu_addr + size - 1) & 0xFFFFFFFF) as u32;
        
        ptr::write_volatile((dbi_base + PCIE_ATU_LOWER_BASE) as *mut u32, lower_base);
        ptr::write_volatile((dbi_base + PCIE_ATU_UPPER_BASE) as *mut u32, upper_base);
        ptr::write_volatile((dbi_base + PCIE_ATU_LIMIT) as *mut u32, limit);
        
        // 3. 配置目标地址 (PCIe 总线地址)
        let lower_target = pci_addr as u32;
        let upper_target = (pci_addr >> 32) as u32;
        
        ptr::write_volatile((dbi_base + PCIE_ATU_LOWER_TARGET) as *mut u32, lower_target);
        ptr::write_volatile((dbi_base + PCIE_ATU_UPPER_TARGET) as *mut u32, upper_target);
        
        // 4. 配置事务类型
        ptr::write_volatile((dbi_base + PCIE_ATU_CR1) as *mut u32, atu_type);
        
        // 5. 使能 iATU
        ptr::write_volatile((dbi_base + PCIE_ATU_CR2) as *mut u32, PCIE_ATU_ENABLE);
        
        // 6. 等待 iATU 使能生效 (最多重试 5 次)
        for _ in 0..5 {
            let cr2 = ptr::read_volatile((dbi_base + PCIE_ATU_CR2) as *const u32);
            if cr2 & PCIE_ATU_ENABLE != 0 {
                return Ok(());
            }
            // 延时 9ms
            arch::delay_ms(9);
        }
        
        Err("iATU enable timeout")
    }
}
```

### 3. 实现配置空间访问

```rust
/// PCIe 配置空间访问结构
pub struct PcieConfigAccess {
    dbi_base: usize,
    cfg_window_base: usize,
    cfg_window_size: usize,
}

impl PcieConfigAccess {
    pub fn new(dbi_base: usize, cfg_window_base: usize, cfg_window_size: usize) -> Self {
        Self {
            dbi_base,
            cfg_window_base,
            cfg_window_size,
        }
    }
    
    /// 读取配置空间 DWORD
    pub fn read_config_dword(&self, bus: u8, dev: u8, func: u8, reg: u16) -> Result<u32, &'static str> {
        // 1. 编码 busdev
        let busdev: u64 = ((bus as u64) << 24) | ((dev as u64) << 19) | ((func as u64) << 16);
        
        // 2. 确定配置空间类型
        let cfg_type = if bus == 0 {
            PCIE_ATU_TYPE_CFG0  // Type 0: 访问同一总线上的设备
        } else {
            PCIE_ATU_TYPE_CFG1  // Type 1: 访问下游总线上的设备
        };
        
        // 3. 🔥 关键步骤：配置 iATU
        program_outbound_atu(
            self.dbi_base,
            0,  // 使用 iATU region 0
            cfg_type,
            self.cfg_window_base as u64,
            busdev,
            self.cfg_window_size as u64,
        )?;
        
        // 4. 通过配置窗口读取 (现在 iATU 已经配置好了)
        let addr = (self.cfg_window_base + reg as usize) as *const u32;
        let value = unsafe { ptr::read_volatile(addr) };
        
        Ok(value)
    }
    
    /// 写入配置空间 DWORD
    pub fn write_config_dword(&self, bus: u8, dev: u8, func: u8, reg: u16, value: u32) -> Result<(), &'static str> {
        let busdev: u64 = ((bus as u64) << 24) | ((dev as u64) << 19) | ((func as u64) << 16);
        
        let cfg_type = if bus == 0 {
            PCIE_ATU_TYPE_CFG0
        } else {
            PCIE_ATU_TYPE_CFG1
        };
        
        program_outbound_atu(
            self.dbi_base,
            0,
            cfg_type,
            self.cfg_window_base as u64,
            busdev,
            self.cfg_window_size as u64,
        )?;
        
        let addr = (self.cfg_window_base + reg as usize) as *mut u32;
        unsafe { ptr::write_volatile(addr, value) };
        
        Ok(())
    }
    
    /// 读取 Vendor ID 和 Device ID
    pub fn read_vendor_device_id(&self, bus: u8, dev: u8, func: u8) -> Result<(u16, u16), &'static str> {
        let val = self.read_config_dword(bus, dev, func, 0x00)?;
        
        // 检查是否有效
        if val == 0xFFFFFFFF || val == 0 {
            return Err("No device present");
        }
        
        let vendor_id = (val & 0xFFFF) as u16;
        let device_id = ((val >> 16) & 0xFFFF) as u16;
        
        Ok((vendor_id, device_id))
    }
}
```

### 4. 实现设备扫描

```rust
/// 扫描 PCIe 总线
pub fn scan_pcie_bus(pcie: &PcieConfigAccess) {
    info!("=== 开始扫描 PCIe 总线 ===");
    
    // 扫描总线 0-255
    for bus in 0..=255u8 {
        // 每个总线最多 32 个设备
        for dev in 0..32u8 {
            // 每个设备最多 8 个功能
            for func in 0..8u8 {
                match pcie.read_vendor_device_id(bus, dev, func) {
                    Ok((vendor_id, device_id)) => {
                        info!(
                            "🎉 发现设备: Bus {:02x}, Dev {:02x}, Func {:x} - {:04x}:{:04x}",
                            bus, dev, func, vendor_id, device_id
                        );
                        
                        // 检查是否是网卡
                        if let Ok(class_code) = pcie.read_config_dword(bus, dev, func, 0x08) {
                            let class = (class_code >> 24) as u8;
                            let subclass = ((class_code >> 16) & 0xFF) as u8;
                            
                            if class == 0x02 {  // Network controller
                                info!("  -> 这是一个网络控制器!");
                            }
                        }
                        
                        // 如果不是多功能设备，跳过后续功能号
                        if func == 0 {
                            if let Ok(header_type) = pcie.read_config_dword(bus, dev, func, 0x0C) {
                                let is_multi_function = ((header_type >> 16) & 0x80) != 0;
                                if !is_multi_function {
                                    break;
                                }
                            }
                        }
                    }
                    Err(_) => {
                        // 没有设备，继续
                        if func == 0 {
                            break;  // 如果功能 0 不存在，跳过该设备
                        }
                    }
                }
            }
        }
    }
    
    info!("=== PCIe 总线扫描完成 ===");
}
```

### 5. 使用示例

```rust
pub fn init_pcie() {
    // 从设备树或硬编码获取地址
    let dbi_base = 0xfe180000;
    let cfg_window_base = 0xf3000000;
    let cfg_window_size = 0x100000;  // 1MB
    
    // 创建配置空间访问对象
    let pcie = PcieConfigAccess::new(dbi_base, cfg_window_base, cfg_window_size);
    
    // 扫描总线
    scan_pcie_bus(&pcie);
    
    // 直接访问特定设备 (如果知道 BDF)
    match pcie.read_vendor_device_id(31, 0, 0) {  // Bus 31, Dev 0, Func 0
        Ok((vendor_id, device_id)) => {
            info!("设备 31:00.0 - {:04x}:{:04x}", vendor_id, device_id);
            
            if vendor_id == 0x10ec && device_id == 0x8125 {
                info!("检测到 Realtek RTL8125 网卡!");
                // 初始化驱动...
            }
        }
        Err(e) => warn!("无法访问设备 31:00.0: {}", e),
    }
}
```

### 6. 完整的验证代码

```rust
/// 测试 iATU 配置是否工作
pub fn test_iatu() {
    let dbi_base = 0xfe180000;
    let cfg_base = 0xf3000000;
    
    info!("=== 测试 iATU 配置 ===");
    
    // 测试 1: 不配置 iATU，直接访问
    info!("Test 1: 直接访问 (未配置 iATU)");
    let val1 = unsafe { ptr::read_volatile(cfg_base as *const u32) };
    info!("  结果: 0x{:08x} (预期: 0xFFFFFFFF)", val1);
    
    // 测试 2: 配置 iATU 后访问
    info!("Test 2: 配置 iATU 后访问 Bus 31, Dev 0, Func 0");
    let busdev: u64 = (31u64 << 24) | (0u64 << 19) | (0u64 << 16);
    
    match program_outbound_atu(
        dbi_base,
        0,
        PCIE_ATU_TYPE_CFG0,
        cfg_base as u64,
        busdev,
        0x100000,
    ) {
        Ok(_) => {
            let val2 = unsafe { ptr::read_volatile(cfg_base as *const u32) };
            info!("  结果: 0x{:08x}", val2);
            
            if val2 != 0xFFFFFFFF && val2 != 0 {
                let vendor = val2 & 0xFFFF;
                let device = (val2 >> 16) & 0xFFFF;
                info!("  ✅ 成功! Vendor: 0x{:04x}, Device: 0x{:04x}", vendor, device);
            } else {
                warn!("  ❌ 读取失败，可能链路未连接");
            }
        }
        Err(e) => error!("  ❌ iATU 配置失败: {}", e),
    }
}
```

### 7. 注意事项

1. **必须先配置 iATU 才能访问**：每次访问不同的 BDF 都需要重新配置

2. **虚拟地址映射**：在裸机环境中，物理地址就是虚拟地址（如果没开启 MMU）

3. **设备树地址**：从设备树中正确读取 DBI 和配置窗口的地址

4. **错误处理**：iATU 配置失败时要有超时机制

5. **总线编号**：你的日志显示 bus=31 (0x1f)，但 busdev 是 0x31000000，需要验证实际的总线编号

### 8. 调试技巧

```rust
/// 打印 iATU 配置状态
pub fn dump_iatu_config(dbi_base: usize, region: u32) {
    unsafe {
        ptr::write_volatile((dbi_base + PCIE_ATU_VIEWPORT) as *mut u32, region);
        
        let cr1 = ptr::read_volatile((dbi_base + PCIE_ATU_CR1) as *const u32);
        let cr2 = ptr::read_volatile((dbi_base + PCIE_ATU_CR2) as *const u32);
        let lower_base = ptr::read_volatile((dbi_base + PCIE_ATU_LOWER_BASE) as *const u32);
        let upper_base = ptr::read_volatile((dbi_base + PCIE_ATU_UPPER_BASE) as *const u32);
        let limit = ptr::read_volatile((dbi_base + PCIE_ATU_LIMIT) as *const u32);
        let lower_target = ptr::read_volatile((dbi_base + PCIE_ATU_LOWER_TARGET) as *const u32);
        let upper_target = ptr::read_volatile((dbi_base + PCIE_ATU_UPPER_TARGET) as *const u32);
        
        info!("iATU Region {} 配置:", region);
        info!("  CR1 (Type):       0x{:08x}", cr1);
        info!("  CR2 (Enable):     0x{:08x} {}", cr2, if cr2 & PCIE_ATU_ENABLE != 0 { "✅" } else { "❌" });
        info!("  Base:             0x{:08x}_{:08x}", upper_base, lower_base);
        info!("  Limit:            0x{:08x}", limit);
        info!("  Target:           0x{:08x}_{:08x}", upper_target, lower_target);
    }
}
```

这个实现完全复刻了 Linux 驱动的逻辑，确保每次访问前都正确配置 iATU！

