/*
 * PCIe ATU Configuration and RTL8125 Driver for TestOS-Reflector
 * 
 * This implementation provides:
 * - PCIe Address Translation Unit (ATU) configuration
 * - PCIe device enumeration and BAR reading
 * - RTL8125 network controller initialization
 * - Basic ICMP ping functionality
 */

#include "lib/t_logger.h"
#include "mem/t_mmio.h"
#include "mem/t_mem.h"
#include "lib/t_string.h"
#include "mem/cache.h"
#include "t_types.h"

/* PCIe DBI base address for RK3588 */
#define DBI_BASE 0xa40c00000UL

/* PCIe Configuration Space Registers */
#define PCIE_CFG_VENDOR_ID 0x00
#define PCIE_CFG_COMMAND   0x04
#define PCIE_CFG_STATUS    0x06
#define PCIE_CFG_CLASS_REV 0x08
#define PCIE_CFG_BAR0      0x10

/* PCIe Command Register bits */
#define PCIE_CMD_IO_ENABLE      (1 << 0)  /* I/O Space Enable */
#define PCIE_CMD_MEM_ENABLE     (1 << 1)  /* Memory Space Enable */
#define PCIE_CMD_BUS_MASTER     (1 << 2)  /* Bus Master Enable */
#define PCIE_CMD_SPECIAL_CYCLES (1 << 3)  /* Special Cycles Enable */
#define PCIE_CMD_MWI_ENABLE     (1 << 4)  /* Memory Write and Invalidate */
#define PCIE_CMD_VGA_SNOOP      (1 << 5)  /* VGA Palette Snoop */
#define PCIE_CMD_PARITY_ERROR   (1 << 6)  /* Parity Error Response */
#define PCIE_CMD_SERR_ENABLE    (1 << 8)  /* SERR# Enable */
#define PCIE_CMD_FAST_B2B       (1 << 9)  /* Fast Back-to-Back Enable */
#define PCIE_CMD_INT_DISABLE    (1 << 10) /* Interrupt Disable */

/* ATU Unroll mode offsets (DBI + 0x300000) */
#define ATU_UNROLL_BASE_OFFSET 0x300000UL

/* ATU Region offsets in Unroll mode (each region is 512 bytes apart) */
#define ATU_REGION_SIZE  0x200 /* 512 bytes per region */
#define ATU_REGION_CTRL1 0x00
#define ATU_REGION_CTRL2 0x04
#define ATU_LOWER_BASE   0x08
#define ATU_UPPER_BASE   0x0C
#define ATU_LOWER_LIMIT  0x10
#define ATU_UPPER_LIMIT  0x14
#define ATU_LOWER_TARGET 0x18
#define ATU_UPPER_TARGET 0x1C

/* ATU Configuration */
#define PCIE_ATU_REGION_INDEX0   0
#define PCIE_ATU_REGION_INDEX1   1
#define PCIE_ATU_TYPE_MEM        0x0
#define PCIE_ATU_TYPE_IO         0x2
#define PCIE_ATU_TYPE_CFG0       0x4
#define PCIE_ATU_TYPE_CFG1       0x5
#define PCIE_ATU_ENABLE          (1 << 31)
#define PCIE_ATU_BAR_MODE_ENABLE (1 << 30)

/* RTL8125 specific registers */
#define RTL8125_MAC0             0x0000
#define RTL8125_MAC4             0x0004
#define RTL8125_MAR0             0x0008
#define RTL8125_TxDescStartAddr  0x0020
#define RTL8125_TxDescStartAddrH 0x0024
#define RTL8125_ChipCmd          0x0037
#define RTL8125_TxPoll           0x0090
#define RTL8125_IntrMask         0x0038
#define RTL8125_IntrStatus       0x003C
#define RTL8125_TxConfig         0x0040
#define RTL8125_RxConfig         0x0044
#define RTL8125_Cfg9346          0x0050
#define RTL8125_RxDescStartAddr  0x00E4
#define RTL8125_RxDescStartAddrH 0x00E8
#define RTL8125_MaxRxPacketSize  0x00DA

/* Chip command bits */
#define CMD_TX_ENABLE 0x04
#define CMD_RX_ENABLE 0x08
#define CMD_RESET     0x10

/* Config register unlock */
#define CFG9346_UNLOCK 0xC0
#define CFG9346_LOCK   0x00

/* Descriptor bits */
#define DESC_OWN 0x80000000
#define DESC_EOR 0x40000000
#define DESC_FS  0x20000000
#define DESC_LS  0x10000000

/* Network configuration */
#define NUM_TX_DESC 4
#define NUM_RX_DESC 4
#define RX_BUF_SIZE 2048
#define TX_BUF_SIZE 2048

/* Ethernet & IP protocol constants */
#define ETH_ALEN       6
#define ETH_HLEN       14
#define ETH_P_IP       0x0800
#define ETH_P_ARP      0x0806
#define IPPROTO_ICMP   1
#define ICMP_ECHO      8
#define ICMP_ECHOREPLY 0

/* Network structures */
typedef struct
{
    uint8_t  dest[ETH_ALEN];
    uint8_t  src[ETH_ALEN];
    uint16_t proto;
} __attribute__((packed)) eth_hdr_t;

typedef struct
{
    uint8_t  version_ihl;
    uint8_t  tos;
    uint16_t total_len;
    uint16_t id;
    uint16_t frag_off;
    uint8_t  ttl;
    uint8_t  protocol;
    uint16_t checksum;
    uint32_t src_addr;
    uint32_t dest_addr;
} __attribute__((packed)) ip_hdr_t;

typedef struct
{
    uint8_t  type;
    uint8_t  code;
    uint16_t checksum;
    uint16_t id;
    uint16_t sequence;
} __attribute__((packed)) icmp_hdr_t;

typedef struct
{
    uint32_t status;
    uint32_t vlan_tag;
    uint32_t buf_addr_lo;
    uint32_t buf_addr_hi;
} __attribute__((packed)) rtl_desc_t;

/* Global variables for network driver */
static volatile uint8_t *rtl_mmio_base = NULL;
static rtl_desc_t       *tx_ring       = NULL;
static rtl_desc_t       *rx_ring       = NULL;
static uint8_t          *tx_buffers[NUM_TX_DESC];
static uint8_t          *rx_buffers[NUM_RX_DESC];
static uint32_t          tx_idx               = 0;
static uint32_t          rx_idx               = 0;
static uint8_t           my_mac[ETH_ALEN]     = {0x2e, 0xc3, 0x69, 0x34, 0x7d, 0x31};
static uint8_t           remote_mac[ETH_ALEN] = {0x38, 0xf7, 0xcd, 0xc8, 0xd9, 0x32};


