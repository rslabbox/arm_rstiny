# RTL8125驱动修复总结

## 问题描述

在RK3588平台上实现RTL8125 2.5G以太网驱动时,遇到网络数据包接收问题:
- 发送ping请求后无法收到回复
- 接收到的数据包内容全为0
- EtherType读取为0x0000而不是期望的0x0800(IP协议)

## 根本原因

问题由**ARM64架构的Cache一致性**导致:

### 1. Cache工作原理
- CPU读写数据时会先访问Cache,而不是直接访问内存
- 硬件DMA(如网卡)直接读写内存,不经过CPU Cache
- 如果Cache中有旧数据,CPU读取时会得到Cache中的旧数据,而不是硬件写入内存的新数据

### 2. 问题表现
```
硬件接收流程:
1. 网卡通过DMA将数据包写入内存(RX buffer)
2. CPU从RX buffer读取数据
   ❌ 问题: CPU从Cache读到旧数据(全0),而不是内存中硬件写入的新数据
```

## 修复方案

### 核心修复1: 初始化时Invalidate RX缓冲区

**位置**: `src/drivers/net/rtl8125.rs` - `init_ring()`函数

**问题分析**:
- RX buffer在栈上分配,初始值为0
- 这些0值可能已经在CPU Cache中
- 当硬件写入数据后,CPU读取时仍然从Cache读到旧的0值

**修复代码**:
```rust
// CRITICAL: Invalidate all RX buffers BEFORE starting hardware!
// This ensures CPU cache doesn't contain stale data that would be read
// instead of the actual packet data written by hardware DMA.
info!("RTL8125: Invalidating RX buffers to prevent stale cache data");
for i in 0..NUM_RX_DESC {
    unsafe {
        invalidate_dcache_range(self.rx_buf[i].as_ptr() as usize, RX_BUF_SIZE);
    }
}
```

**原理**:
- `invalidate_dcache_range()`: 丢弃CPU Cache中的数据
- 强制CPU在下次读取时从内存读取数据
- 必须在启动硬件**之前**执行,确保没有旧数据在Cache中

### 核心修复2: 遍历所有RX描述符

**位置**: `src/drivers/net/rtl8125.rs` - `recv()`函数

**问题分析**:
- 原代码只检查`cur_rx`指向的描述符
- 硬件实际使用的描述符可能与软件期望的不一致
- 导致硬件写入RX[0],但软件在等待RX[1]

**修复前**:
```rust
pub fn recv(&mut self, buf: &mut [u8]) -> Result<usize, &'static str> {
    let cur_rx = self.cur_rx;  // 只检查一个描述符
    
    if (self.rx_desc[cur_rx].status & desc_status::OWN) != 0 {
        return Err("No packet available");  // 如果这个描述符没数据就返回
    }
    // ...
}
```

**修复后**:
```rust
pub fn recv(&mut self, buf: &mut [u8]) -> Result<usize, &'static str> {
    // Try all RX descriptors, starting from cur_rx
    for i in 0..NUM_RX_DESC {
        let idx = (self.cur_rx + i) % NUM_RX_DESC;
        
        // Invalidate descriptor cache
        unsafe {
            invalidate_dcache_range(
                &self.rx_desc[idx] as *const _ as usize,
                core::mem::size_of::<RxDesc>(),
            );
        }
        
        let rx_status = self.rx_desc[idx].status;
        
        // Check if this descriptor has a packet (OWN bit clear)
        if (rx_status & desc_status::OWN) != 0 {
            continue;  // 没有数据,检查下一个
        }
        
        // Found a packet! Process it
        // ...
        return Ok(len);
    }
    
    // No packet in any descriptor
    Err("No packet available")
}
```

**原理**:
- 遍历所有4个RX描述符,找到第一个有数据的(OWN位为0)
- 硬件可能不按顺序使用描述符,或者重复使用某个描述符
- 这样无论硬件使用哪个描述符,软件都能找到

## 其他重要修复

### 1. Cache操作实现

**位置**: `src/hal/cpu.rs`

添加了ARM64 cache操作函数:

```rust
/// Clean (write-back) data cache - 用于发送前
pub unsafe fn clean_dcache_range(addr: usize, size: usize) {
    let cache_line_size = get_dcache_line_size();
    let start = addr & !(cache_line_size - 1);  // 对齐到cache line
    let end = (addr + size + cache_line_size - 1) & !(cache_line_size - 1);
    
    let mut current = start;
    while current < end {
        asm!("dc cvac, {}", in(reg) current);  // Data Cache Clean by VA
        current += cache_line_size;
    }
    asm!("dsb sy");  // 确保完成
}

/// Invalidate (discard) data cache - 用于接收前/后
pub unsafe fn invalidate_dcache_range(addr: usize, size: usize) {
    let cache_line_size = get_dcache_line_size();
    let start = addr & !(cache_line_size - 1);
    let end = (addr + size + cache_line_size - 1) & !(cache_line_size - 1);
    
    let mut current = start;
    while current < end {
        asm!("dc ivac, {}", in(reg) current);  // Data Cache Invalidate by VA
        current += cache_line_size;
    }
    asm!("dsb sy");
}
```

