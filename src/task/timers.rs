use core::sync::atomic::{AtomicU64, Ordering};

use weak_map::WeakMap;

use crate::{drivers::timer::current_nanoseconds, hal::Mutex, task::task_ref::TaskState};

type WeakTaskRef = alloc::sync::Weak<super::SchedulableTask>;

static TIMER_KEY: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct TimerKey {
    deadline: u64,
    key: u64,
}

static TIMER_WHEEL: Mutex<WeakMap<TimerKey, WeakTaskRef>> = Mutex::new(WeakMap::new());

pub(crate) fn set_timer(deadline: u64, task: &super::TaskRef) -> Option<TimerKey> {
    if deadline <= current_nanoseconds() {
        return None;
    }

    let mut wheel = TIMER_WHEEL.lock();
    let key = TimerKey {
        deadline,
        key: TIMER_KEY.fetch_add(1, Ordering::AcqRel),
    };
    wheel.insert(key, task);

    Some(key)
}

#[allow(unused)]
pub(crate) fn cancel_timer(key: &TimerKey) {
    let mut wheel = TIMER_WHEEL.lock();
    wheel.remove(key);
}

#[allow(unused)]
pub(crate) fn has_timer(key: &TimerKey) -> bool {
    TIMER_WHEEL.lock().contains_key(key)
}

pub(crate) fn check_events() {
    let mut wheel = TIMER_WHEEL.lock();
    for (key, maybe_task) in &mut *wheel {
        if key.deadline <= current_nanoseconds() {
            if let Some(task) = maybe_task.upgrade() {
                // task_unblock(&task);
                if task.try_set_state(TaskState::Sleeping, TaskState::Ready) {
                    super::task_ops::TASK_MANAGER
                        .lock()
                        .put_prev_task(task.clone(), false);
                    core::mem::take(maybe_task);
                } else {
                    error!(
                        "Failed to wake up task id={} from timer: current state={:?}",
                        task.id(),
                        task.state()
                    );
                }
            }
        } else {
            break;
        }
    }
}