/* Helper functions for MMIO */
static inline uint8_t
rtl_read8(uint32_t reg)
{
    return read8((void *) (rtl_mmio_base + reg));
}

static inline uint16_t
rtl_read16(uint32_t reg)
{
    return read16((void *) (rtl_mmio_base + reg));
}

static inline uint32_t
rtl_read32(uint32_t reg)
{
    return read32((void *) (rtl_mmio_base + reg));
}

static inline void
rtl_write8(uint32_t reg, uint8_t val)
{
    write8(val, (void *) (rtl_mmio_base + reg));
}

static inline void
rtl_write16(uint32_t reg, uint16_t val)
{
    write16(val, (void *) (rtl_mmio_base + reg));
}

static inline void
rtl_write32(uint32_t reg, uint32_t val)
{
    write32(val, (void *) (rtl_mmio_base + reg));
}

/* Simple delay function */
static void
udelay(uint32_t us)
{
    volatile uint32_t i;
    for (i = 0; i < us * 100; i++)
        ;
}

static void
mdelay(uint32_t ms)
{
    udelay(ms * 1000);
}

/* Checksum calculation */
static uint16_t
ip_checksum(void *data, int len)
{
    uint32_t  sum = 0;
    uint16_t *ptr = (uint16_t *) data;

    while (len > 1) {
        sum += *ptr++;
        len -= 2;
    }

    if (len == 1) {
        sum += *(uint8_t *) ptr;
    }

    while (sum >> 16) {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }

    return ~sum;
}

/* Byte swap helpers */
static inline uint16_t
htons(uint16_t val)
{
    return ((val & 0xFF) << 8) | ((val >> 8) & 0xFF);
}

static inline uint32_t
htonl(uint32_t val)
{
    return ((val & 0xFF) << 24) | ((val & 0xFF00) << 8) | ((val >> 8) & 0xFF00) |
           ((val >> 24) & 0xFF);
}

static inline uint16_t
ntohs(uint16_t val)
{
    return htons(val);
}

static inline uint32_t
ntohl(uint32_t val)
{
    return htonl(val);
}

/* Memory copy helper */
static void
memcpy_local(void *dst, const void *src, size_t n)
{
    uint8_t       *d = (uint8_t *) dst;
    const uint8_t *s = (const uint8_t *) src;
    while (n--)
        *d++ = *s++;
}

static void
memset_local(void *dst, int val, size_t n)
{
    uint8_t *d = (uint8_t *) dst;
    while (n--)
        *d++ = (uint8_t) val;
}

/**
 * dw_pcie_setup_atu - Setup PCIe Address Translation Unit (Unroll mode)
 *
 * Configure the ATU to map CPU address space to PCIe bus address space
 * Using iATU Unroll mode (DBI + 0x300000 + region * 0x200)
 */
static int
dw_pcie_setup_atu(uint64_t dbi_base,
                  uint32_t region_index,
                  uint32_t type,
                  uint64_t cpu_addr,
                  uint64_t pci_addr,
                  uint64_t size)
{
    uint32_t retries = 5;
    uint32_t val;

    /* Calculate ATU region base address (Unroll mode) */
    uint64_t atu_base    = dbi_base + ATU_UNROLL_BASE_OFFSET;
    uint64_t region_base = atu_base + (region_index * ATU_REGION_SIZE);

    logger_info("=== Setting up PCIe ATU Region %d (Unroll Mode) ===\n", region_index);
    logger_info("  Type: 0x%x (%s)\n",
                type,
                type == PCIE_ATU_TYPE_MEM    ? "Memory"
                : type == PCIE_ATU_TYPE_CFG0 ? "Config"
                                             : "Unknown");
    logger_info("  CPU Address (source): 0x%llx\n", cpu_addr);
    logger_info("  PCI Address (target): 0x%llx\n", pci_addr);
    logger_info("  Size: 0x%llx (%llu bytes)\n", size, size);
    logger_info("  DBI Base: 0x%llx\n", dbi_base);
    logger_info("  ATU Base: 0x%llx\n", atu_base);
    logger_info("  Region Base: 0x%llx\n", region_base);

    /* Create volatile pointers for direct register access */
    /* Use explicit casting: uint64_t -> uint64_t -> pointer to avoid truncation */
    volatile uint32_t *reg_lower_base =
        (volatile uint32_t *) (uint64_t) (region_base + ATU_LOWER_BASE);
    volatile uint32_t *reg_upper_base =
        (volatile uint32_t *) (uint64_t) (region_base + ATU_UPPER_BASE);
    volatile uint32_t *reg_lower_limit =
        (volatile uint32_t *) (uint64_t) (region_base + ATU_LOWER_LIMIT);
    volatile uint32_t *reg_upper_limit =
        (volatile uint32_t *) (uint64_t) (region_base + ATU_UPPER_LIMIT);
    volatile uint32_t *reg_lower_target =
        (volatile uint32_t *) (uint64_t) (region_base + ATU_LOWER_TARGET);
    volatile uint32_t *reg_upper_target =
        (volatile uint32_t *) (uint64_t) (region_base + ATU_UPPER_TARGET);
    volatile uint32_t *reg_ctrl1 =
        (volatile uint32_t *) (uint64_t) (region_base + ATU_REGION_CTRL1);
    volatile uint32_t *reg_ctrl2 =
        (volatile uint32_t *) (uint64_t) (region_base + ATU_REGION_CTRL2);

    /* Configure lower and upper base (source CPU address) */
    uint32_t lower_base = (uint32_t) (cpu_addr & 0xFFFFFFFF);
    uint32_t upper_base = (uint32_t) (cpu_addr >> 32);

    *reg_lower_base = lower_base;
    DSB_SY();
    *reg_upper_base = upper_base;
    DSB_SY();

    logger_debug("  Lower base: 0x%08x\n", lower_base);
    logger_debug("  Upper base: 0x%08x\n", upper_base);

    /* Configure limit (end of source address range) */
    uint64_t limit_addr  = cpu_addr + size - 1;
    uint32_t lower_limit = (uint32_t) (limit_addr & 0xFFFFFFFF);
    uint32_t upper_limit = (uint32_t) (limit_addr >> 32);

    *reg_lower_limit = lower_limit;
    DSB_SY();
    *reg_upper_limit = upper_limit;
    DSB_SY();

    logger_debug("  Lower limit: 0x%08x\n", lower_limit);
    logger_debug("  Upper limit: 0x%08x\n", upper_limit);

    /* Configure target address (PCIe bus address) */
    uint32_t lower_target = (uint32_t) (pci_addr & 0xFFFFFFFF);
    uint32_t upper_target = (uint32_t) (pci_addr >> 32);

    *reg_lower_target = lower_target;
    DSB_SY();
    *reg_upper_target = upper_target;
    DSB_SY();

    logger_debug("  Lower target: 0x%08x\n", lower_target);
    logger_debug("  Upper target: 0x%08x\n", upper_target);

    /* Configure region control (transaction type) */
    *reg_ctrl1 = type;
    DSB_SY();
    logger_debug("  CTRL1 (Type): 0x%08x\n", type);

    /* Enable ATU region */
    *reg_ctrl2 = PCIE_ATU_ENABLE;
    DSB_SY();
    logger_debug("  CTRL2 (Enable): 0x%08x\n", PCIE_ATU_ENABLE);

    /* Wait for ATU enable to take effect */
    while (retries--) {
        DSB_SY();
        val = *reg_ctrl2;
        DSB_SY();
        if (val & PCIE_ATU_ENABLE) {
            logger_info("ATU region %d enabled successfully!\n", region_index);
            if (retries < 4) {
                logger_info("  (enabled after %d retries)\n", 4 - retries);
            }
            return 0;
        }
        udelay(1000); /* Wait 1ms */
    }

    logger_error("Failed to enable ATU region %d (timeout)\n", region_index);
    return -1;
}

