//! Tock 内核中与进程相关的策略。
//!
//! 该文件包含 Tock 内核在管理进程时可以使用的策略的定义和实现。
//! 例如，这些策略控制决策，例如是否应该重新启动特定进程。

use crate::process;
use crate::process::Process;

/// 用于在进程出现故障时执行有关操作的策略的通用Trait
///
/// 实现可以使用 `Process` 引用来决定要采取的行动。 如果需要，实现还可以使用 `debug!()` 来打印消息。
pub trait ProcessFaultPolicy {
    /// 决定内核应对“进程”故障采取何种行动。
    fn action(&self, process: &dyn Process) -> process::FaultAction;
}

/// 如果Process出现故障，只需让整个board恐慌。
pub struct PanicFaultPolicy {}

impl ProcessFaultPolicy for PanicFaultPolicy {
    fn action(&self, _: &dyn Process) -> process::FaultAction {
        process::FaultAction::Panic
    }
}

/// 如果Process出现故障，只需停止Process并不再调度它。
pub struct StopFaultPolicy {}

impl ProcessFaultPolicy for StopFaultPolicy {
    fn action(&self, _: &dyn Process) -> process::FaultAction {
        process::FaultAction::Stop
    }
}

/// 如果进程出现故障，停止进程并不再安排它，但还会打印一条调试消息，通知用户进程出现故障并停止。
pub struct StopWithDebugFaultPolicy {}

impl ProcessFaultPolicy for StopWithDebugFaultPolicy {
    fn action(&self, process: &dyn Process) -> process::FaultAction {
        crate::debug!(
            "Process {} faulted and was stopped.",
            process.get_process_name()
        );
        process::FaultAction::Stop
    }
}

/// 如果出现故障，请始终重新启动该过程。
pub struct RestartFaultPolicy {}

impl ProcessFaultPolicy for RestartFaultPolicy {
    fn action(&self, _: &dyn Process) -> process::FaultAction {
        process::FaultAction::Restart
    }
}

/// `ProcessFaultPolicy` 的实现，它使用阈值来决定是否在进程出现故障时重新启动进程。
/// 如果进程重新启动的次数超过阈值，则进程将停止并且不再调度。
pub struct ThresholdRestartFaultPolicy {
    threshold: usize,
}

impl ThresholdRestartFaultPolicy {
    pub const fn new(threshold: usize) -> ThresholdRestartFaultPolicy {
        ThresholdRestartFaultPolicy { threshold }
    }
}

impl ProcessFaultPolicy for ThresholdRestartFaultPolicy {
    fn action(&self, process: &dyn Process) -> process::FaultAction {
        if process.get_restart_count() <= self.threshold {
            process::FaultAction::Restart
        } else {
            process::FaultAction::Stop
        }
    }
}

/// Implementation of `ProcessFaultPolicy` that uses a threshold to decide
/// whether to restart a process when it faults. If the process has been
/// restarted more times than the threshold then the board will panic.
pub struct ThresholdRestartThenPanicFaultPolicy {
    threshold: usize,
}

impl ThresholdRestartThenPanicFaultPolicy {
    pub const fn new(threshold: usize) -> ThresholdRestartThenPanicFaultPolicy {
        ThresholdRestartThenPanicFaultPolicy { threshold }
    }
}

impl ProcessFaultPolicy for ThresholdRestartThenPanicFaultPolicy {
    fn action(&self, process: &dyn Process) -> process::FaultAction {
        if process.get_restart_count() <= self.threshold {
            process::FaultAction::Restart
        } else {
            process::FaultAction::Panic
        }
    }
}
