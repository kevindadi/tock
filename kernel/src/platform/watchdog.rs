//! Interface for configuring a watchdog

/// 在内核中实现看门狗的特征。 从 `kernel_loop()` 代码调用此 trait 来设置和
/// 维护看门狗定时器。 如何处理看门狗中断取决于特定的“芯片”。
pub trait WatchDog {
    /// 该功能必须使能看门狗定时器并配置为定时触发。
    /// 定时器的周期留给实现来决定。
    /// 实现必须确保它不会触发得太早（例如，当我们还没有挂起时）或太晚而无法捕获故障。

    /// 调用此函数后，看门狗必须运行。
    fn setup(&self) {}

    /// 该函数必须触发看门狗来重置定时器。
    /// 如果看门狗先前被暂停，那么这也应该恢复定时器。
    fn tickle(&self) {}

    /// 暂停看门狗定时器。 调用此函数后，计时器不应触发，
    /// 直到调用 `tickle()` 之后。 该函数在睡眠前调用。
    fn suspend(&self) {}

    /// 恢复看门狗定时器。 调用此函数后，计时器应再次运行。
    /// 在调用 `suspend()` 之后，在从睡眠中返回后调用它。
    fn resume(&self) {
        self.tickle();
    }
}

/// 为Unit实现默认的 WatchDog Trait。
impl WatchDog for () {}