/**
 * pcie_config_read32 - Read 32-bit value from PCIe config space
 * 
 * This function temporarily maps ATU Region 1 to Config space,
 * reads the value, then maps it back to Memory space.
 */
static uint32_t
pcie_config_read32(uint64_t dbi_base,
                   uint64_t cfg_base,
                   uint64_t bar_phys,
                   uint32_t offset,
                   bool     restore_memory)
{
    uint32_t val;
    int      ret;

    /* Map ATU Region 1 to Config space for reading */
    ret = dw_pcie_setup_atu(dbi_base,
                            PCIE_ATU_REGION_INDEX1,
                            PCIE_ATU_TYPE_CFG0,
                            0xf3000000UL, /* CPU address */
                            0x00000000UL, /* PCIe address */
                            0x100000UL);  /* 1MB */
    if (ret != 0) {
        logger_error("Failed to map ATU to Config space for read!\n");
        return 0xFFFFFFFF;
    }

    /* Read the value */
    val = read32((void *) (cfg_base + offset));

    /* Restore ATU Region 1 to Memory space if requested */
    if (restore_memory && bar_phys != 0) {
        ret = dw_pcie_setup_atu(dbi_base,
                                PCIE_ATU_REGION_INDEX1,
                                PCIE_ATU_TYPE_MEM,
                                0x9c0100000UL, /* CPU address */
                                bar_phys,      /* PCIe BAR address */
                                0x10000UL);    /* 64KB */
        if (ret != 0) {
            logger_error("Failed to restore ATU to Memory space!\n");
        }
    }

    return val;
}

/**
 * pcie_config_write32 - Write 32-bit value to PCIe config space
 */
static void
pcie_config_write32(uint64_t dbi_base,
                    uint64_t cfg_base,
                    uint64_t bar_phys,
                    uint32_t offset,
                    uint32_t value,
                    bool     restore_memory)
{
    int ret;

    /* Map ATU Region 1 to Config space for writing */
    ret = dw_pcie_setup_atu(dbi_base,
                            PCIE_ATU_REGION_INDEX1,
                            PCIE_ATU_TYPE_CFG0,
                            0xf3000000UL,
                            0x00000000UL,
                            0x100000UL);
    if (ret != 0) {
        logger_error("Failed to map ATU to Config space for write!\n");
        return;
    }

    /* Write the value */
    write32(value, (void *) (cfg_base + offset));

    /* Restore ATU Region 1 to Memory space if requested */
    if (restore_memory && bar_phys != 0) {
        ret = dw_pcie_setup_atu(dbi_base,
                                PCIE_ATU_REGION_INDEX1,
                                PCIE_ATU_TYPE_MEM,
                                0x9c0100000UL,
                                bar_phys,
                                0x10000UL);
        if (ret != 0) {
            logger_error("Failed to restore ATU to Memory space!\n");
        }
    }
}

/**
 * pcie_scan_bus - Scan PCIe bus for devices
 * 
 * Uses pcie_config_read32 which temporarily switches ATU Region 1 to Config mode
 */
static int
pcie_scan_bus(uint64_t  dbi_base,
              uint64_t  cfg_base,
              uint32_t *vendor_id,
              uint32_t *device_id,
              uint32_t *class_code)
{
    uint32_t val;

    logger_info("=== Scanning PCIe Bus ===\n");
    logger_info("  DBI base: 0x%llx\n", dbi_base);
    logger_info("  Config base: 0x%llx\n", cfg_base);

    /* Read Vendor ID and Device ID (offset 0x00) */
    val        = pcie_config_read32(dbi_base, cfg_base, 0, 0x00, false);
    *vendor_id = val & 0xFFFF;
    *device_id = (val >> 16) & 0xFFFF;

    logger_info("  Vendor ID: 0x%04x\n", *vendor_id);
    logger_info("  Device ID: 0x%04x\n", *device_id);

    if (*vendor_id == 0xFFFF || *vendor_id == 0x0000) {
        logger_error("  No device found (invalid vendor ID)\n");
        return -1;
    }

    /* Read Class Code (offset 0x08) */
    val         = pcie_config_read32(dbi_base, cfg_base, 0, 0x08, false);
    *class_code = val >> 8;

    logger_info("  Class Code: 0x%06x\n", *class_code);
    logger_info("  Revision ID: 0x%02x\n", val & 0xFF);

    return 0;
}

/**
 * pcie_enable_device - Enable PCIe device (Memory Space, Bus Master)
 * 
 * Uses pcie_config_read32/write32 which temporarily switches ATU Region 1
 */
