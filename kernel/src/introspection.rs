//! 检查内核状态的机制。
//!
//! 特别是它提供了获取board上进程状态的功能。它可能会扩展到其他内核状态。
//!
//! 为了限制对可以使用这个模块的访问，即使它是公共的（在 Rust 意义上）所以它在这个 crate 之外是可见的，
//! the introspection functions要求调用者具有调用函数的正确能力。 这可以防止任意capsule能够使用此模块，
//! 并且只有board author明确传递了正确功能才能使用它的capsule。

use core::cell::Cell;

use crate::capabilities::ProcessManagementCapability;
use crate::kernel::Kernel;
use crate::process;
use crate::process::ProcessId;
use crate::utilities::cells::NumericCellExt;

/// 该结构提供检查功能。
pub struct KernelInfo {
    kernel: &'static Kernel,
}

impl KernelInfo {
    pub fn new(kernel: &'static Kernel) -> KernelInfo {
        KernelInfo { kernel: kernel }
    }

    /// 返回此平台上已加载的进程数。 这在功能上相当于board上已使用了多少个进程槽。
    /// 这不考虑进程处于什么状态，只要它已经被加载。
    pub fn number_loaded_processes(&self, _capability: &dyn ProcessManagementCapability) -> usize {
        let count: Cell<usize> = Cell::new(0);
        self.kernel.process_each(|_| count.increment());
        count.get()
    }

    /// 返回有多少进程被认为是活动的。 这包括处于“Running”和“Yield”状态的进程。
    /// 这不包括发生故障的进程，或内核不再调度的进程，因为它们发生故障过于频繁或出于其他原因。
    pub fn number_active_processes(&self, _capability: &dyn ProcessManagementCapability) -> usize {
        let count: Cell<usize> = Cell::new(0);
        self.kernel
            .process_each(|process| match process.get_state() {
                process::State::Running => count.increment(),
                process::State::Yielded => count.increment(),
                _ => {}
            });
        count.get()
    }

    /// 返回有多少进程被认为是非活动的。 这包括处于“故障”状态的进程和内核出于任何原因未调度的进程。
    pub fn number_inactive_processes(
        &self,
        _capability: &dyn ProcessManagementCapability,
    ) -> usize {
        let count: Cell<usize> = Cell::new(0);
        self.kernel
            .process_each(|process| match process.get_state() {
                process::State::Running => {}
                process::State::Yielded => {}
                _ => count.increment(),
            });
        count.get()
    }

    /// Get the name of the process.
    pub fn process_name(
        &self,
        app: ProcessId,
        _capability: &dyn ProcessManagementCapability,
    ) -> &'static str {
        self.kernel
            .process_map_or("unknown", app, |process| process.get_process_name())
    }

    /// 返回应用程序调用的系统调用数
    pub fn number_app_syscalls(
        &self,
        app: ProcessId,
        _capability: &dyn ProcessManagementCapability,
    ) -> usize {
        self.kernel
            .process_map_or(0, app, |process| process.debug_syscall_count())
    }

    /// 返回应用程序经历的dropped upcalls。如果在capsule尝试安排 upcall 时应用程序的队列已满，则可以放弃 upcall。
    pub fn number_app_dropped_upcalls(
        &self,
        app: ProcessId,
        _capability: &dyn ProcessManagementCapability,
    ) -> usize {
        self.kernel
            .process_map_or(0, app, |process| process.debug_dropped_upcall_count())
    }

    /// 返回此应用程序已重新启动的次数
    pub fn number_app_restarts(
        &self,
        app: ProcessId,
        _capability: &dyn ProcessManagementCapability,
    ) -> usize {
        self.kernel
            .process_map_or(0, app, |process| process.get_restart_count())
    }

    /// 返回此应用程序超出其时间片的次数
    pub fn number_app_timeslice_expirations(
        &self,
        app: ProcessId,
        _capability: &dyn ProcessManagementCapability,
    ) -> usize {
        self.kernel
            .process_map_or(0, app, |process| process.debug_timeslice_expiration_count())
    }

    /// 返回（此应用已分配的Grant区域中的Grant数量，系统中存在的Grant总数）的元组。
    pub fn number_app_grant_uses(
        &self,
        app: ProcessId,
        _capability: &dyn ProcessManagementCapability,
    ) -> (usize, usize) {
        // Just need to get the number, this has already been finalized, but it
        // doesn't hurt to call this again.
        let number_of_grants = self.kernel.get_grant_count_and_finalize();
        let used = self.kernel.process_map_or(0, app, |process| {
            // 让process告诉我们分配的Grant数量。
            // 如果此Process无效，则我们无法计算Grant，我们所能做的就是返回 0。
            process.grant_allocated_count().unwrap_or(0)
        });

        (used, number_of_grants)
    }

    /// 返回所有进程超过其时间片的总次数。
    pub fn timeslice_expirations(&self, _capability: &dyn ProcessManagementCapability) -> usize {
        let count: Cell<usize> = Cell::new(0);
        self.kernel.process_each(|proc| {
            count.add(proc.debug_timeslice_expiration_count());
        });
        count.get()
    }
}
