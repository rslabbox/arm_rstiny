//! GICv3 (Generic Interrupt Controller version 3) driver.

use arm_gic::{
    IntId, UniqueMmioPointer,
    gicv3::{
        GicCpuInterface, GicV3, InterruptGroup,
        registers::{Gicd, GicrSgi},
    },
};
use core::ptr::NonNull;
use memory_addr::VirtAddr;
use spin::Mutex;

use crate::TinyResult;
use crate::error::TinyError;
use crate::hal::percpu;

/// The type of an interrupt handler.
pub type IrqHandler = fn(usize);

/// Maximum number of interrupts supported (SGIs + PPIs + SPIs).
const MAX_IRQ_COUNT: usize = 1024;

/// Global interrupt handler table.
static IRQ_HANDLER_TABLE: Mutex<[Option<IrqHandler>; MAX_IRQ_COUNT]> =
    Mutex::new([None; MAX_IRQ_COUNT]);

/// Global GIC instance.
static GIC: Mutex<Option<GicV3>> = Mutex::new(None);

/// IRQ handler called from exception vector.
pub fn irq_handler() {
    let intid = match GicCpuInterface::get_and_acknowledge_interrupt(InterruptGroup::Group1) {
        Some(id) => id,
        None => {
            error!("Failed to acknowledge interrupt");
            return;
        }
    };

    // Call the registered handler if exists
    let intid_val = u32::from(intid) as usize;
    let handler_table = IRQ_HANDLER_TABLE.lock();
    if let Some(handler) = handler_table[intid_val] {
        drop(handler_table); // Release lock before calling handler
        handler(intid_val);
    } else {
        warn!("No handler registered for IRQ: {:?}", intid);
    }

    GicCpuInterface::end_interrupt(intid, InterruptGroup::Group1);
}

/// Register an interrupt handler for the given interrupt ID.
pub fn irqset_register(intid: IntId, handler: IrqHandler) {
    let mut handler_table = IRQ_HANDLER_TABLE.lock();
    let intid_val = u32::from(intid) as usize;
    handler_table[intid_val] = Some(handler);
    debug!("IRQ registered: {:?} on CPU {}", intid, percpu::cpu_id());
}

/// Unregister the interrupt handler for the given interrupt ID.
#[allow(dead_code)]
pub fn irqset_unregister(intid: IntId) {
    let mut handler_table = IRQ_HANDLER_TABLE.lock();
    let intid_val = u32::from(intid) as usize;
    handler_table[intid_val] = None;
    debug!("IRQ unregistered: {:?}", intid);
}

/// Enable the given interrupt.
pub fn irqset_enable(intid: IntId, priority: u8) {
    let mut gic = GIC.lock();
    if let Some(ref mut gic) = *gic {
        let intid_val = u32::from(intid);
        // Determine the core ID based on interrupt type
        // SGIs and PPIs are per-core, use current CPU ID
        let core_id = if intid_val < 32 {
            Some(percpu::cpu_id())
        } else {
            None // SPIs are shared
        };
        // Set interrupt priority
        gic.set_interrupt_priority(intid, core_id, priority)
            .unwrap_or_else(|e| error!("Failed to set priority for IRQ {:?}: {:?}", intid, e));
        gic.enable_interrupt(intid, core_id, true)
            .unwrap_or_else(|e| error!("Failed to enable IRQ {:?}: {:?}", intid, e));
        debug!("IRQ enabled: {:?} on CPU {}", intid, percpu::cpu_id());
    } else {
        warn!("GIC not initialized, cannot enable IRQ: {:?}", intid);
    }
}

/// Disable the given interrupt.
#[allow(dead_code)]
pub fn irqset_disable(intid: IntId) {
    let mut gic = GIC.lock();
    if let Some(ref mut gic) = *gic {
        let intid_val = u32::from(intid);
        // Determine the core ID based on interrupt type
        // SGIs and PPIs are per-core, use current CPU ID
        let core_id = if intid_val < 32 {
            Some(percpu::cpu_id())
        } else {
            None // SPIs are shared
        };

        let _ = gic.enable_interrupt(intid, core_id, false);
        debug!("IRQ disabled: {:?}", intid);
    } else {
        warn!("GIC not initialized, cannot disable IRQ: {:?}", intid);
    }
}

/// Initialize the GICv3 interrupt controller.
pub fn init(gicd_virt: VirtAddr, gicr_virt: VirtAddr) -> TinyResult<()> {
    // Base addresses of the GICv3 distributor and redistributor.
    let gicd_base_address: *mut Gicd = gicd_virt.as_mut_ptr_of();
    let gicr_base_address: *mut GicrSgi = gicr_virt.as_mut_ptr_of();

    let gicd = unsafe {
        UniqueMmioPointer::new(NonNull::new(gicd_base_address).ok_or(TinyError::InvalidGicPointer)?)
    };
    let gicr = NonNull::new(gicr_base_address).ok_or(TinyError::InvalidGicPointer)?;

    // Initialise the GIC.
    let mut gic = unsafe { GicV3::new(gicd, gicr, crate::config::kernel::TINYENV_SMP, false) };
    gic.setup(0);

    // Store the GIC instance globally for later use
    *GIC.lock() = Some(gic);

    // Set priority mask to allow all priorities
    GicCpuInterface::set_priority_mask(0xff);

    arm_gic::irq_enable();

    Ok(())
}

/// Initialize GIC for secondary CPU.
///
/// Each secondary CPU needs to configure its own CPU interface.
/// The distributor is already initialized by the primary CPU.
pub fn init_secondary(cpu_id: usize) {
    // Setup GIC CPU interface for this CPU
    let mut gic = GIC.lock();
    
    if let Some(ref mut gic) = *gic {
        // gic.setup(cpu_id);
        gic.init_cpu(cpu_id);
        // gic.gicd.configure_default_settings();

        // Enable group 1 for the current security state.
        GicCpuInterface::enable_group1(true);

    } else {
        warn!("GIC not initialized, cannot setup CPU interface for CPU {}", cpu_id);
        return;
    }

    // Set priority mask to allow all priorities
    GicCpuInterface::set_priority_mask(0xff);

    // Enable interrupts on this CPU
    arm_gic::irq_enable();

    debug!("GIC initialized for CPU {}", cpu_id);
}