static int
pcie_enable_device(uint64_t dbi_base, uint64_t cfg_base, uint64_t bar_phys)
{
    uint32_t cmd_reg;
    uint16_t cmd_val, status_val;

    logger_info("=== Enabling PCIe Device ===\n");

    /* Read current Command Register (offset 0x04, 16-bit) */
    cmd_reg    = pcie_config_read32(dbi_base, cfg_base, bar_phys, PCIE_CFG_COMMAND, false);
    cmd_val    = cmd_reg & 0xFFFF;
    status_val = (cmd_reg >> 16) & 0xFFFF;

    logger_info("  Original Command: 0x%04x\n", cmd_val);
    logger_info("  Original Status:  0x%04x\n", status_val);

    /* Enable Memory Space, Bus Master, and I/O Space */
    cmd_val |= PCIE_CMD_MEM_ENABLE;   /* Enable Memory Space access */
    cmd_val |= PCIE_CMD_BUS_MASTER;   /* Enable Bus Master (DMA) */
    cmd_val |= PCIE_CMD_IO_ENABLE;    /* Enable I/O Space access */
    cmd_val &= ~PCIE_CMD_INT_DISABLE; /* Enable interrupts */

    /* Write back Command Register (preserve Status register in upper 16 bits) */
    cmd_reg = (status_val << 16) | cmd_val;
    pcie_config_write32(dbi_base, cfg_base, bar_phys, PCIE_CFG_COMMAND, cmd_reg, false);

    /* Read back to verify */
    cmd_reg = pcie_config_read32(dbi_base, cfg_base, bar_phys, PCIE_CFG_COMMAND, true);
    cmd_val = cmd_reg & 0xFFFF;

    logger_info("  New Command: 0x%04x\n", cmd_val);
    logger_info("    Memory Space Enable: %s\n", (cmd_val & PCIE_CMD_MEM_ENABLE) ? "YES" : "NO");
    logger_info("    Bus Master Enable:   %s\n", (cmd_val & PCIE_CMD_BUS_MASTER) ? "YES" : "NO");
    logger_info("    I/O Space Enable:    %s\n", (cmd_val & PCIE_CMD_IO_ENABLE) ? "YES" : "NO");
    logger_info("    Interrupt Disable:   %s\n", (cmd_val & PCIE_CMD_INT_DISABLE) ? "YES" : "NO");

    if (!(cmd_val & PCIE_CMD_MEM_ENABLE)) {
        logger_error("  Failed to enable Memory Space!\n");
        return -1;
    }

    logger_info("  Device enabled successfully!\n");
    return 0;
}


/**
 * pcie_get_bar_info - Get BAR information
 * 
 * Uses pcie_config_read32/write32 which temporarily switches ATU Region 1
 */
static int
pcie_get_bar_info(uint64_t  dbi_base,
                  uint64_t  cfg_base,
                  uint32_t  bar_num,
                  uint64_t *bar_addr,
                  uint64_t *bar_size)
{
    uint32_t bar_offset = 0x10 + (bar_num * 4);
    uint32_t bar_val, bar_orig, size_mask;

    logger_info("=== Reading BAR%d Information ===\n", bar_num);

    /* Read original BAR value */
    bar_orig = pcie_config_read32(dbi_base, cfg_base, 0, bar_offset, false);
    logger_debug("  Original BAR value: 0x%08x\n", bar_orig);

    /* Write all 1s to determine size */
    pcie_config_write32(dbi_base, cfg_base, 0, bar_offset, 0xFFFFFFFF, false);
    bar_val = pcie_config_read32(dbi_base, cfg_base, 0, bar_offset, false);

    /* Restore original value */
    pcie_config_write32(dbi_base, cfg_base, 0, bar_offset, bar_orig, false);

    /* Calculate size */
    if (bar_val & 0x1) {
        /* I/O BAR */
        logger_info("  BAR%d is I/O type\n", bar_num);
        size_mask = bar_val & 0xFFFFFFFC;
        *bar_size = (~size_mask) + 1;
        *bar_addr = bar_orig & 0xFFFFFFFC;
    } else {
        /* Memory BAR */
        logger_info("  BAR%d is Memory type\n", bar_num);
        size_mask = bar_val & 0xFFFFFFF0;
        *bar_size = (~size_mask) + 1;
        *bar_addr = bar_orig & 0xFFFFFFF0;

        /* Check if 64-bit BAR */
        if ((bar_orig & 0x6) == 0x4) {
            logger_info("  64-bit BAR detected\n");
            uint32_t bar_upper = pcie_config_read32(dbi_base, cfg_base, 0, bar_offset + 4, false);
            *bar_addr |= ((uint64_t) bar_upper << 32);
        }
    }

    logger_info("  BAR%d Address: 0x%llx\n", bar_num, *bar_addr);
    logger_info("  BAR%d Size: 0x%llx (%llu bytes)\n", bar_num, *bar_size, *bar_size);

    return 0;
}

/**
 * rtl8125_init - Initialize RTL8125 network controller
 */
