//! Tock 中实现板的接口。

use crate::errorcode;
use crate::platform::chip::Chip;
use crate::platform::scheduler_timer;
use crate::platform::watchdog;
use crate::process;
use crate::scheduler::Scheduler;
use crate::syscall;
use crate::syscall_driver::SyscallDriver;
use tock_tbf::types::CommandPermissions;

/// 开发板提供给内核的组合特性，包括内核支持的所有可扩展操作.
///
/// 这是为特定板配置内核的主要方法。
pub trait KernelResources<C: Chip> {
    /// 内核将使用的系统调用调度机制的实现。
    type SyscallDriverLookup: SyscallDriverLookup;

    /// 内核将使用的系统调用过滤机制的实现。
    type SyscallFilter: SyscallFilter;

    /// 内核将使用的进程故障处理机制的实现。
    type ProcessFault: ProcessFault;

    /// 内核将使用的上下文切换回调处理程序的实现。
    type ContextSwitchCallback: ContextSwitchCallback;

    /// 内核将使用的调度算法的实现。
    type Scheduler: Scheduler<C>;

    /// 用于创建提供给应用程序的时间片的计时器的实现。
    type SchedulerTimer: scheduler_timer::SchedulerTimer;

    /// WatchDog 计时器的实现用于监视内核的运行。
    type WatchDog: watchdog::WatchDog;

    /// 返回对该平台将用于route syscalls的 SyscallDriverLookup 实现的引用。
    fn syscall_driver_lookup(&self) -> &Self::SyscallDriverLookup;

    /// 返回对该平台希望内核使用的 SyscallFilter 实现的引用。
    fn syscall_filter(&self) -> &Self::SyscallFilter;

    /// 返回对此平台希望内核使用的 ProcessFault 处理程序的实现的引用。
    fn process_fault(&self) -> &Self::ProcessFault;

    /// 返回对此平台希望内核使用的调度程序实现的引用。
    fn scheduler(&self) -> &Self::Scheduler;

    /// 返回对此平台的 SchedulerTimer 计时器实现的引用。
    fn scheduler_timer(&self) -> &Self::SchedulerTimer;

    /// 返回对此平台上 WatchDog 实现的引用。
    fn watchdog(&self) -> &Self::WatchDog;

    /// 返回对此平台的 ContextSwitchCallback 实现的引用。
    fn context_switch_callback(&self) -> &Self::ContextSwitchCallback;
}

/// 配置系system call dispatch 映射。
///
/// 每个板都应该定义一个实现这个特性的结构。
/// 这个特征是如何处理系统调用调度的核心，实现负责为每个系统调用号调度到驱动程序。
///
/// ## Example
///
/// ```ignore
/// struct Hail {
///     console: &'static capsules::console::Console<'static>,
///     ipc: kernel::ipc::IPC,
///     dac: &'static capsules::dac::Dac<'static>,
/// }
///
/// impl SyscallDriverLookup for Hail {
///     fn with_driver<F, R>(&self, driver_num: usize, f: F) -> R
///     where
///         F: FnOnce(Option<&dyn kernel::SyscallDriver>) -> R,
///     {
///         match driver_num {
///             capsules::console::DRIVER_NUM => f(Some(self.console)),
///             kernel::ipc::DRIVER_NUM => f(Some(&self.ipc)),
///             capsules::dac::DRIVER_NUM => f(Some(self.dac)),
///
///             _ => f(None),
///         }
///     }
/// }
/// ```
pub trait SyscallDriverLookup {
    /// 系统调用号到实现该系统调用的驱动程序方法的对象的特定于平台的映射。
    ///
    ///
    /// An implementation
    fn with_driver<F, R>(&self, driver_num: usize, f: F) -> R
    where
        F: FnOnce(Option<&dyn SyscallDriver>) -> R;
}

/// 用于实现内核用来决定是否处理特定系统调用的系统调用过滤器的Trait
pub trait SyscallFilter {
    /// 检查平台提供的系统调用过滤器以查找所有non-yield syscall。
    /// 如果提供的进程允许系统调用，则返回“Ok(())”。
    /// 否则，返回带有“ErrorCode”的“Err()”，该错误代码将返回给调用应用程序。
    /// 默认实现允许所有系统调用。
    ///
    /// 这个 API 应该被认为是不稳定的，并且很可能在未来发生变化。
    fn filter_syscall(
        &self,
        _process: &dyn process::Process,
        _syscall: &syscall::Syscall,
    ) -> Result<(), errorcode::ErrorCode> {
        Ok(())
    }
}

/// 实现默认允许单元的所有 SyscallFilter Trait。
impl SyscallFilter for () {}

/// 基于 TBF 标头的允许列表系统调用过滤器，默认允许所有fallback。
/// 这将检查进程是否指定了 TbfHeaderPermissions。如果进程具有 TbfHeaderPermissions，
/// 它们将用于确定访问权限。 有关这方面的详细信息，请参阅 TockBinaryFormat 文档。
/// 如果没有指定权限，默认是允许系统调用。
pub struct TbfHeaderFilterDefaultAllow {}

