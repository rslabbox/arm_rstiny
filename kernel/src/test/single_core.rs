use crate::utils::single_core::SingleCore;

pub fn run() {
    let state = SingleCore::new(0);
    let other = SingleCore::new(());
    {
        let mut borrow = state.borrow_mut();
        *borrow = 42;
        assert!(state.try_borrow_mut().is_none());
        drop(other.borrow_mut());
        assert!(state.try_borrow_mut().is_none());
    }
    assert_eq!(*state.try_borrow_mut().unwrap(), 42);
    assert!(state.try_borrow_mut().is_some());
}