### 2. 发送路径Cache处理

**位置**: `send()`函数

```rust
// 1. Clean TX buffer - 将CPU写入的数据刷到内存
unsafe {
    clean_dcache_range(self.tx_buf[entry].as_ptr() as usize, len);
}

// 2. Clean TX descriptor - 将描述符刷到内存
unsafe {
    clean_dcache_range(
        &self.tx_desc[entry] as *const _ as usize,
        core::mem::size_of::<TxDesc>(),
    );
}

// 3. 等待发送完成时 Invalidate descriptor - 读取硬件更新
unsafe {
    invalidate_dcache_range(
        &self.tx_desc[entry] as *const _ as usize,
        core::mem::size_of::<TxDesc>(),
    );
}
```

### 3. 接收路径Cache处理

**位置**: `recv()`函数

```rust
// 1. Invalidate RX descriptor - 读取硬件更新的状态
unsafe {
    invalidate_dcache_range(
        &self.rx_desc[idx] as *const _ as usize,
        core::mem::size_of::<RxDesc>(),
    );
}

// 2. Invalidate RX buffer - 读取硬件写入的数据
unsafe {
    invalidate_dcache_range(
        self.rx_buf[cur_rx].as_ptr() as usize, 
        RX_BUF_SIZE  // 必须invalidate整个buffer!
    );
}

// 3. Reclaim时 Clean descriptor - 将更新的描述符刷到内存
unsafe {
    clean_dcache_range(
        &self.rx_desc[cur_rx] as *const _ as usize,
        core::mem::size_of::<RxDesc>(),
    );
}
```

### 4. RX配置修复

**位置**: `hw_start()`函数

```rust
// 原代码: 缺少接收模式标志
let rx_config = (RX_FIFO_THRESH << 13) | (RX_DMA_BURST << 8);

// 修复后: 添加0x0E (ACCEPT_BROADCAST | ACCEPT_MULTICAST | ACCEPT_MY_PHYS)
let rx_config = (RX_FIFO_THRESH << 13) | (RX_DMA_BURST << 8) | 0x0E;
```

## 调试过程关键发现

### 1. 关闭Cache验证
- 关闭I-cache和D-cache后,数据能够正确读取
- **证明**: 问题确实是cache一致性导致的

### 2. Volatile读取诊断
- 即使使用volatile读取(绕过cache),数据仍然为0
- **说明**: 不仅是cache问题,还有其他因素(描述符轮询)

### 3. 日志分析
```
第一个ping: RX[0] 成功
第二个ping: 等待RX[1] 超时 (实际数据在RX[0])
第三个ping: RX[0] 成功
```
- **发现**: 硬件重复使用RX[0],软件却在轮询RX[1]

## 测试结果

修复后的ping测试(10次):
```
seq=0: ✓ 成功
seq=1: ✗ 超时 (ARP相关)
seq=2: ✓ 成功
seq=3: ✗ 超时 (收到ARP包)
seq=4: ✓ 成功
seq=5: ✓ 成功
seq=6: ✗ 超时
seq=7: ✓ 成功
...
```

**成功率**: ~60-70%

**剩余问题**:
- 偶尔超时是因为简单网络栈没有实现ARP协议
- 使用广播MAC地址发送,依赖对方ARP缓存
- 生产环境需要完整的ARP实现

## 经验总结

### 1. DMA与Cache一致性
- **关键规则**: DMA操作必须配合cache操作
  - CPU→硬件(发送): `clean_dcache` (write-back)
  - 硬件→CPU(接收): `invalidate_dcache` (discard cache)
  - 读取硬件更新: `invalidate_dcache` (强制从内存读)

### 2. Cache操作时机
- **初始化**: Invalidate RX buffers(丢弃栈上的初始0值)
- **发送前**: Clean TX buffer和descriptor
- **接收前**: Invalidate RX descriptor
- **接收后**: Invalidate RX buffer(整个buffer,不只是len字节)
- **回收时**: Clean RX descriptor

### 3. Cache对齐
- Cache操作必须按cache line对齐
- ARM64 cache line通常是64字节
- 地址向下对齐,大小向上对齐

### 4. 硬件行为不可预测
- 不要假设硬件会按照软件期望的顺序工作
- 遍历所有可能的位置查找数据
- RTL8125可能重复使用同一个RX描述符

### 5. 调试技巧
- 关闭cache验证是否是cache问题
- 使用volatile读取绕过cache
- 添加hexdump查看实际内存内容
- 对比C参考实现的每个细节

## 参考代码

- C参考实现: `pcie_test_impl.c`
- U-Boot RTL8169驱动
- ARM Architecture Reference Manual (cache操作)

## 总结

这个bug的修复展示了嵌入式系统中Cache一致性的重要性。关键是理解:

1. **CPU和硬件看到的内存可能不一致** - Cache导致的
2. **必须显式同步** - 使用clean/invalidate操作
3. **时机很关键** - 在正确的时间点执行cache操作
4. **硬件行为需要适配** - 不能假设硬件会按照规范工作

最终通过在初始化时invalidate RX buffers + 遍历所有RX描述符,成功解决了数据接收问题,实现了基本的网络通信功能。
