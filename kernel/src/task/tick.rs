//! Periodic scheduler tick policy, implemented using a one-shot timer.
use crate::{
    arch::machine::{
        gic::{self, IrqId, Trigger},
        time, timer,
    },
    config::{TICK_NS, TIMER_IRQ},
    utils::single_core::SingleCore,
};
pub(crate) const IRQ: IrqId = IrqId::ppi(TIMER_IRQ - 16);
const PRIORITY: u8 = 0x80;
static PERIOD: SingleCore<Option<u64>> = SingleCore::new(None);

pub(super) fn init() {
    timer::init();
    {
        let mut period = PERIOD.borrow_mut();
        assert!(period.is_none(), "scheduler tick already initialized");
        let ticks = (time::frequency() as u128 * TICK_NS as u128)
            .div_ceil(1_000_000_000)
            .max(1);
        *period = Some(u64::try_from(ticks).expect("tick interval overflow"));
    }
    // Configure the route with its source disabled, clear stale pending state,
    // then arm and enable it before the first user context can run.
    gic::set_enable(IRQ, false);
    gic::set_trigger(IRQ, Trigger::Level);
    gic::set_priority(IRQ, PRIORITY);
    gic::set_pending(IRQ, false);
    rearm();
    gic::set_enable(IRQ, true);
}
fn rearm() {
    let ticks = PERIOD.borrow_mut().expect("scheduler tick not initialized");
    timer::arm(
        time::now()
            .checked_add(ticks)
            .expect("timer deadline overflow"),
    );
}
/// Clear the expired source before GIC completion. The caller requests a
/// scheduling point only after completing the active interrupt.
pub(crate) fn on_interrupt() {
    rearm();
}