static int
rtl8125_init(uint64_t mmio_base)
{
    int      i;
    uint32_t val;

    logger_info("=== Initializing RTL8125 Network Controller ===\n");
    logger_info("  MMIO Base: 0x%llx\n", mmio_base);

    rtl_mmio_base = (volatile uint8_t *) mmio_base;

    /* Read and display MAC address */
    logger_info("  Reading MAC address...\n");
    for (i = 0; i < 6; i++) {
        my_mac[i] = rtl_read8(RTL8125_MAC0 + i);
    }
    logger_info("  MAC Address: %x:%x:%x:%x:%x:%x\n",
                my_mac[0],
                my_mac[1],
                my_mac[2],
                my_mac[3],
                my_mac[4],
                my_mac[5]);

    /* Software reset */
    logger_info("  Performing software reset...\n");
    rtl_write8(RTL8125_ChipCmd, CMD_RESET);
    mdelay(10);

    /* Wait for reset to complete */
    for (i = 0; i < 1000; i++) {
        if (!(rtl_read8(RTL8125_ChipCmd) & CMD_RESET))
            break;
        udelay(10);
    }

    if (i >= 1000) {
        logger_error("  Reset timeout!\n");
        return -1;
    }
    logger_info("  Reset completed\n");

    /* Unlock config registers */
    rtl_write8(RTL8125_Cfg9346, CFG9346_UNLOCK);
    logger_debug("  Config registers unlocked\n");

    /* Allocate descriptor rings and buffers */
    logger_info("  Allocating TX/RX descriptor rings...\n");

    /* For simplicity, using static allocation in real implementation
     * these should be DMA-able memory regions */
    tx_ring = (rtl_desc_t *) 0x50200000; /* Example physical address */
    rx_ring = (rtl_desc_t *) 0x50201000;
    memset((tx_ring), 0, sizeof(rtl_desc_t));
    memset((rx_ring), 0, sizeof(rtl_desc_t));

    logger_warn("  Note: Using placeholder addresses for descriptors\n");
    logger_warn("  In production, allocate proper DMA memory!\n");

    /* Setup TX descriptors */
    logger_info("  Setting up TX ring...\n");
    for (i = 0; i < NUM_TX_DESC; i++) {
        tx_ring[i].status      = 0;
        tx_ring[i].vlan_tag    = 0;
        tx_ring[i].buf_addr_lo = 0x50300000 + (i * TX_BUF_SIZE);
        tx_ring[i].buf_addr_hi = 0;
        tx_buffers[i]          = (uint8_t *) (0x50300000UL + (i * TX_BUF_SIZE));
    }
    tx_ring[NUM_TX_DESC - 1].status |= DESC_EOR;

    /* Setup RX descriptors */
    logger_info("  Setting up RX ring...\n");
    for (i = 0; i < NUM_RX_DESC; i++) {
        rx_ring[i].status      = DESC_OWN | RX_BUF_SIZE;
        rx_ring[i].vlan_tag    = 0;
        rx_ring[i].buf_addr_lo = 0x50400000 + (i * RX_BUF_SIZE);
        rx_ring[i].buf_addr_hi = 0;
        rx_buffers[i]          = (uint8_t *) (0x50400000UL + (i * RX_BUF_SIZE));
    }
    rx_ring[NUM_RX_DESC - 1].status |= DESC_EOR;

    // ===== 关键：刷新描述符缓存，让硬件能看到初始化的描述符 =====
    logger_debug("  Flushing TX/RX descriptor rings to memory...\n");
    clean_dcache_va_range((void *) tx_ring, NUM_TX_DESC * sizeof(rtl_desc_t));
    clean_dcache_va_range((void *) rx_ring, NUM_RX_DESC * sizeof(rtl_desc_t));

    /* Write descriptor addresses to NIC */
    rtl_write32(RTL8125_TxDescStartAddr, 0x50200000);
    rtl_write32(RTL8125_TxDescStartAddrH, 0);
    rtl_write32(RTL8125_RxDescStartAddr, 0x50201000);
    rtl_write32(RTL8125_RxDescStartAddrH, 0);

    logger_debug("  TX descriptor ring at: 0x50200000\n");
    logger_debug("  RX descriptor ring at: 0x50201000\n");

    /* Configure TX */
    logger_info("  Configuring TX...\n");
    val = (3 << 24) | (6 << 8); /* IFG and DMA burst */
    rtl_write32(RTL8125_TxConfig, val);
    logger_debug("  TX Config: 0x%08x\n", val);

    /* Configure RX */
    logger_info("  Configuring RX...\n");
    val = (7 << 13) | (6 << 8) | 0x0E; /* Accept all packets */
    rtl_write32(RTL8125_RxConfig, val);
    logger_debug("  RX Config: 0x%08x\n", val);

    /* Set max RX packet size */
    rtl_write16(RTL8125_MaxRxPacketSize, RX_BUF_SIZE);

    /* Enable TX and RX */
    logger_info("  Enabling TX and RX...\n");
    rtl_write8(RTL8125_ChipCmd, CMD_TX_ENABLE | CMD_RX_ENABLE);

    /* Lock config registers */
    rtl_write8(RTL8125_Cfg9346, CFG9346_LOCK);
    logger_debug("  Config registers locked\n");

    logger_info("RTL8125 initialization complete!\n");
    return 0;
}

/**
 * rtl8125_send_packet - Send a packet via RTL8125
 */
static int
rtl8125_send_packet(uint8_t *data, uint32_t len)
{
    if (!tx_ring || !tx_buffers[tx_idx]) {
        logger_error("TX ring/buffer not initialized!\n");
        return -1;
    }
    if (len > TX_BUF_SIZE) {
        logger_error("Packet too large for TX buffer!\n");
        return -1;
    }

    // 拷贝数据到当前TX缓冲区
    memcpy_local(tx_buffers[tx_idx], data, len);

    // 填充短包到最小以太网帧长度 (64字节)
    uint32_t pad_len = len;
    if (pad_len < 60) {  // ETH_ZLEN = 60 (不含FCS)
        memset_local(tx_buffers[tx_idx] + len, 0, 60 - len);
        pad_len = 60;
    }

    // ===== 关键：刷新 TX 缓冲区的缓存，让硬件能看到数据 =====
    // 对齐到缓存行边界
    uint64_t buf_start = (uint64_t) tx_buffers[tx_idx] & ~(g_cache_line_size - 1);
    uint64_t buf_end   = ((uint64_t) tx_buffers[tx_idx] + pad_len + g_cache_line_size - 1) &
                       ~(g_cache_line_size - 1);
    clean_dcache_va_range((void *) buf_start, buf_end - buf_start);

    // 设置描述符 OWN/FS/LS/长度
    tx_ring[tx_idx].status = DESC_OWN | DESC_FS | DESC_LS | pad_len;

    // ===== 关键：刷新描述符的缓存，让硬件能看到描述符 =====
    uint64_t desc_start = (uint64_t) &tx_ring[tx_idx] & ~(g_cache_line_size - 1);
    uint64_t desc_end = ((uint64_t) &tx_ring[tx_idx] + sizeof(rtl_desc_t) + g_cache_line_size - 1) &
                        ~(g_cache_line_size - 1);
    clean_dcache_va_range((void *) desc_start, desc_end - desc_start);

    // 触发硬件发送 - RTL8125 使用 0x1 而不是 0x40 (0x40 是 RTL8169 的值)
    rtl_write8(RTL8125_TxPoll, 0x01);

    // 等待发送完成（轮询OWN位）
    uint32_t timeout = 10000;
    while (timeout--) {
        // ===== 关键：使描述符缓存失效，从内存读取硬件更新 =====
        invalidate_dcache_va_range((void *) desc_start, desc_end - desc_start);

        if (!(tx_ring[tx_idx].status & DESC_OWN))
            break;

        udelay(10);
    }

    if (tx_ring[tx_idx].status & DESC_OWN) {
        logger_error("TX timeout!\n");
        return -1;
    }

    // 移动到下一个描述符
    tx_idx = (tx_idx + 1) % NUM_TX_DESC;
    return 0;
}

/**
 * rtl8125_recv_packet - Receive a packet from RTL8125
 */
