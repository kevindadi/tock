//! 用于存储从内核到进程的Upcall的数据结构.

use core::ptr::NonNull;

use crate::config;
use crate::debug;
use crate::process;
use crate::process::ProcessId;
use crate::syscall::SyscallReturn;
use crate::ErrorCode;

/// Type to uniquely identify an upcall subscription across all drivers.
///
/// 这包含驱动程序中的驱动程序编号和订阅编号。
#[derive(Copy, Clone, PartialEq, Debug)]
pub struct UpcallId {
    pub driver_num: usize,
    pub subscribe_num: usize,
}

/// 调度进程 Upcall 时可能发生的错误。
///
/// 考虑到 null upcall 是由进程设置的明确定义的 upcall，计划 null upcall(不会传递给进程)不是错误。
/// 它的行为本质上与进程设置适当的 Upcall 相同，并且会忽略所有调用，其好处是没有任务插入到进程的任务队列中。
#[derive(Copy, Clone, Debug)]
pub enum UpcallError {
    /// 传递的 `subscribe_num` 超出了此进程可用的调用次数。
    ///
    /// 对于带有 `n` 向上调用的 [`Grant`](crate::grant::Grant)，
    /// 当使用 `subscribe_num >= n` 调用 `GrantKernelData::schedule_upcall` 时会返回此错误。
    //
    /// 没有安排 Upcall，对 `GrantKernelData::schedule_upcall` 的调用没有可观察到的效果。
    ///
    InvalidSubscribeNum,
    /// 进程的任务队列已满。
    ///
    /// 当为一个进程安排了太多任务（例如，Upcalls），而该进程没有让步或有机会恢复执行时，可能会发生此错误。
    ///
    /// 没有安排 Upcall，对 `GrantKernelData::schedule_upcall` 的调用没有可观察到的效果。
    QueueFull,
    /// 违反了内核内部的不变量。
    ///
    /// 这个错误永远不会发生。
    /// 如果进程处于非活动状态（应该被 [`Grant::enter`](crate::grant::Grant::enter) 捕获）
    /// 或 `process.tasks` 被占用，则可以返回它。
    ///
    /// 这些情况无法合理处理。
    KernelError,
}

/// 用于在进程中调用 upcall 的类型。
///
/// 这本质上是一个函数指针的包装器，带有相关的Process数据。
pub(crate) struct Upcall {
    /// The ProcessId of the process this upcall is for.
    pub(crate) process_id: ProcessId,

    /// 此特定 upcall 的唯一标识符，表示用于提交它的 driver_num 和 subdriver_num.
    pub(crate) upcall_id: UpcallId,

    /// 调用 subscribe() 时应用程序传递的应用程序数据
    pub(crate) appdata: usize,

    /// 指向与 app_id 关联的应用程序中函数的第一条指令的指针。
    ///
    /// 如果这个值为`None`，这是一个null upcall，实际上不能被调度。
    /// `Upcall` 可以在首次创建时为 null，或者在应用取消订阅 upcall 之后。
    pub(crate) fn_ptr: Option<NonNull<()>>,
}

impl Upcall {
    pub(crate) fn new(
        process_id: ProcessId,
        upcall_id: UpcallId,
        appdata: usize,
        fn_ptr: Option<NonNull<()>>,
    ) -> Upcall {
        Upcall {
            process_id,
            upcall_id,
            appdata,
            fn_ptr,
        }
    }

    /// Schedule the upcall.
    ///
    /// 这会将给定进程的 [`Upcall`] 排队。 如果进程的队列已满并且无法安排upcall，或者这是一个空upcall，则返回“false”。
    ///
    /// 参数（`r0-r2`）是传回进程的值，特定于各个`Driver`接口。
    ///
    /// 这个函数也将 `process` 作为一个参数（即使我们的结构中有 process_id）以避免搜索 processes 数组来安排 upcall。
    /// 目前，传递这个参数很方便，所以我们利用它。
    /// 如果将来不是这种情况，我们可以选择 `process` 并使用存储的 process_id 进行搜索。
    pub(crate) fn schedule(
        &mut self,
        process: &dyn process::Process,
        r0: usize,
        r1: usize,
        r2: usize,
    ) -> Result<(), UpcallError> {
        let res = self.fn_ptr.map_or(
            // A null-Upcall is treated as being delivered to
            // the process and ignored
            Ok(()),
            |fp| {
                let enqueue_res =
                    process.enqueue_task(process::Task::FunctionCall(process::FunctionCall {
                        source: process::FunctionCallSource::Driver(self.upcall_id),
                        argument0: r0,
                        argument1: r1,
                        argument2: r2,
                        argument3: self.appdata,
                        pc: fp.as_ptr() as usize,
                    }));

                match enqueue_res {
                    Ok(()) => Ok(()),
                    Err(ErrorCode::NODEVICE) => {
                        // There should be no code path to schedule an
                        // Upcall on a process that is no longer
                        // alive. Indicate a kernel-internal error.
                        Err(UpcallError::KernelError)
                    }
                    Err(ErrorCode::NOMEM) => {
                        // No space left in the process' task queue.
                        Err(UpcallError::QueueFull)
                    }
                    Err(_) => {
                        // All other errors returned by
                        // `Process::enqueue_task` must be treated as
                        // kernel-internal errors
                        Err(UpcallError::KernelError)
                    }
                }
            },
        );

        if config::CONFIG.trace_syscalls {
            debug!(
                "[{:?}] schedule[{:#x}:{}] @{:#x}({:#x}, {:#x}, {:#x}, {:#x}) = {:?}",
                self.process_id,
                self.upcall_id.driver_num,
                self.upcall_id.subscribe_num,
                self.fn_ptr.map_or(0x0 as *mut (), |fp| fp.as_ptr()) as usize,
                r0,
                r1,
                r2,
                self.appdata,
                res
            );
        }
        res
    }

    /// 创建适合返回用户空间的成功系统调用返回类型。
    ///
    /// 此函数旨在在成功订阅调用和upcall交换后返回到用户空间的“old call”。
    ///
    /// 我们提供这个`.into`函数是因为返回类型需要包含upcall的函数指针。
    pub(crate) fn into_subscribe_success(self) -> SyscallReturn {
        match self.fn_ptr {
            Some(fp) => SyscallReturn::SubscribeSuccess(fp.as_ptr(), self.appdata),
            None => SyscallReturn::SubscribeSuccess(0 as *const (), self.appdata),
        }
    }

    /// 创建一个适合返回用户空间的失败案例系统调用返回类型。
    ///
    /// 这适用于无法处理订阅调用并且从用户空间传递的函数指针必须返回用户空间的情况。
    ///
    /// 我们提供这个`.into`函数是因为返回类型需要包含upcall的函数指针。
    pub(crate) fn into_subscribe_failure(self, err: ErrorCode) -> SyscallReturn {
        match self.fn_ptr {
            Some(fp) => SyscallReturn::SubscribeFailure(err, fp.as_ptr(), self.appdata),
            None => SyscallReturn::SubscribeFailure(err, 0 as *const (), self.appdata),
        }
    }
}
