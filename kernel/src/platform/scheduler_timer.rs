//! 用于执行进程时间片的调度程序计时器
//!
//! 内核使用的接口来配置可以抢占用户空间进程的计时器。

use crate::hil::time::{self, Frequency, Ticks};

/// 系统调度程序定时器的接口。
///
/// 系统调度程序计时器提供倒计时计时器以强制执行进程调度时间量。
/// 当 CPU 处于活动状态时，实现应该具有一致的时序，但不需要在睡眠期间运行。
/// 请注意，许多调度程序实现还将代表进程运行内核所花费的时间计入进程时间量。
///
/// 此接口的实现必须满足的主要要求是它必须能够在定时器到期时产生中断。
/// 此中断将中断执行过程，将控制权返回给内核，并允许调度程序决定接下来要运行什么。
///
/// 在大多数芯片上，此接口将由核心外围设备（例如 ARM 核心 SysTick 外围设备）实现。
/// 然而，一些芯片缺少这个可选的外设，在这种情况下，它可能由另一个定时器或alarm外设实现，
/// 或者需要在共享硬件定时器之上进行虚拟化。
///
/// `SchedulerTimer` 接口经过精心设计，非常通用，以支持不同硬件平台上所需的各种实现。
/// 一般的操作是内核会启动一个定时器，它启动分配给进程的时间片。
/// 当进程运行时，内核将启动定时器，告诉实现它必须确保在时间片用完时会发生中断。
/// 当进程停止运行时，内核将解除定时器，向实现指示不再需要中断。
/// 为了检查进程是否已经用尽了它的时间量，内核将明确地询问实现。
/// 当时间片用完时，内核本身并不期望得到一个中断来处理。
/// 这是因为时间片可能在内核本身运行时结束，内核不需要有效地抢占自己。
///
/// 此接口中的 `arm()` 和 `disarm()` 函数用作可选的优化机会。
/// 这对允许实现仅在绝对必要时启用中断，即在进程实际执行时。
/// 但是，正确的实现可以在调度程序计时器启动的任何时候启用中断。
/// 实现必须确保在调用 arm() 时启用中断。
///
/// 在使用中断时，实现必须小心。 由于在核心内核循环和调度程序中使用了`SchedulerTimer`，
/// 因此在调用`SchedulerTimer`函数之前可能没有执行上半部分中断处理程序。
/// 特别是，在虚拟化计时器之上的实现可能会收到中断触发的上行调用“延迟”（即在内核调用
/// `has_expired()` 之后）。 实现应该确保他们可以可靠地检查时间片过期。
pub trait SchedulerTimer {
    /// 为进程时间片启动计时器。 `us` 参数是时间片的长度，以微秒为单位。
    ///
    /// 这必须设置一个计时器，其间隔尽可能接近给定的间隔（以微秒为单位）。
    /// 不需要启用中断。 但是，如果实现不能将计时与中断生成分开，
    /// 则`start()`的实现应该启用中断，并在定时器处于活动状态时使它们保持启用状态。
    ///
    /// 调用者可以假设至少有一个 24 位宽的时钟。 具体时序取决于驱动时钟。
    /// 对于具有专用 SysTick 外围设备的 ARM 板，由于对该值的额外硬件支持，10ms 的增量是
    /// 最准确的。 ARM SysTick 支持高达 400 毫秒的间隔。
    fn start(&self, us: u32);

    /// Reset the SchedulerTimer.
    ///
    /// 这必须重置计时器，并且可以安全地禁用它并将其置于低功耗状态。
    /// 在 `reset()` 之后立即调用 `start()` 以外的任何函数都是无效的。
    ///
    /// 实现_应该_禁用计时器并将其置于低功耗状态。 但是，并非所有实现都能够保证这一点
    /// 例如，取决于底层硬件或定时器是否在虚拟化定时器之上实现。
    fn reset(&self);

    /// 启动 SchedulerTimer 定时器并确保将产生中断。
    ///
    /// 计时器必须已经通过调用 start() 启动。 该函数保证在已经启动的定时器到期时会产生中断。
    /// 此中断将抢占正在运行的用户空间进程。
    ///
    /// 如果在调用 arm() 时中断已启用，则此函数应该是无操作实现。
    fn arm(&self);

