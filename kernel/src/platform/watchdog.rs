//! Interface for configuring a watchdog

/// 在内核中实现看门狗的特征。 从 `kernel_loop()` 代码调用此 trait 来设置和
/// 维护看门狗定时器。 如何处理看门狗中断取决于特定的“芯片”。
pub trait WatchDog {
    /// This function must enable the watchdog timer and configure it to
    /// trigger regulary. The period of the timer is left to the implementation
    /// to decide. The implementation must ensure that it doesn't trigger too
    /// early (when we haven't hung for example) or too late as to not catch
    /// faults.
    /// After calling this function the watchdog must be running.
    fn setup(&self) {}

    /// This function must tickle the watchdog to reset the timer.
    /// If the watchdog was previously suspended then this should also
    /// resume the timer.
    fn tickle(&self) {}

    /// Suspends the watchdog timer. After calling this the timer should not
    /// fire until after `tickle()` has been called. This function is called
    /// before sleeping.
    fn suspend(&self) {}

    /// Resumes the watchdog timer. After calling this the timer should be
    /// running again. This is called after returning from sleep, after
    /// `suspend()` was called.
    fn resume(&self) {
        self.tickle();
    }
}

/// Implement default WatchDog trait for unit.
impl WatchDog for () {}
