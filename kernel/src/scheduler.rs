//! Tock 内核调度程序的接口。

pub mod cooperative;
pub mod mlfq;
pub mod priority;
pub mod round_robin;

use crate::dynamic_deferred_call::DynamicDeferredCall;
use crate::kernel::StoppedExecutingReason;
use crate::platform::chip::Chip;
use crate::process::ProcessId;
use crate::Kernel;

/// 任何调度程序必须实现的Trait
pub trait Scheduler<C: Chip> {
    /// Decide which process to run next.
    ///
    /// 调度器必须决定是否运行一个进程，如果是，运行哪一个。
    /// 如果调度器选择不运行进程，它可以请求芯片进入睡眠模式。
    ///
    /// 如果调度程序选择要运行的进程，它必须提供其“ProcessId”和
    /// 可选的时间片长度（以微秒为单位）以提供给该进程。
    /// 如果时间片为“None”，则进程将协同运行（即没有抢占）
    /// 否则，该过程将以设置为指定长度的时间片运行。
    fn next(&self, kernel: &Kernel) -> SchedulingDecision;

    /// 通知调度程序为什么最后一个进程停止执行，以及它执行了多长时间。
    /// 值得注意的是，如果调度程序请求此进程协同运行，则 `execution_time_us` 将为 `None`。
    fn result(&self, result: StoppedExecutingReason, execution_time_us: Option<u32>);

    /// 告诉调度程序执行内核工作，例如中断下半部分和动态延迟调用。
    /// 大多数调度程序将使用此默认实现，但有时希望延迟中断处理的调度程序将重新实现它。
    ///
    /// 提供此接口允许调度程序完全管理主内核循环的执行方式。
    /// 例如，试图帮助进程满足其最后期限的更高级的调度程序可能需要推迟下半部分中断处理
    /// 或选择性地服务某些中断。 或者，Power aware调度程序可能希望在任何时候有选择地
    /// 选择要完成的工作以满足power要求。
    ///
    /// 然而，这个函数的自定义实现必须非常小心，因为这个函数是在核心内核循环中调用的。
    unsafe fn execute_kernel_work(&self, chip: &C) {
        chip.service_pending_interrupts();
        DynamicDeferredCall::call_global_instance_while(|| !chip.has_pending_interrupts());
    }

    /// 询问调度程序是否暂停执行用户空间进程以处理内核任务。 大多数调度程序将使用这个默认实现，
    /// 它总是优先考虑内核工作，但是希望延迟中断处理的调度程序可能要重新实现它。
    unsafe fn do_kernel_work_now(&self, chip: &C) -> bool {
        chip.has_pending_interrupts()
            || DynamicDeferredCall::global_instance_calls_pending().unwrap_or(false)
    }

    /// 询问调度程序是否继续尝试执行进程。
    ///
    /// 一旦一个进程被调度，内核将尝试执行它，直到它没有更多的工作要做或耗尽它的时间片。
    /// 内核将在每个循环之前调用此函数，以检查调度程序是否要继续尝试执行此过程。
    ///
    /// 大多数调度程序将使用此默认实现，如果有需要服务的中断或延迟调用，这将导致 `do_process()` 循环返回。
    /// 但是，希望推迟中断处理的调度程序可能会改变这一点，或者希望检查当前进程的执行是否导致更高
    /// 优先级进程准备就绪的优先级调度程序（例如在 IPC 的情况下）。
    /// 如果这返回 `false`，则 `do_process` 将以 `KernelPreemption` 退出。
    ///
    /// `id` 是当前活动进程的标识符。
    unsafe fn continue_process(&self, _id: ProcessId, chip: &C) -> bool {
        !(chip.has_pending_interrupts()
            || DynamicDeferredCall::global_instance_calls_pending().unwrap_or(false))
    }
}

/// 枚举表示调度程序可以在每次调用 `scheduler.next()` 时请求的操作。
#[derive(Copy, Clone)]
pub enum SchedulingDecision {
    /// 告诉内核使用传递的时间片运行指定的进程。
    /// 如果 `None` 作为时间片传递，进程将协同运行。
    RunProcess((ProcessId, Option<u32>)),

    /// 告诉内核进入睡眠状态。 值得注意的是，如果调度程序在内核任务准备好时要求内核休眠，
    /// 内核将不会休眠，而是重新启动主循环并再次调用`next()`。
    TrySleep,
}
