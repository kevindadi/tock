//! 在 Tock 中实现微控制器的接口。

use crate::platform::mpu;
use crate::syscall;
use core::fmt::Write;

/// 单个 MCU 的接口。
///
/// 该特征定义了 Tock 操作的芯片特定属性。
/// 其中包括是否以及使用哪种内存保护机制和 scheduler_timer，
/// 如何在内核和用户态应用程序之间切换，以及如何处理硬件事件。
///
/// 每个微控制器都应该定义一个结构并实现这个特性。
pub trait Chip {
    /// 此芯片的特定内存保护单元 (MPU)。
    type MPU: mpu::MPU;

    /// 此特定芯片的用户空间和内核之间接口的实现。
    /// 这可能是特定于架构的，但个别芯片可能有各种定制要求。
    type UserspaceKernelBoundary: syscall::UserspaceKernelBoundary;

    /// 内核调用这个函数来告诉芯片检查所有Pending的中断，并将它们正确地分派给芯片的外围驱动程序。
    /// 这个函数应该在内部循环，直到所有的中断都被处理完。
    /// 但是，如果在最后一次检查之后但在此函数返回之前发生中断,内核将处理这种极端情况。
    fn service_pending_interrupts(&self);

    /// 要求芯片检查是否有任何Pending中断。
    fn has_pending_interrupts(&self) -> bool;

    /// 返回对此芯片上 MPU 实现的引用。
    fn mpu(&self) -> &Self::MPU;

    /// 返回对用户空间和内核空间之间接口实现的引用。
    fn userspace_kernel_boundary(&self) -> &Self::UserspaceKernelBoundary;

    /// 当芯片无事可做并且应该进入低功耗睡眠状态时调用。
    /// 这种低功耗睡眠状态应该允许中断仍然处于活动状态，
    /// 以便下一个中断事件唤醒芯片并恢复调度程序。
    fn sleep(&self);

    /// 在原子状态下运行函数，这意味着中断被禁用，以便在传入函数执行期间不会触发中断。
    unsafe fn atomic<F, R>(&self, f: F) -> R
    where
        F: FnOnce() -> R;

    /// 将芯片状态（系统寄存器）打印到提供的写入器。 这不会打印出执行上下文（数据寄存器），
    /// 因为这取决于它们的存储方式；
    /// 这是由 `syscall::UserspaceKernelBoundary::print_context` 实现的。
    /// 这也不会打印出由 `process::Process::print_memory_map` 实现的进程内存状态。
    ///  MPU 状态由 MPU 的 Display trait 实现打印。被Panic使用。
    unsafe fn print_state(&self, writer: &mut dyn Write);
}

/// 用于处理硬件芯片上的中断和延迟调用的接口。
///
/// 每块板都必须构建这个特性的实现来处理特定的中断。 当一个中断（由编号标识）已触发并应处理时，
/// 将使用中断编号调用此 trait 的实现。 然后实现可以处理中断，
/// 或者返回“false”以表示它不知道如何处理中断。
///
/// 这个功能被赋予了这个“InterruptService”接口，因此多个对象可以链接在一起来处理芯片的中断。
/// 这对于代码组织和在特定微控制器的多个变体存在时消除重复的需要很有用。
/// 然后一个共享的基础对象可以处理大多数中断，而特定于变体的对象可以处理特定于变体的中断。
///
/// 为了在使用 `InterruptService` 时简化 Rust 代码的结构，应该“自上而下”地传递中断号。
/// 也就是说，要处理的中断将首先传递给最具体的“InterruptService”对象。
/// 如果该对象不能处理中断，那么它应该保持对第二个最具体的对象的引用，
/// 并通过调用该对象来处理中断来返回。 这一直持续到基础对象处理中断或决定芯片不知道
/// 如何处理中断。 例如，考虑一个依赖于 `nRF52` crate 的 `nRF52840` 芯片。
/// 如果两者都有他们知道如何处理的特定中断，则流程将如下所示：
///
/// ```ignore
///           +---->nrf52840_peripherals
///           |        |
///           |        |
///           |        v
/// kernel-->nrf52     nrf52_peripherals
/// ```
/// 内核指示“nrf52”crate处理中断，如果有一个中断准备好，
/// 那么该中断将通过 InterruptService 对象传递，直到有东西可以为它服务。
pub trait InterruptService<T> {
    /// 如果此芯片支持，则服务中断。 如果不支持此中断号，则返回 false。
    unsafe fn service_interrupt(&self, interrupt: u32) -> bool;

    /// Service a deferred call。 如果不支持此任务，则返回 false。
    unsafe fn service_deferred_call(&self, task: T) -> bool;
}

/// 类似时钟的东西应该支持的通用操作。
pub trait ClockInterface {
    fn is_enabled(&self) -> bool;
    fn enable(&self);
    fn disable(&self);
}

/// 需要时钟但没有时钟控制的接口的辅助结构。
pub struct NoClockControl {}
impl ClockInterface for NoClockControl {
    fn is_enabled(&self) -> bool {
        true
    }
    fn enable(&self) {}
    fn disable(&self) {}
}

/// NoClockControl 的实例，用于需要引用 `ClockInterface` 对象的事物。
pub static mut NO_CLOCK_CONTROL: NoClockControl = NoClockControl {};