/// 为基于 TBF 标头的过滤实现默认的 SyscallFilter 特征。
impl SyscallFilter for TbfHeaderFilterDefaultAllow {
    fn filter_syscall(
        &self,
        process: &dyn process::Process,
        syscall: &syscall::Syscall,
    ) -> Result<(), errorcode::ErrorCode> {
        match syscall {
            // Subscribe is allowed if any commands are
            syscall::Syscall::Subscribe {
                driver_number,
                subdriver_number: _,
                upcall_ptr: _,
                appdata: _,
            } => match process.get_command_permissions(*driver_number, 0) {
                CommandPermissions::NoPermsAtAll => Ok(()),
                CommandPermissions::NoPermsThisDriver => Err(errorcode::ErrorCode::NODEVICE),
                CommandPermissions::Mask(_allowed) => Ok(()),
            },

            syscall::Syscall::Command {
                driver_number,
                subdriver_number,
                arg0: _,
                arg1: _,
            } => match process.get_command_permissions(*driver_number, subdriver_number / 64) {
                CommandPermissions::NoPermsAtAll => Ok(()),
                CommandPermissions::NoPermsThisDriver => Err(errorcode::ErrorCode::NODEVICE),
                CommandPermissions::Mask(allowed) => {
                    if (1 << (subdriver_number % 64)) & allowed > 0 {
                        Ok(())
                    } else {
                        Err(errorcode::ErrorCode::NODEVICE)
                    }
                }
            },

            // Allow is allowed if any commands are
            syscall::Syscall::ReadWriteAllow {
                driver_number,
                subdriver_number: _,
                allow_address: _,
                allow_size: _,
            } => match process.get_command_permissions(*driver_number, 0) {
                CommandPermissions::NoPermsAtAll => Ok(()),
                CommandPermissions::NoPermsThisDriver => Err(errorcode::ErrorCode::NODEVICE),
                CommandPermissions::Mask(_allowed) => Ok(()),
            },

            // Allow is allowed if any commands are
            syscall::Syscall::UserspaceReadableAllow {
                driver_number,
                subdriver_number: _,
                allow_address: _,
                allow_size: _,
            } => match process.get_command_permissions(*driver_number, 0) {
                CommandPermissions::NoPermsAtAll => Ok(()),
                CommandPermissions::NoPermsThisDriver => Err(errorcode::ErrorCode::NODEVICE),
                CommandPermissions::Mask(_allowed) => Ok(()),
            },

            // Allow is allowed if any commands are
            syscall::Syscall::ReadOnlyAllow {
                driver_number,
                subdriver_number: _,
                allow_address: _,
                allow_size: _,
            } => match process.get_command_permissions(*driver_number, 0) {
                CommandPermissions::NoPermsAtAll => Ok(()),
                CommandPermissions::NoPermsThisDriver => Err(errorcode::ErrorCode::NODEVICE),
                CommandPermissions::Mask(_allowed) => Ok(()),
            },

            // Non-filterable system calls
            syscall::Syscall::Yield { .. }
            | syscall::Syscall::Memop { .. }
            | syscall::Syscall::Exit { .. } => Ok(()),
        }
    }
}

/// 用于实现Process故障处理程序以在Process故障时运行的Trait。
pub trait ProcessFault {
    /// This function is called when an app faults.
    ///
    /// 这是一个可选功能，可以由“平台”实现，允许芯片处理应用程序故障而不终止或重新启动应用程序。
    ///
    /// 如果此函数返回 `Ok(())`，则内核不会终止或重新启动应用程序，而是允许它继续运行。
    /// 注意在这种情况下，芯片必须已经修复了故障的根本原因，否则它将再次发生。
    ///
    /// 这不能用于应用程序绕过 Tock 的保护。
    /// 例如，如果此函数只是忽略错误并允许应用程序继续，则故障将继续发生。
    ///
    /// 如果返回 `Err(())`，那么内核会将应用程序设置为故障并遵循 `FaultResponse` 协议。
    ///
    /// “平台”不太可能需要实现这一点。 这应该仅用于少数用例。 可能的用例包括：
    ///    - 允许内核模拟未实现的指令
    ///      这可用于允许应用程序在未实现某些指令（例如原子指令）的硬件上运行。
    ///    - 允许内核处理由应用程序触发的硬件故障。
    ///      这可以让应用程序在触发某些类型的故障时继续运行。
    ///      例如，如果应用程序触发内存奇偶校验错误，内核可以处理该错误并允许应用程序继续（或不继续）。
    ///    - 允许应用从外部 QSPI 执行。
    ///      这可用于允许应用程序在“平台”可以处理访问错误的情况下执行，以确保正确映射 QPSI。
    #[allow(unused_variables)]
    fn process_fault_hook(&self, process: &dyn process::Process) -> Result<(), ()> {
        Err(())
    }
}

/// 为Unit实现默认的 ProcessFault Trait
impl ProcessFault for () {}

/// 在用户空间上下文切换上实现处理程序的特征
pub trait ContextSwitchCallback {
    /// 在内核切换到进程之前调用此函数。
    ///
    /// `process` 是即将运行的应用程序
    fn context_switch_hook(&self, process: &dyn process::Process);
}

/// 为Unit实现默认的 ContextSwitchCallback Trait
impl ContextSwitchCallback for () {
    fn context_switch_hook(&self, _process: &dyn process::Process) {}
}