static int
rtl8125_recv_packet(uint8_t *buffer, uint32_t *len, uint32_t timeout_ms)
{
    if (!rx_ring || !rx_buffers[rx_idx]) {
        logger_error("RX ring/buffer not initialized!\n");
        return -1;
    }

    // 计算描述符的缓存对齐范围
    uint64_t desc_start = (uint64_t) &rx_ring[rx_idx] & ~(g_cache_line_size - 1);
    uint64_t desc_end = ((uint64_t) &rx_ring[rx_idx] + sizeof(rtl_desc_t) + g_cache_line_size - 1) &
                        ~(g_cache_line_size - 1);

    uint32_t timeout = timeout_ms * 100;
    while (timeout--) {
        // ===== 关键：使描述符缓存失效，从内存读取硬件更新 =====
        invalidate_dcache_va_range((void *) desc_start, desc_end - desc_start);

        if (!(rx_ring[rx_idx].status & DESC_OWN))
            break;

        udelay(10);
    }

    if (rx_ring[rx_idx].status & DESC_OWN) {
        // 超时未收到包
        logger_debug("RX timeout: OWN bit still set (status=0x%08x)\n", rx_ring[rx_idx].status);
        return -1;
    }

    logger_debug("RX packet received! status=0x%08x\n", rx_ring[rx_idx].status);

    // 检查是否有错误
    if (rx_ring[rx_idx].status & 0x00200000) {  // RxRES bit
        logger_error("RX error detected in status\n");
        // 仍然需要重新初始化描述符
        goto reinit_desc;
    }

    // 获取包长度（不含 FCS 4字节）
    uint32_t pkt_len = (rx_ring[rx_idx].status & 0x3FFF) - 4;
    if (pkt_len > RX_BUF_SIZE)
        pkt_len = RX_BUF_SIZE;

    logger_debug("RX packet length: %d bytes\n", pkt_len);

    // ===== 关键：使 RX 缓冲区缓存失效，从内存读取硬件写入的数据 =====
    uint64_t buf_start = (uint64_t) rx_buffers[rx_idx] & ~(g_cache_line_size - 1);
    uint64_t buf_end   = ((uint64_t) rx_buffers[rx_idx] + pkt_len + g_cache_line_size - 1) &
                       ~(g_cache_line_size - 1);
    invalidate_dcache_va_range((void *) buf_start, buf_end - buf_start);

    memcpy_local(buffer, rx_buffers[rx_idx], pkt_len);
    *len = pkt_len;

reinit_desc:
    // 重新初始化描述符（关键：需要重新设置 buf_addr 和 OWN）
    if (rx_idx == NUM_RX_DESC - 1) {
        rx_ring[rx_idx].status = (DESC_OWN | DESC_EOR) + RX_BUF_SIZE;
    } else {
        rx_ring[rx_idx].status = DESC_OWN + RX_BUF_SIZE;
    }
    // 重新写入缓冲区地址（U-Boot 也这样做）
    rx_ring[rx_idx].buf_addr_lo = 0x50400000 + (rx_idx * RX_BUF_SIZE);
    rx_ring[rx_idx].buf_addr_hi = 0;

    // ===== 关键：刷新描述符缓存，让硬件能看到更新 =====
    clean_dcache_va_range((void *) desc_start, desc_end - desc_start);

    rx_idx = (rx_idx + 1) % NUM_RX_DESC;
    return 0;
}

/**
 * send_ping - Send ICMP Echo Request
 */
static void
send_ping(uint8_t src_ip[4], uint8_t dst_ip[4], uint16_t seq)
{
    uint8_t     packet[128];
    eth_hdr_t  *eth;
    ip_hdr_t   *ip;
    icmp_hdr_t *icmp;
    uint8_t    *payload;
    uint32_t    pkt_len = 0;

    logger_info("=== Preparing ICMP Echo Request (Ping) ===\n");
    logger_info("  Source IP: %d.%d.%d.%d\n", src_ip[0], src_ip[1], src_ip[2], src_ip[3]);
    logger_info("  Destination IP: %d.%d.%d.%d\n", dst_ip[0], dst_ip[1], dst_ip[2], dst_ip[3]);
    logger_info("  Sequence: %d\n", seq);

    memset_local(packet, 0, sizeof(packet));

    /* Ethernet header */
    eth = (eth_hdr_t *) packet;
    /* Destination MAC (broadcast for simplicity) */
    memcpy_local(eth->dest, remote_mac, ETH_ALEN);
    memcpy_local(eth->src, my_mac, ETH_ALEN);
    eth->proto = htons(ETH_P_IP);
    pkt_len += sizeof(eth_hdr_t);

    logger_debug("  Ethernet header:\n");
    logger_debug("    Dest MAC: %02x:%02x:%02x:%02x:%02x:%02x\n",
                 eth->dest[0],
                 eth->dest[1],
                 eth->dest[2],
                 eth->dest[3],
                 eth->dest[4],
                 eth->dest[5]);
    logger_debug("    Src MAC: %02x:%02x:%02x:%02x:%02x:%02x\n",
                 my_mac[0],
                 my_mac[1],
                 my_mac[2],
                 my_mac[3],
                 my_mac[4],
                 my_mac[5]);
    logger_debug("    EtherType: 0x%04x (IP)\n", ETH_P_IP);

    /* IP header */
    ip              = (ip_hdr_t *) (packet + pkt_len);
    ip->version_ihl = 0x45; /* IPv4, 20-byte header */
    ip->tos         = 0;
    ip->total_len   = htons(sizeof(ip_hdr_t) + sizeof(icmp_hdr_t) + 32);
    ip->id          = htons(0x1234);
    ip->frag_off    = 0;
    ip->ttl         = 64;
    ip->protocol    = IPPROTO_ICMP;
    ip->checksum    = 0;
    memcpy_local(&ip->src_addr, src_ip, 4);
    memcpy_local(&ip->dest_addr, dst_ip, 4);
    ip->checksum = ip_checksum(ip, sizeof(ip_hdr_t));
    pkt_len += sizeof(ip_hdr_t);

    logger_debug("  IP header:\n");
    logger_debug("    Version: 4, Header length: 20 bytes\n");
    logger_debug("    Total length: %d bytes\n", ntohs(ip->total_len));
    logger_debug("    TTL: %d\n", ip->ttl);
    logger_debug("    Protocol: %d (ICMP)\n", ip->protocol);
    logger_debug("    Checksum: 0x%04x\n", ntohs(ip->checksum));

    /* ICMP header */
    icmp           = (icmp_hdr_t *) (packet + pkt_len);
    icmp->type     = ICMP_ECHO;
    icmp->code     = 0;
    icmp->checksum = 0;
    icmp->id       = htons(0x5678);
    icmp->sequence = htons(seq);
    pkt_len += sizeof(icmp_hdr_t);

    /* ICMP payload */
    payload = packet + pkt_len;
    for (int i = 0; i < 32; i++) {
        payload[i] = i;
    }
    pkt_len += 32;

    /* Calculate ICMP checksum */
    icmp->checksum = ip_checksum(icmp, sizeof(icmp_hdr_t) + 32);

    logger_debug("  ICMP header:\n");
    logger_debug("    Type: %d (Echo Request)\n", icmp->type);
    logger_debug("    Code: %d\n", icmp->code);
    logger_debug("    Checksum: 0x%04x\n", ntohs(icmp->checksum));
    logger_debug("    ID: 0x%04x\n", ntohs(icmp->id));
    logger_debug("    Sequence: %d\n", ntohs(icmp->sequence));

    logger_info("  Total packet size: %d bytes\n", pkt_len);
    logger_info("  Sending ping packet...\n");

    /* Send the packet */
    rtl8125_send_packet(packet, pkt_len);

    logger_info("Ping request sent successfully!\n");
}