    /// Disarm 不再需要中断的 SchedulerTimer 定时器。
    ///
    /// 这不会停止计时器，但会向 SchedulerTimer 指示不再需要中断（即进程不再执行）。
    /// 通过不需要中断，这可以允许某些实现通过处理中断来更有效。
    ///
    /// 如果实现不能在不停止计时机制的情况下禁用中断，则该函数应该是无操作实现。
    fn disarm(&self);

    /// 如果时间片仍处于活动状态，则返回进程时间片中剩余的微秒数。
    ///
    /// 如果时间片仍处于活动状态，则返回 `Some()` 以及时间片中剩余的微秒数。
    /// 如果时间片已过期，则返回“无”。
    ///
    /// 在给定时间片返回“None”（表示时间片已过期）后，该函数可能不会被调用，
    /// 直到再次调用“start()”（开始新的时间片）。 如果在返回 `None` 之后再次
    /// 调用 `get_remaining_us()` 而没有对 `start()` 的干预调用，则返回值是未指定的，
    /// 并且实现可以返回任何他们喜欢的值。
    fn get_remaining_us(&self) -> Option<u32>;
}

/// 计时器永不过期的虚拟“SchedulerTimer”实现。
///
/// 使用这个实现是可行的，但意味着调度程序不能中断non-yield进程。
impl SchedulerTimer for () {
    fn reset(&self) {}

    fn start(&self, _: u32) {}

    fn disarm(&self) {}

    fn arm(&self) {}

    fn get_remaining_us(&self) -> Option<u32> {
        Some(10000) // chose arbitrary large value
    }
}

/// 在虚拟警报之上实现 SchedulerTimer 特征。
///
/// 目前，这个实现稍微依赖于capsule中的虚拟alarm实现——即它假设 get_alarm 即使在
/// 定时器被解除后仍将返回传递的值。 因此，这只能通过虚拟alarm来实现。
/// 如果有一个专用的硬件定时器可用，那么直接为该硬件外围设备实现调度程序定时器性能更高，
/// 而无需在两者之间进行alarm抽象。
///
/// 这主要处理从wall time（所需的输入Trait）到用于跟踪alarm时间的ticks的转换。
pub struct VirtualSchedulerTimer<A: 'static + time::Alarm<'static>> {
    alarm: &'static A,
}

impl<A: 'static + time::Alarm<'static>> VirtualSchedulerTimer<A> {
    pub fn new(alarm: &'static A) -> Self {
        Self { alarm }
    }
}

impl<A: 'static + time::Alarm<'static>> SchedulerTimer for VirtualSchedulerTimer<A> {
    fn reset(&self) {
        let _ = self.alarm.disarm();
    }

    fn start(&self, us: u32) {
        let tics = {
            // 我们需要将微秒转换为native tic，这可能会在 32 位算术中溢出。
            // 所以我们转换为64位。 64 位除法是一个昂贵的子程序，但如果 `us` 是 10 的幂，
            // 编译器将使用 1_000_000 除数来简化它。
            let us = us as u64;
            let hertz = A::Frequency::frequency() as u64;

            (hertz * us / 1_000_000) as u32
        };

        let reference = self.alarm.now();
        self.alarm.set_alarm(reference, A::Ticks::from(tics));
    }

    fn arm(&self) {
        //self.alarm.arm();
    }

    fn disarm(&self) {
        //self.alarm.disarm();
    }

    fn get_remaining_us(&self) -> Option<u32> {
        // 我们需要从native tic 转换为`us`，乘法可能会在 32 位算术中溢出。 所以我们转换为64位。

        let diff = self
            .alarm
            .get_alarm()
            .wrapping_sub(self.alarm.now())
            .into_u32() as u64;

        // 如果下一个alarm距离现在超过一秒，则alarm必须已过期。
        // 当现在已经通过alarm时，使用此公式来防止错误。 选择 1 秒是因为它明显大于 start() 允许
        // 的 400 毫秒最大值，并且不需要计算开销（例如，使用 500 毫秒需要将返回的刻度除以 2）
        // 但是，如果alarm频率相对于 cpu 频率足够慢，则可能会在 now() == get_alarm() 时对其进行
        // 评估，因此我们会特殊情况下alarm已触发但减法未溢出的结果
        if diff >= A::Frequency::frequency() as u64 || diff == 0 {
            None
        } else {
            let hertz = A::Frequency::frequency() as u64;
            Some(((diff * 1_000_000) / hertz) as u32)
        }
    }
}
