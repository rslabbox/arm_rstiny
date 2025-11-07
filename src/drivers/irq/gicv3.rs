//! GICv3 (Generic Interrupt Controller version 3) driver.

use arm_gic::{
    IntId, UniqueMmioPointer,
    gicv3::{
        GicCpuInterface, GicV3, InterruptGroup,
        registers::{Gicd, GicrSgi},
    },
};
use core::ptr::NonNull;
use memory_addr::pa;
use spin::Mutex;

use crate::mm::phys_to_virt;
use crate::platform::{CurrentBoard, board::Board};

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
    let intid = GicCpuInterface::get_and_acknowledge_interrupt(InterruptGroup::Group1).unwrap();
    trace!("Handling IRQ: {:?}", intid);

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
    debug!("IRQ registered: {:?}", intid);
}

/// Unregister the interrupt handler for the given interrupt ID.
pub fn irqset_unregister(intid: IntId) {
    let mut handler_table = IRQ_HANDLER_TABLE.lock();
    let intid_val = u32::from(intid) as usize;
    handler_table[intid_val] = None;
    debug!("IRQ unregistered: {:?}", intid);
}

/// Enable the given interrupt.
pub fn irqset_enable(intid: IntId) {
    let mut gic = GIC.lock();
    if let Some(ref mut gic) = *gic {
        let intid_val = u32::from(intid);
        // Determine the core ID based on interrupt type
        let core_id = if intid_val < 32 {
            Some(0) // SGIs and PPIs are per-core
        } else {
            None // SPIs are shared
        };

        let _ = gic.enable_interrupt(intid, core_id, true);
        debug!("IRQ enabled: {:?}", intid);
    } else {
        warn!("GIC not initialized, cannot enable IRQ: {:?}", intid);
    }
}

/// Disable the given interrupt.
pub fn irqset_disable(intid: IntId) {
    let mut gic = GIC.lock();
    if let Some(ref mut gic) = *gic {
        let intid_val = u32::from(intid);
        // Determine the core ID based on interrupt type
        let core_id = if intid_val < 32 {
            Some(0) // SGIs and PPIs are per-core
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
pub fn init() {
    // Base addresses of the GICv3 distributor and redistributor.
    let gicd_base_address: *mut Gicd = phys_to_virt(pa!(CurrentBoard::GICD_BASE)).as_mut_ptr_of();
    let gicr_base_address: *mut GicrSgi =
        phys_to_virt(pa!(CurrentBoard::GICR_BASE)).as_mut_ptr_of();

    let gicd = unsafe { UniqueMmioPointer::new(NonNull::new(gicd_base_address).unwrap()) };
    let gicr = NonNull::new(gicr_base_address).unwrap();

    // Initialise the GIC.
    let mut gic = unsafe { GicV3::new(gicd, gicr, 1, false) };
    gic.setup(0);

    // Store the GIC instance globally for later use
    *GIC.lock() = Some(gic);

    arm_gic::irq_enable();
}