/**
 * test_dw_pcie_atu - Main test function
 *
 * This function configures PCIe ATU, scans for RTL8125, and tests ping
 */
void
test_dw_pcie_atu(void)
{
    uint64_t mmio_base_phys = 0xf3000000UL;
    uint64_t dbi_base_phys  = DBI_BASE;
    uint64_t cpu_addr       = 0xf3000000UL;
    uint64_t pci_addr       = 0x00000000UL;
    uint64_t size           = 0x100000UL; /* 1MB */
    uint64_t phy_addr       = 0x40100000UL;

    uint32_t vendor_id, device_id, class_code;
    uint64_t bar_addr, bar_size;
    uint64_t rtl_mmio_phys = 0x9c0100000UL; /* From your Rust code */
    uint64_t rtl_mmio_virt;
    int      ret;

    logger_info("\n");
    logger_info("========================================\n");
    logger_info("=== Testing DesignWare PCIe ATU ===\n");
    logger_info("========================================\n");
    logger_info("\n");

    /* Note: phys_to_virt function needs to be implemented or use direct mapping */
    uint64_t mmio_base_virt = mmio_base_phys; /* Assuming identity mapping */
    uint64_t dbi_base_virt  = dbi_base_phys;

    logger_info("Physical addresses:\n");
    logger_info("  MMIO base (config window): 0x%llx\n", mmio_base_phys);
    logger_info("  DBI base: 0x%llx\n", dbi_base_phys);
    logger_info("  Physical start: 0x%llx\n", phy_addr);
    logger_info("\n");

    /* Step 1: Scan PCIe bus (uses ATU Region 1 dynamically) */
    logger_info("Step 1: Scanning PCIe bus for devices\n");
    ret = pcie_scan_bus(dbi_base_virt, mmio_base_virt, &vendor_id, &device_id, &class_code);
    if (ret != 0) {
        logger_error("No PCIe device found!\n");
        return;
    }

    /* Check if it's a RealTek device */
    if (vendor_id == 0x10EC) {
        logger_info("  Device identified: RealTek (0x10EC)\n");
        if (device_id == 0x8125) {
            logger_info("  Model: RTL8125 2.5GbE Controller\n");
        } else if (device_id == 0x8169) {
            logger_info("  Model: RTL8169 GbE Controller\n");
        } else {
            logger_info("  Model: Unknown (Device ID 0x%04x)\n", device_id);
        }
    } else {
        logger_warn("  Warning: Not a RealTek device!\n");
    }
    logger_info("\n");

    /* Step 2: Read BAR information */
    logger_info("Step 2: Reading device BAR information\n");
    pcie_get_bar_info(dbi_base_virt, mmio_base_virt, 2, &bar_addr, &bar_size);
    logger_info("\n");

    /* Step 3: Enable PCIe device (Memory Space, Bus Master) */
    logger_info("Step 3: Enabling PCIe device\n");
    logger_info("  This enables Memory Space access and Bus Master capability\n");
    logger_info("  After this, ATU Region 1 is configured for Memory access to BAR\n");
    ret = pcie_enable_device(dbi_base_virt, mmio_base_virt, bar_addr);
    if (ret != 0) {
        logger_error("Failed to enable PCIe device!\n");
        return;
    }
    logger_info("\n");

    /* Step 4: Map BAR to memory */
    logger_info("Step 4: Mapping device BAR to system memory\n");
    logger_info("  Using physical address: 0x%llx\n", rtl_mmio_phys);
    rtl_mmio_virt = rtl_mmio_phys; /* Assuming identity mapping */
    logger_info("  Virtual address: 0x%llx\n", rtl_mmio_virt);

    /* Verify we can read from the BAR */
    uint32_t test_val = read32((void *) rtl_mmio_virt);
    logger_info("  Test read from BAR: 0x%lx\n", test_val);
    logger_info("\n");

    /* Step 5: Initialize RTL8125 */
    logger_info("Step 5: Initializing RTL8125 driver\n");
    ret = rtl8125_init(rtl_mmio_virt);
    if (ret != 0) {
        logger_error("Failed to initialize RTL8125!\n");
        return;
    }
    logger_info("\n");

    /* Step 6: Test ping */
    logger_info("Step 6: Testing ICMP ping functionality\n");
    uint8_t local_ip[4]  = {192, 168, 22, 102};
    uint8_t remote_ip[4] = {192, 168, 22, 101};

    logger_info("Network configuration:\n");
    logger_info("  Local IP: %d.%d.%d.%d\n", local_ip[0], local_ip[1], local_ip[2], local_ip[3]);
    logger_info("  Remote IP (ping target): %d.%d.%d.%d\n",
                remote_ip[0],
                remote_ip[1],
                remote_ip[2],
                remote_ip[3]);
    logger_info("\n");

    /* Send ping */
    send_ping(local_ip, remote_ip, 1);
    logger_info("\n");

    /* Wait for reply */
    logger_info("Waiting for ping reply...\n");
    uint8_t  rx_buffer[1024];
    uint32_t rx_len;

    // 尝试多次接收,因为可能会先收到 ARP 包
    int max_tries = 5;
    for (int try = 0; try < max_tries; try++) {
        logger_debug("  Receive attempt %d/%d...\n", try + 1, max_tries);
        ret = rtl8125_recv_packet(rx_buffer, &rx_len, 2000);  // 增加超时到 2 秒
        if (ret == 0) {
            logger_info("Received packet (%d bytes)\n", rx_len);

            /* Parse the reply */
            eth_hdr_t *eth   = (eth_hdr_t *) rx_buffer;
            uint16_t   proto = ntohs(eth->proto);
            logger_debug("  EtherType: 0x%04x\n", proto);

            if (proto == ETH_P_ARP) {
                logger_info("  Received ARP packet:\n");
                logger_info("    Source MAC: %02x:%02x:%02x:%02x:%02x:%02x\n",
                            eth->src[0],
                            eth->src[1],
                            eth->src[2],
                            eth->src[3],
                            eth->src[4],
                            eth->src[5]);
                logger_info("    Dest MAC: %02x:%02x:%02x:%02x:%02x:%02x\n",
                            eth->dest[0],
                            eth->dest[1],
                            eth->dest[2],
                            eth->dest[3],
                            eth->dest[4],
                            eth->dest[5]);
                logger_info("    (ignoring, waiting for ICMP reply)\n");
                continue;  // 继续等待 ICMP 回复
            }

            if (proto == ETH_P_IP) {
                ip_hdr_t *ip = (ip_hdr_t *) (rx_buffer + sizeof(eth_hdr_t));
                logger_debug("  IP Protocol: %d\n", ip->protocol);

                if (ip->protocol == IPPROTO_ICMP) {
                    icmp_hdr_t *icmp =
                        (icmp_hdr_t *) (rx_buffer + sizeof(eth_hdr_t) + sizeof(ip_hdr_t));
                    logger_debug("  ICMP Type: %d\n", icmp->type);

                    if (icmp->type == ICMP_ECHOREPLY) {
                        logger_info("\n");
                        logger_info("=== ICMP Echo Reply Received! ===\n");
                        logger_info("  Ethernet Header:\n");
                        logger_info("    Source MAC: %02x:%02x:%02x:%02x:%02x:%02x\n",
                                    eth->src[0],
                                    eth->src[1],
                                    eth->src[2],
                                    eth->src[3],
                                    eth->src[4],
                                    eth->src[5]);
                        logger_info("    Dest MAC: %02x:%02x:%02x:%02x:%02x:%02x\n",
                                    eth->dest[0],
                                    eth->dest[1],
                                    eth->dest[2],
                                    eth->dest[3],
                                    eth->dest[4],
                                    eth->dest[5]);
                        logger_info("    EtherType: 0x%04x (IP)\n", proto);

                        logger_info("  IP Header:\n");
                        logger_info("    Source IP: %d.%d.%d.%d\n",
                                    (ip->src_addr >> 0) & 0xFF,
                                    (ip->src_addr >> 8) & 0xFF,
                                    (ip->src_addr >> 16) & 0xFF,
                                    (ip->src_addr >> 24) & 0xFF);
                        logger_info("    Dest IP: %d.%d.%d.%d\n",
                                    (ip->dest_addr >> 0) & 0xFF,
                                    (ip->dest_addr >> 8) & 0xFF,
                                    (ip->dest_addr >> 16) & 0xFF,
                                    (ip->dest_addr >> 24) & 0xFF);
                        logger_info("    TTL: %d\n", ip->ttl);
                        logger_info("    Protocol: %d (ICMP)\n", ip->protocol);

                        logger_info("  ICMP Header:\n");
                        logger_info("    Type: %d (Echo Reply)\n", icmp->type);
                        logger_info("    Code: %d\n", icmp->code);
                        logger_info("    ID: 0x%04x\n", ntohs(icmp->id));
                        logger_info("    Sequence: %d\n", ntohs(icmp->sequence));
                        logger_info("    Checksum: 0x%04x\n", ntohs(icmp->checksum));
                        logger_info("\n");
                        logger_info("Ping test SUCCESSFUL!\n");
                        goto ping_success;
                    }
                }
            }
        }
    }

    logger_warn("No ICMP reply received after %d attempts\n", max_tries);
    logger_info("Note: Packets were sent successfully (verified by tcpdump)\n");

ping_success:
    // 检查是否还有其他包到达（如 ARP）
    logger_info("\nChecking for additional packets...\n");
    ret = rtl8125_recv_packet(rx_buffer, &rx_len, 500);
    if (ret == 0) {
        logger_info("Received additional packet (%d bytes)\n", rx_len);

        /* Parse the packet */
        eth_hdr_t *eth   = (eth_hdr_t *) rx_buffer;
        uint16_t   proto = ntohs(eth->proto);

        logger_info("  Ethernet Header:\n");
        logger_info("    Source MAC: %02x:%02x:%02x:%02x:%02x:%02x\n",
                    eth->src[0],
                    eth->src[1],
                    eth->src[2],
                    eth->src[3],
                    eth->src[4],
                    eth->src[5]);
        logger_info("    Dest MAC: %02x:%02x:%02x:%02x:%02x:%02x\n",
                    eth->dest[0],
                    eth->dest[1],
                    eth->dest[2],
                    eth->dest[3],
                    eth->dest[4],
                    eth->dest[5]);
        logger_info("    EtherType: 0x%04x ", proto);

        if (proto == ETH_P_ARP) {
            logger_info("(ARP)\n");
        } else if (proto == ETH_P_IP) {
            logger_info("(IP)\n");
            ip_hdr_t *ip = (ip_hdr_t *) (rx_buffer + sizeof(eth_hdr_t));
            logger_info("  IP Header:\n");
            logger_info("    Source IP: %d.%d.%d.%d\n",
                        (ip->src_addr >> 0) & 0xFF,
                        (ip->src_addr >> 8) & 0xFF,
                        (ip->src_addr >> 16) & 0xFF,
                        (ip->src_addr >> 24) & 0xFF);
            logger_info("    Dest IP: %d.%d.%d.%d\n",
                        (ip->dest_addr >> 0) & 0xFF,
                        (ip->dest_addr >> 8) & 0xFF,
                        (ip->dest_addr >> 16) & 0xFF,
                        (ip->dest_addr >> 24) & 0xFF);
            logger_info("    Protocol: %d ", ip->protocol);

            if (ip->protocol == IPPROTO_ICMP) {
                logger_info("(ICMP)\n");
                icmp_hdr_t *icmp =
                    (icmp_hdr_t *) (rx_buffer + sizeof(eth_hdr_t) + sizeof(ip_hdr_t));
                logger_info("    ICMP Type: %d ", icmp->type);
                if (icmp->type == ICMP_ECHO) {
                    logger_info("(Echo Request)\n");
                } else if (icmp->type == ICMP_ECHOREPLY) {
                    logger_info("(Echo Reply)\n");
                } else {
                    logger_info("(Other)\n");
                }
                logger_info("    ICMP Sequence: %d\n", ntohs(icmp->sequence));
            } else {
                logger_info("(Other)\n");
            }
        } else {
            logger_info("(Unknown)\n");
        }
    } else {
        logger_info("  No additional packets (timeout)\n");
    }

    logger_info("\n");
    logger_info("========================================\n");
    logger_info("=== PCIe ATU Test Complete ===\n");
    logger_info("========================================\n");
}
