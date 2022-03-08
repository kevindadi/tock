//! Tock 的主内核循环、调度程序循环和调度程序特征
//! 该模块还包括调度程序策略实现常用的实用程序功能
//! 调度策略（循环、优先级等）在“sched” carte中定义并由板子选择

use core::cell::Cell;
use core::ptr::NonNull;

use crate::capabilities;
use crate::config;
use crate::debug;
use crate::dynamic_deferred_call::DynamicDeferredCall;
use crate::errorcode::ErrorCode;
use crate::grant::{AllowRoSize, AllowRwSize, Grant, UpcallSize};
use crate::ipc;
use crate::memop;
use crate::platform::chip::Chip;
use crate::platform::mpu::MPU;
use crate::platform::platform::ContextSwitchCallback;
use crate::platform::platform::KernelResources;
use crate::platform::platform::{ProcessFault, SyscallDriverLookup, SyscallFilter};
use crate::platform::scheduler_timer::SchedulerTimer;
use crate::platform::watchdog::WatchDog;
use crate::process::ProcessId;
use crate::process::{self, Task};
use crate::scheduler::{Scheduler, SchedulingDecision};
use crate::syscall::{ContextSwitchReason, SyscallReturn};
use crate::syscall::{Syscall, YieldCall};
use crate::syscall_driver::CommandReturn;
use crate::upcall::{Upcall, UpcallId};
use crate::utilities::cells::NumericCellExt;

/// 以微秒为单位的阈值，以考虑进程的时间片已耗尽
/// 也就是说，如果剩余时间片小于此阈值，Tock 将跳过重新调度进程
pub(crate) const MIN_QUANTA_THRESHOLD_US: u32 = 500;

/// 内核的主要对象.每个开发板都需要创建一个
pub struct Kernel {
    /// 在任何给定时间存在多少“待办事项”。 这些包括未完成的调用和处于运行状态的进程.
    work: Cell<usize>,

    /// 这包含一个指向静态进程指针数组的指针.
    processes: &'static [Option<&'static dyn process::Process>],

    /// 跟踪创建了多少进程标识符的计数器。 这用于为进程创建新的唯一标识符.
    process_identifier_max: Cell<usize>,

    /// 已设置多少个Grant区域。 每次调用 `create_grant()` 时都会增加。
    /// 我们需要明确地跟踪这一点，以便在创建进程时可以为每个Grant分配指针。
    grant_counter: Cell<usize>,

    /// 用于标记Grant已完成的标志。
    /// 这意味着内核不能支持创建新的Grant，因为已经创建了进程并且已经建立了Grant的数据结构
    grants_finalized: Cell<bool>,
}

/// 枚举用于通知调度程序为什么进程停止执行（也就是为什么 `do_process()` 返回）
#[derive(PartialEq, Eq)]
pub enum StoppedExecutingReason {
    /// 进程返回，因为它不再准备运行
    NoWorkLeft,

    /// 进程出现故障，并且配置了开发板重启策略，使其未重启且没有内核崩溃
    StoppedFaulted,

    /// 内核停止了该进程
    Stopped,

    /// 该进程被抢占，因为它的时间片已过期
    TimesliceExpired,

    /// 进程返回是因为它被内核抢占了
    /// 这可能意味着内核工作已准备就绪（很可能是因为触发了中断并且内核线程需要执行中断的下半部分），
    /// 或者因为调度程序不再想要执行该进程.
    KernelPreemption,
}

/// 表示try分配Grant区域时的不同结果
enum AllocResult {
    NoAllocation,
    NewAllocation,
    SameAllocation,
}

/// 尝试为指定的驱动程序和进程分配Grant区域
/// 返回是否分配了新的授权
fn try_allocate_grant<KR: KernelResources<C>, C: Chip>(
    resources: &KR,
    driver_number: usize,
    process: &dyn process::Process,
) -> AllocResult {
    let before_count = process.grant_allocated_count().unwrap_or(0);
    resources
        .syscall_driver_lookup()
        .with_driver(driver_number, |driver| match driver {
            Some(d) => match d.allocate_grant(process.processid()).is_ok() {
                true if before_count == process.grant_allocated_count().unwrap_or(0) => {
                    AllocResult::SameAllocation
                }
                true => AllocResult::NewAllocation,
                false => AllocResult::NoAllocation,
            },
            None => AllocResult::NoAllocation,
        })
}

impl Kernel {
    pub fn new(processes: &'static [Option<&'static dyn process::Process>]) -> Kernel {
        Kernel {
            work: Cell::new(0),
            processes,
            process_identifier_max: Cell::new(0),
            grant_counter: Cell::new(0),
            grants_finalized: Cell::new(false),
        }
    }

    /// 为某个流程安排了一些事情，因此还有更多工作要做
    ///
    /// 这仅在核心内核 crate 中公开
    pub(crate) fn increment_work(&self) {
        self.work.increment();
    }

    /// 为某个流程安排了一些事情，因此还有更多工作要做
    /// 这是公开的，但有能力限制。
    /// 目的是“Process”的外部实现需要能够表明还有更多流程工作要做。
    pub fn increment_work_external(
        &self,
        _capability: &dyn capabilities::ExternalProcessCapability,
    ) {
        self.increment_work();
    }

    /// 对于一个进程，一些事情已经完成，所以我们减少了有多少工作要做
    ///
    /// 这仅在核心内核 crate 中公开。
    pub(crate) fn decrement_work(&self) {
        self.work.decrement();
    }

    /// Something finished for a process, so we decrement how much work there is
    /// to do.
    ///
    /// This is exposed publicly, but restricted with a capability.
    /// 目的是“Process”的外部实现需要能够表明有多少工作已经完成。
    pub fn decrement_work_external(
        &self,
        _capability: &dyn capabilities::ExternalProcessCapability,
    ) {
        self.decrement_work();
    }

    /// 帮助函数，用于确定我们是否应该为进程提供服务或进入睡眠状态。
    pub(crate) fn processes_blocked(&self) -> bool {
        self.work.get() == 0
    }

    /// 帮助函数将 process_map_or 的所有非泛型部分移动到非泛型函数中，
    /// 以减少单态化导致的代码膨胀。
    pub(crate) fn get_process(&self, processid: ProcessId) -> Option<&dyn process::Process> {
        // 我们在 `appid` 中使用索引，所以我们可以直接查找。
        // 但是，我们不能保证应用程序仍然存在于进程数组中的该索引处。
        // 为了避免额外的开销，我们在这里进行查找和检查，而不是调用`.index()`。
        match self.processes.get(processid.index) {
            Some(Some(process)) => {
                // 检查此处存储的进程是否与 `appid` 中的标识符匹配。
                if process.processid() == processid {
                    Some(*process)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// 如果存在，则对特定进程运行闭包。
    /// 如果在 `ProcessId` 中指定的索引处不存在具有匹配 `ProcessId` 的进程，
    /// 则将返回 `default`。
    ///
    /// 如果进程被remove（并且进程数组中有一个“None”），
    /// 如果进程更改了它的标识符（可能在重新启动之后），
    /// 或者如果进程被移动到不同的索引，则不会找到匹配项进程数组。
    /// 请注意，如果进程仍然存在于数组中的正确位置但处于任何“停止”状态，则将找到匹配项。
    pub(crate) fn process_map_or<F, R>(&self, default: R, appid: ProcessId, closure: F) -> R
    where
        F: FnOnce(&dyn process::Process) -> R,
    {
        match self.get_process(appid) {
            Some(process) => closure(process),
            None => default,
        }
    }

    /// Run a closure on a specific process if it exists. If the process with a
    /// matching `ProcessId` does not exist at the index specified within the
    /// `ProcessId`, then `default` will be returned.
    ///
    /// A match will not be found if the process was removed (and there is a
    /// `None` in the process array), if the process changed its identifier
    /// (likely after being restarted), or if the process was moved to a
    /// different index in the processes array. Note that a match _will_ be
    /// found if the process still exists in the correct location in the array
    /// but is in any "stopped" state.
    ///
    /// 这在功能上与 `process_map_or()` 相同，但此方法在kernel crate 之外可用，
    /// 并且需要 `ProcessManagementCapability` 才能使用。
    pub fn process_map_or_external<F, R>(
        &self,
        default: R,
        appid: ProcessId,
        closure: F,
        _capability: &dyn capabilities::ProcessManagementCapability,
    ) -> R
    where
        F: FnOnce(&dyn process::Process) -> R,
    {
        match self.get_process(appid) {
            Some(process) => closure(process),
            None => default,
        }
    }

    /// 在每个有效进程上运行一个闭包。 这将迭代进程数组并在每个存在的进程上调用闭包。
    pub(crate) fn process_each<F>(&self, mut closure: F)
    where
        F: FnMut(&dyn process::Process),
    {
        for process in self.processes.iter() {
            match process {
                Some(p) => {
                    closure(*p);
                }
                None => {}
            }
        }
    }

    /// 返回内核加载的所有进程的迭代器
    pub(crate) fn get_process_iter(
        &self,
    ) -> core::iter::FilterMap<
        core::slice::Iter<Option<&dyn process::Process>>,
        fn(&Option<&'static dyn process::Process>) -> Option<&'static dyn process::Process>,
    > {
        fn keep_some(
            &x: &Option<&'static dyn process::Process>,
        ) -> Option<&'static dyn process::Process> {
            x
        }
        self.processes.iter().filter_map(keep_some)
    }

    /// 在每个有效进程上运行一个闭包。 这将迭代进程数组并在每个存在的进程上调用闭包。
    ///
    /// 这在功能上与 `process_each()` 相同，
    /// 但此方法在内核 crate 之外可用，并且需要使用 `ProcessManagementCapability`。
    pub fn process_each_capability<F>(
        &'static self,
        _capability: &dyn capabilities::ProcessManagementCapability,
        mut closure: F,
    ) where
        F: FnMut(&dyn process::Process),
    {
        for process in self.processes.iter() {
            match process {
                Some(p) => {
                    closure(*p);
                }
                None => {}
            }
        }
    }

    /// 在每个进程上运行一个闭包，但只有在闭包返回“None”时才继续。
    /// 也就是说，如果闭包返回任何非“None”值，则迭代停止并将此函数的值返回给被调用者。
    pub(crate) fn process_until<T, F>(&self, closure: F) -> Option<T>
    where
        F: Fn(&dyn process::Process) -> Option<T>,
    {
        for process in self.processes.iter() {
            match process {
                Some(p) => {
                    let ret = closure(*p);
                    if ret.is_some() {
                        return ret;
                    }
                }
                None => {}
            }
        }
        None
    }

    /// 给定存储在进程数组中的进程，检查提供的“ProcessId”是否仍然有效。
    /// 如果 ProcessId 仍然引用有效进程，则返回 `true`，否则返回 `false`。
    ///
    /// `ProcessId` 本身需要执行 `.index()` 命令来验证引用的应用程序是否仍在正确的索引处。
    pub(crate) fn processid_is_valid(&self, appid: &ProcessId) -> bool {
        self.processes.get(appid.index).map_or(false, |p| {
            p.map_or(false, |process| process.processid().id() == appid.id())
        })
    }

    /// 创建一个新的Grant。 这用于板级初始化以设置Capsules用于与Process交互的Grant。
    ///
    /// Grant**必须**仅在_before_进程被初始化时创建。进程使用已分配的Grant数量来正确初始化进程的内存，
    /// 每个Grant都有一个指针。 如果在进程初始化后创建授权，这将出现Panic。
    ///
    /// 调用此函数仅限于某些用户，并且要强制执行此调用此函数需要 `MemoryAllocationCapability` 能力。
    pub fn create_grant<
        T: Default,
        Upcalls: UpcallSize,
        AllowROs: AllowRoSize,
        AllowRWs: AllowRwSize,
    >(
        &'static self,
        driver_num: usize,
        _capability: &dyn capabilities::MemoryAllocationCapability,
    ) -> Grant<T, Upcalls, AllowROs, AllowRWs> {
        if self.grants_finalized.get() {
            panic!("Grants finalized. Cannot create a new grant.");
        }

        // 创建并返回一个新的Grant
        let grant_index = self.grant_counter.get();
        self.grant_counter.increment();
        Grant::new(self, driver_num, grant_index)
    }

    /// 返回系统中已设置的Grant数量，并将Grant标记为“已完成”。
    /// 这意味着不能再创建Grant，因为在调用此函数时已根据Grant数量设置了数据结构。
    /// 实际上，这是在创建进程时调用的，并且进程内存是根据当前Grant的数量设置的。
    pub(crate) fn get_grant_count_and_finalize(&self) -> usize {
        self.grants_finalized.set(true);
        self.grant_counter.get()
    }

    /// 返回系统中已设置的Grant数量，并将Grant标记为“已完成”。
    /// 这意味着不能再创建Grant，因为在调用此函数时已根据Grant数量设置了数据结构。

    /// 实际上，这是在创建进程时调用的，并且进程内存是根据当前Grant的数量设置的。
    /// Pub，但有能力限制。 目的是“Process”的外部实现需要能够检索最终的Grant数量。
    pub fn get_grant_count_and_finalize_external(
        &self,
        _capability: &dyn capabilities::ExternalProcessCapability,
    ) -> usize {
        self.get_grant_count_and_finalize()
    }

    /// 为进程创建一个新的唯一标识符并返回该标识符。
    ///
    /// 通常我们只选择一个比我们之前用于任何进程的更大的数字，以确保标识符是唯一的。
    pub(crate) fn create_process_identifier(&self) -> usize {
        self.process_identifier_max.get_and_increment()
    }

    /// 导致所有应用程序出现故障。
    ///
    /// 这将在每个应用程序上调用 `set_fault_state()`，导致应用程序进入状态，就好像它已经崩溃（例如 MPU 违规）。
    /// 如果该进程被配置为重新启动，它将被重新启动。
    ///
    /// 只有具有 `ProcessManagementCapability` 的调用者才能调用此函数。
    /// 这限制了一般Capsules能够调用此函数，因为Capsules不应该能够任意重启所有应用程序。
    pub fn hardfault_all_apps<C: capabilities::ProcessManagementCapability>(&self, _c: &C) {
        for p in self.processes.iter() {
            p.map(|process| {
                process.set_fault_state();
            });
        }
    }

    /// 执行核心 Tock 内核循环的一次迭代。
    ///
    /// 该函数负责三个主要操作：
    ///
    /// 1. 检查内核本身是否有任何工作要完成，以及调度程序是否想立即完成该工作。
    /// 如果是这样，它允许内核运行它
    /// 2. 检查是否有任何进程有任何工作要完成，
    /// 如果是，调度程序是否要允许任何进程现在运行，如果是，是哪一个。
    /// 3. 在确保调度程序不想完成任何内核或进程工作（或没有工作要做）之后，
    /// 是否没有未处理的中断需要处理，让芯片进入睡眠状态。
    ///
    /// 这个函数有一个配置选项：`no_sleep`。
    /// 如果该参数设置为 true，内核将永远不会尝试使芯片进入睡眠状态，并且可以立即再次调用该函数。
    pub fn kernel_loop_operation<KR: KernelResources<C>, C: Chip, const NUM_PROCS: usize>(
        &self,
        resources: &KR,
        chip: &C,
        ipc: Option<&ipc::IPC<NUM_PROCS>>,
        no_sleep: bool,
        _capability: &dyn capabilities::MainLoopCapability,
    ) {
        let scheduler = resources.scheduler();

        resources.watchdog().tickle();
        unsafe {
            // 询问调度程序我们是否应该在内核内部执行任务，例如处理中断。
            // 调度程序可能想要优先处理进程，或者可能没有内核工作要做。
            match scheduler.do_kernel_work_now(chip) {
                true => {
                    // 执行内核工作。
                    // 这包括处理中断，并且是Chips/Capsules crates中的代码能够执行的方式。
                    scheduler.execute_kernel_work(chip);
                }
                false => {
                    // 没有准备好内核工作，所以向调度程序询问一个进程。
                    match scheduler.next(self) {
                        SchedulingDecision::RunProcess((appid, timeslice_us)) => {
                            self.process_map_or((), appid, |process| {
                                let (reason, time_executed) =
                                    self.do_process(resources, chip, process, ipc, timeslice_us);
                                scheduler.result(reason, time_executed);
                            });
                        }
                        SchedulingDecision::TrySleep => {
                            // 对于测试，禁用休眠芯片可能会有所帮助，以防运行测试不产生任何中断。
                            if !no_sleep {
                                chip.atomic(|| {
                                    // 如果中断Pending，则无法休眠，因为在大多数平台上，未处理的中断会唤醒设备。
                                    // 此外，如果唯一的Pending中断发生在调度程序决定让芯片进入睡眠状态之后，
                                    // 但在这个Atomic部分开始之前，中断将不会被服务并且芯片永远不会从睡眠中唤醒。
                                    if !chip.has_pending_interrupts()
                                        && !DynamicDeferredCall::global_instance_calls_pending()
                                            .unwrap_or(false)
                                    {
                                        resources.watchdog().suspend();
                                        chip.sleep();
                                        resources.watchdog().resume();
                                    }
                                });
                            }
                        }
                    }
                }
            }
        }
    }

    /// 操作系统的主循环。
    ///
    /// 此循环的大部分行为由正在使用的“调度程序”实现控制。
    pub fn kernel_loop<KR: KernelResources<C>, C: Chip, const NUM_PROCS: usize>(
        &self,
        resources: &KR,
        chip: &C,
        ipc: Option<&ipc::IPC<NUM_PROCS>>,
        capability: &dyn capabilities::MainLoopCapability,
    ) -> ! {
        resources.watchdog().setup();
        loop {
            self.kernel_loop_operation(resources, chip, ipc, false, capability);
        }
    }

    /// 将控制权从内核转移到用户空间进程。
    ///
    /// 此函数由主内核循环调用以运行用户空间代码。
    /// 值得注意的是，来自进程的系统调用在内核中处理，*由内核线程*在此函数中处理，
    /// 并且立即为进程设置系统调用返回值。 通常，在调用系统调用后允许进程继续运行。
    /// 但是，调度程序会得到一个输出，
    /// 因为 `do_process()` 将在重新执行进程之前检查调度程序以允许它从系统调用返回。
    /// 如果一个进程在没有挂起的上行调用的情况下产生、退出、超过其时间片或被中断，
    /// 那么 `do_process()` 将返回。
    ///
    /// 根据使用的特定调度程序，此功能可能以几种不同的方式起作用。
    /// `scheduler.continue_process()` 允许调度程序告诉内核是继续执行进程，
    /// 还是在内核任务准备好（下半部中断处理程序或动态延迟调用）后立即将控制权返回给调度程序，
    /// 或者继续执行用户空间进程，直到达到上述停止条件之一。
    /// 一些调度器可能不需要调度器定时器； 为时间片传递“None”将使用Null调度器计时器，
    /// 即使芯片提供了real的调度器计时器。
    /// 调度程序可以传递他们选择的时间片（in tock），但如果传递的时间片小于“MIN_QUANTA_THRESHOLD_US”，
    /// 则该进程将不会执行，并且该函数将立即返回。
    ///
    /// 此函数返回一个元组，指示此函数返回调度程序的原因，
    /// 以及进程执行所花费的时间量（如果进程协作运行，则返回“None”）。
    /// 值得注意的是，内核在这个函数中花费的时间、执行系统调用或仅仅设置到/从用户空间的切换，都计入进程。
    fn do_process<KR: KernelResources<C>, C: Chip, const NUM_PROCS: usize>(
        &self,
        resources: &KR,
        chip: &C,
        process: &dyn process::Process,
        ipc: Option<&crate::ipc::IPC<NUM_PROCS>>,
        timeslice_us: Option<u32>,
    ) -> (StoppedExecutingReason, Option<u32>) {
        // 如果进程应该在没有任何时间片限制的情况下执行，我们必须使用虚拟调度程序计时器。
        // 请注意，即使请求时间片，芯片也可能无法提供real的调度器定时器实现。
        let scheduler_timer: &dyn SchedulerTimer = if timeslice_us.is_none() {
            &() // 虚拟定时器，无抢占
        } else {
            resources.scheduler_timer()
        };

        // 清除调度程序计时器，然后启动计数器。 这将启动进程的时间片。
        // 由于此时内核仍在执行，因此调度程序计时器不需要在“start()”之后启用中断。
        scheduler_timer.reset();
        timeslice_us.map(|timeslice| scheduler_timer.start(timeslice));

        // 需要跟踪进程不再执行的原因，以便我们可以通知调度程序。
        let mut return_reason = StoppedExecutingReason::NoWorkLeft;

        // 由于时间片计算了进程的执行时间和进程在内核中花费的时间（设置它并处理它
        // 的系统调用），我们打算继续运行该进程直到它没有更多的工作要做。
        // 如果调度程序不再想要执行这个进程或者如果它超过了它的时间片，我们就会跳出这个循环。
        loop {
            let stop_running = match scheduler_timer.get_remaining_us() {
                Some(us) => us <= MIN_QUANTA_THRESHOLD_US,
                None => true,
            };
            if stop_running {
                // 内核执行时进程超时。
                process.debug_timeslice_expired();
                return_reason = StoppedExecutingReason::TimesliceExpired;
                break;
            }

            // 检查调度程序是否希望继续运行此进程。
            let continue_process = unsafe {
                resources
                    .scheduler()
                    .continue_process(process.processid(), chip)
            };
            if !continue_process {
                return_reason = StoppedExecutingReason::KernelPreemption;
                break;
            }

            // 检查此过程是否实际上已准备好运行。 如果没有，我们不会尝试运行它。
            // 例如，如果进程出现故障并停止，则可能会发生这种情况。
            if !process.ready() {
                return_reason = StoppedExecutingReason::NoWorkLeft;
                break;
            }

            match process.get_state() {
                process::State::Running => {
                    // Running意味着该进程预计会运行，因此请继续进行设置并切换到执行该进程。
                    // 启用调度程序计时器会指示它在时间片到期时生成中断。 底层计时器不受影响。
                    resources
                        .context_switch_callback()
                        .context_switch_hook(process);
                    process.setup_mpu();
                    chip.mpu().enable_app_mpu();
                    scheduler_timer.arm();
                    let context_switch_reason = process.switch_to();
                    scheduler_timer.disarm();
                    chip.mpu().disable_app_mpu();

                    // 现在该进程已返回内核。 检查原因并酌情处理该过程。
                    match context_switch_reason {
                        Some(ContextSwitchReason::Fault) => {
                            // 应用程序出现故障，检查芯片是否要处理故障。
                            if resources
                                .process_fault()
                                .process_fault_hook(process)
                                .is_err()
                            {
                                // 让Process酌情处理。
                                process.set_fault_state();
                            }
                        }
                        Some(ContextSwitchReason::SyscallFired { syscall }) => {
                            self.handle_syscall(resources, process, syscall);
                        }
                        Some(ContextSwitchReason::Interrupted) => {
                            if scheduler_timer.get_remaining_us().is_none() {
                                // 此中断是时间片到期。
                                process.debug_timeslice_expired();
                                return_reason = StoppedExecutingReason::TimesliceExpired;
                                break;
                            }
                            // 转到循环的开头来决定是中断处理中断，继续执行这个进程，还是切换到另一个进程。
                            continue;
                        }
                        None => {
                            // 切换到此过程时出现问题。 通过将其置于故障状态来指示这一点。
                            process.set_fault_state();
                        }
                    }
                }
                process::State::Yielded | process::State::Unstarted => {
                    // 如果该Process已产生或尚未开始，则它正在等待Upcall。
                    // 如果有为此Process安排的任务，请继续并设置流程以执行它。
                    match process.dequeue_task() {
                        None => break,
                        Some(cb) => match cb {
                            Task::FunctionCall(ccb) => {
                                if config::CONFIG.trace_syscalls {
                                    debug!(
                                        "[{:?}] function_call @{:#x}({:#x}, {:#x}, {:#x}, {:#x})",
                                        process.processid(),
                                        ccb.pc,
                                        ccb.argument0,
                                        ccb.argument1,
                                        ccb.argument2,
                                        ccb.argument3,
                                    );
                                }
                                process.set_process_function(ccb);
                            }
                            Task::IPC((otherapp, ipc_type)) => {
                                ipc.map_or_else(
                                    || {
                                        assert!(
                                            false,
                                            "Kernel consistency error: IPC Task with no IPC"
                                        );
                                    },
                                    |ipc| {
                                        // TODO（alevy）：这可能由于多种原因而出错。
                                        // 我们是否应该以某种方式传达错误https://github.com/tock/tock/issues/1993
                                        unsafe {
                                            let _ = ipc.schedule_upcall(
                                                process.processid(),
                                                otherapp,
                                                ipc_type,
                                            );
                                        }
                                    },
                                );
                            }
                        },
                    }
                }
                process::State::Faulted | process::State::Terminated => {
                    // 我们永远不应该安排一个错误的进程。
                    panic!("Attempted to schedule a faulty process");
                }
                process::State::StoppedRunning => {
                    return_reason = StoppedExecutingReason::Stopped;
                    break;
                }
                process::State::StoppedYielded => {
                    return_reason = StoppedExecutingReason::Stopped;
                    break;
                }
            }
        }

        // 检查进程在执行时使用了多少时间，并返回该值，以便我们可以将其提供给调度程序。
        let time_executed_us = timeslice_us.map_or(None, |timeslice| {
            // 注意，如果 .get_remaining_us() 之前返回了 `None`，我们不能再次调用它，所以我们_必须_首先检查返回原因。
            if return_reason == StoppedExecutingReason::TimesliceExpired {
                // 使用了整个时间片
                Some(timeslice)
            } else {
                match scheduler_timer.get_remaining_us() {
                    Some(remaining) => Some(timeslice - remaining),
                    None => Some(timeslice), // used whole timeslice
                }
            }
        });

        // 重置调度程序计时器，以防它在到期时无条件地触发中断。
        // 例如，我们不希望它在芯片休眠时过期。
        scheduler_timer.reset();

        (return_reason, time_executed_us)
    }

    /// 在特定Process上调用系统调用的方法。 应用内核系统调用过滤策略（如果有）。
    /// 处理 `Yield` 和 `Exit`，将 `Memop` 分派到 `memop::memop`，
    /// 并通过平台 `with_driver` 方法将外围驱动系统调用分派到外围驱动封装。
    #[inline]
    fn handle_syscall<KR: KernelResources<C>, C: Chip>(
        &self,
        resources: &KR,
        process: &dyn process::Process,
        syscall: Syscall,
    ) {
        // 用于进程调试的钩子。
        process.debug_syscall_called(syscall);

        // 在此处强制执行特定于平台的系统调用过滤。
        //
        // 在继续处理 non-yield 系统调用之前，内核首先检查平台是否要阻止该进程的系统调用，
        // 如果是，则设置一个返回值，该值返回给调用进程。
        //
        // 过滤系统调用（即阻止系统调用运行）不会导致进程丢失其时间片。
        // 错误将立即返回（假设进程尚未耗尽其时间片），允许进程决定如何处理错误。
        match syscall {
            Syscall::Yield {
                which: _,
                address: _,
            } => {} // Yield is not filterable.
            Syscall::Exit {
                which: _,
                completion_code: _,
            } => {} // Exit is not filterable.
            Syscall::Memop {
                operand: _,
                arg0: _,
            } => {} // Memop is not filterable.
            _ => {
                // Check all other syscalls for filtering.
                if let Err(response) = resources.syscall_filter().filter_syscall(process, &syscall)
                {
                    process.set_syscall_return_value(SyscallReturn::Failure(response));

                    if config::CONFIG.trace_syscalls {
                        debug!(
                            "[{:?}] Filtered: {:?} was rejected with {:?}",
                            process.processid(),
                            syscall,
                            response
                        );
                    }

                    return;
                }
            }
        }

        // Handle each of the syscalls.
        match syscall {
            Syscall::Memop { operand, arg0 } => {
                let rval = memop::memop(process, operand, arg0);
                if config::CONFIG.trace_syscalls {
                    debug!(
                        "[{:?}] memop({}, {:#x}) = {:?}",
                        process.processid(),
                        operand,
                        arg0,
                        rval
                    );
                }
                process.set_syscall_return_value(rval);
            }
            Syscall::Yield { which, address } => {
                if config::CONFIG.trace_syscalls {
                    debug!("[{:?}] yield. which: {}", process.processid(), which);
                }
                if which > (YieldCall::Wait as usize) {
                    // 只有 0 和 1 有效，所以这不是有效的 yield 系统调用，Yield 没有返回值，
                    // 因为它可以将函数调用压入堆栈； 只需将控制权交还给Process即可。
                    return;
                }
                let wait = which == (YieldCall::Wait as usize);
                // 如果这是一个 yield-no-wait 并且没有待处理的任务，则立即返回。
                // 否则，进入yield状态并立即执行任务或在任务到达时执行。
                let return_now = !wait && !process.has_tasks();
                if return_now {
                    // 将“我是否触发了Upcall”标志设置为 0，立即返回。如果地址无效，则什么也不做。
                    //
                    // # Safety
                    //
                    // 只要不存在对进程内存的引用，这很好。 我们没有引用，所以我们可以安全地调用`set_byte()`。
                    unsafe {
                        process.set_byte(address, 0);
                    }
                } else {
                    // 已经有排队的Upcall要执行，或者我们应该等待它们：在下一个循环迭代中
                    // 处理并将“我是否触发Upcall”标志设置为 1。如果地址无效，则不执行任何操作。
                    //
                    // # Safety
                    //
                    // This is fine as long as no references to the process's
                    // memory exist. We do not have a reference, so we can
                    // safely call `set_byte()`.
                    unsafe {
                        process.set_byte(address, 1);
                    }
                    process.set_yielded_state();
                }
            }
            Syscall::Subscribe {
                driver_number,
                subdriver_number,
                upcall_ptr,
                appdata,
            } => {
                // upcall 被标识为驱动程序编号和子驱动程序编号的元组.
                let upcall_id = UpcallId {
                    driver_num: driver_number,
                    subscribe_num: subdriver_number,
                };

                // 首先检查 `upcall_ptr` 是否为空
                // 一个空的 `upcall_ptr` 将在此处产生 `None` 并表示特殊的“取消订阅”操作。
                let ptr = NonNull::new(upcall_ptr);

                // 为方便起见，现在创建一个 `Upcall` 类型。 这只是一个数据结构，不做任何检查或转换。
                let upcall = Upcall::new(process.processid(), upcall_id, appdata, ptr);

                // 如果 `ptr` 不为 null，我们必须首先验证 upcall 函数指针是否在进程可访问内存中。
                // 根据 TRD104：

                // > If the passed upcall is not valid (is outside process
                // > executable memory...), the kernel...MUST immediately return
                // > a failure with a error code of `INVALID`.
                let rval1 = ptr.map_or(None, |upcall_ptr_nonnull| {
                    if !process.is_valid_upcall_function_pointer(upcall_ptr_nonnull) {
                        Some(ErrorCode::INVAL)
                    } else {
                        None
                    }
                });

                // 如果 upcall 为 null 或有效，那么我们继续处理 upcall。
                let rval = match rval1 {
                    Some(err) => upcall.into_subscribe_failure(err),
                    None => {
                        // 此时我们必须保存新的 upcall 并返回旧的。
                        // Upcall由核心内核存储在Grant区域中，因此我们可以保证正确的Upcall交换。
                        // 但是，如果以前从未使用过此驱动程序，我们确实需要帮助来初始分配Grant。
                        //
                        // 为了避免检查Process liveness 和Grant分配的开销，我们假设Grant是最初分配的。
                        // 如果事实证明不是我们要求Capsule allocate Grant。
                        match crate::grant::subscribe(process, upcall) {
                            Ok(upcall) => upcall.into_subscribe_success(),
                            Err((upcall, err @ ErrorCode::NOMEM)) => {
                                // 如果我们遇到内存错误，我们总是尝试分配Grant，因为这可能是第一次访问Grant。
                                match try_allocate_grant(resources, driver_number, process) {
                                    AllocResult::NewAllocation => {
                                        // 现在我们再试一次。有可能Capsule实际上没有分配Grant，
                                        // 此时这将再次失败，我们将错误返回给用户空间。
                                        match crate::grant::subscribe(process, upcall) {
                                            // Ok() 返回上一个Upcall，而 Err() 返回刚刚传递的Upcall。
                                            Ok(upcall) => upcall.into_subscribe_success(),
                                            Err((upcall, err)) => {
                                                upcall.into_subscribe_failure(err)
                                            }
                                        }
                                    }
                                    alloc_failure => {
                                        // 我们实际上并没有创建一个新的分配，所以只是错误。
                                        match (config::CONFIG.trace_syscalls, alloc_failure) {
                                            (true, AllocResult::NoAllocation) => {
                                                debug!("[{:?}] WARN driver #{:x} did not allocate grant",
                                                                           process.processid(), driver_number);
                                            }
                                            (true, AllocResult::SameAllocation) => {
                                                debug!("[{:?}] ERROR driver #{:x} allocated wrong grant counts",
                                                                           process.processid(), driver_number);
                                            }
                                            _ => {}
                                        }
                                        upcall.into_subscribe_failure(err)
                                    }
                                }
                            }
                            Err((upcall, err)) => upcall.into_subscribe_failure(err),
                        }
                    }
                };

                // 根据 TRD104，我们仅在订阅返回成功时才清除Upcall。
                // 在这一点上，我们知道结果并在必要时清除。
                if rval.is_success() {
                    // 每个元组应该只存在一个Upcall。
                    // 为了确保没有具有相同标识符但使用旧函数指针的Pending Upcall，我们现在清除它们。
                    process.remove_pending_upcalls(upcall_id);
                }

                if config::CONFIG.trace_syscalls {
                    debug!(
                        "[{:?}] subscribe({:#x}, {}, @{:#x}, {:#x}) = {:?}",
                        process.processid(),
                        driver_number,
                        subdriver_number,
                        upcall_ptr as usize,
                        appdata,
                        rval
                    );
                }

                process.set_syscall_return_value(rval);
            }
            Syscall::Command {
                driver_number,
                subdriver_number,
                arg0,
                arg1,
            } => {
                let cres = resources
                    .syscall_driver_lookup()
                    .with_driver(driver_number, |driver| match driver {
                        Some(d) => d.command(subdriver_number, arg0, arg1, process.processid()),
                        None => CommandReturn::failure(ErrorCode::NODEVICE),
                    });

                let res = SyscallReturn::from_command_return(cres);

                if config::CONFIG.trace_syscalls {
                    debug!(
                        "[{:?}] cmd({:#x}, {}, {:#x}, {:#x}) = {:?}",
                        process.processid(),
                        driver_number,
                        subdriver_number,
                        arg0,
                        arg1,
                        res,
                    );
                }
                process.set_syscall_return_value(res);
            }
            Syscall::ReadWriteAllow {
                driver_number,
                subdriver_number,
                allow_address,
                allow_size,
            } => {
                // 尝试创建一个适当的 [`ReadWriteProcessBuffer`]。
                // 这种方法将确保有问题的内存位于进程可访问的内存空间中。
                let res = match process.build_readwrite_process_buffer(allow_address, allow_size) {
                    Ok(rw_pbuf) => {
                        // 创建 [`ReadWriteProcessBuffer`] 有效，尝试在Grant中设置。
                        match crate::grant::allow_rw(
                            process,
                            driver_number,
                            subdriver_number,
                            rw_pbuf,
                        ) {
                            Ok(rw_pbuf) => {
                                let (ptr, len) = rw_pbuf.consume();
                                SyscallReturn::AllowReadWriteSuccess(ptr, len)
                            }
                            Err((rw_pbuf, err @ ErrorCode::NOMEM)) => {
                                // 如果我们遇到内存错误，我们总是尝试分配Grant，因为这可能是第一次访问Grant。
                                match try_allocate_grant(resources, driver_number, process) {
                                    AllocResult::NewAllocation => {
                                        // 如果我们真的分配了新的Grant，请再试一次并尊重结果。
                                        match crate::grant::allow_rw(
                                            process,
                                            driver_number,
                                            subdriver_number,
                                            rw_pbuf,
                                        ) {
                                            Ok(rw_pbuf) => {
                                                let (ptr, len) = rw_pbuf.consume();
                                                SyscallReturn::AllowReadWriteSuccess(ptr, len)
                                            }
                                            Err((rw_pbuf, err)) => {
                                                let (ptr, len) = rw_pbuf.consume();
                                                SyscallReturn::AllowReadWriteFailure(err, ptr, len)
                                            }
                                        }
                                    }
                                    alloc_failure => {
                                        // We didn't actually create a new
                                        // alloc, so just error.
                                        match (config::CONFIG.trace_syscalls, alloc_failure) {
                                            (true, AllocResult::NoAllocation) => {
                                                debug!("[{:?}] WARN driver #{:x} did not allocate grant",
                                                                           process.processid(), driver_number);
                                            }
                                            (true, AllocResult::SameAllocation) => {
                                                debug!("[{:?}] ERROR driver #{:x} allocated wrong grant counts",
                                                                           process.processid(), driver_number);
                                            }
                                            _ => {}
                                        }
                                        let (ptr, len) = rw_pbuf.consume();
                                        SyscallReturn::AllowReadWriteFailure(err, ptr, len)
                                    }
                                }
                            }
                            Err((rw_pbuf, err)) => {
                                let (ptr, len) = rw_pbuf.consume();
                                SyscallReturn::AllowReadWriteFailure(err, ptr, len)
                            }
                        }
                    }
                    Err(allow_error) => {
                        // 创建 [`ReadWriteProcessBuffer`] 时出错。 使用原始参数向Process报告。
                        SyscallReturn::AllowReadWriteFailure(allow_error, allow_address, allow_size)
                    }
                };

                if config::CONFIG.trace_syscalls {
                    debug!(
                        "[{:?}] read-write allow({:#x}, {}, @{:#x}, {}) = {:?}",
                        process.processid(),
                        driver_number,
                        subdriver_number,
                        allow_address as usize,
                        allow_size,
                        res
                    );
                }
                process.set_syscall_return_value(res);
            }
            Syscall::UserspaceReadableAllow {
                driver_number,
                subdriver_number,
                allow_address,
                allow_size,
            } => {
                let res = resources
                    .syscall_driver_lookup()
                    .with_driver(driver_number, |driver| match driver {
                        Some(d) => {
                            // 尝试创建一个合适的 [`UserspaceReadableProcessBuffer`]。
                            // 这种方法将确保有问题的内存位于进程可访问的内存空间中。
                            match process.build_readwrite_process_buffer(allow_address, allow_size)
                            {
                                Ok(rw_pbuf) => {
                                    //创建 [`UserspaceReadableProcessBuffer`] 工作，将其提供给Capsule。
                                    match d.allow_userspace_readable(
                                        process.processid(),
                                        subdriver_number,
                                        rw_pbuf,
                                    ) {
                                        Ok(returned_pbuf) => {
                                            // Capsule已接受允许操作。将先前的缓冲区信息传递回进程。
                                            let (ptr, len) = returned_pbuf.consume();
                                            SyscallReturn::UserspaceReadableAllowSuccess(ptr, len)
                                        }
                                        Err((rejected_pbuf, err)) => {
                                            // Capsule拒绝了允许操作。 将新的缓冲区信息传递回进程。
                                            let (ptr, len) = rejected_pbuf.consume();
                                            SyscallReturn::UserspaceReadableAllowFailure(
                                                err, ptr, len,
                                            )
                                        }
                                    }
                                }
                                Err(allow_error) => {
                                    // 创建 [`UserspaceReadableProcessBuffer`] 时出错。 向进程报告。
                                    SyscallReturn::UserspaceReadableAllowFailure(
                                        allow_error,
                                        allow_address,
                                        allow_size,
                                    )
                                }
                            }
                        }

                        None => SyscallReturn::UserspaceReadableAllowFailure(
                            ErrorCode::NODEVICE,
                            allow_address,
                            allow_size,
                        ),
                    });

                if config::CONFIG.trace_syscalls {
                    debug!(
                        "[{:?}] userspace readable allow({:#x}, {}, @{:#x}, {}) = {:?}",
                        process.processid(),
                        driver_number,
                        subdriver_number,
                        allow_address as usize,
                        allow_size,
                        res
                    );
                }
                process.set_syscall_return_value(res);
            }
            Syscall::ReadOnlyAllow {
                driver_number,
                subdriver_number,
                allow_address,
                allow_size,
            } => {
                // 尝试创建一个适当的 [`ReadOnlyProcessBuffer`]。
                // 这种方法将确保有问题的内存位于进程可访问的内存空间中。
                let res = match process.build_readonly_process_buffer(allow_address, allow_size) {
                    Ok(ro_pbuf) => {
                        // 创建 [`ReadOnlyProcessBuffer`] 有效，尝试在Grant中设置。
                        match crate::grant::allow_ro(
                            process,
                            driver_number,
                            subdriver_number,
                            ro_pbuf,
                        ) {
                            Ok(ro_pbuf) => {
                                let (ptr, len) = ro_pbuf.consume();
                                SyscallReturn::AllowReadOnlySuccess(ptr, len)
                            }
                            Err((ro_pbuf, err @ ErrorCode::NOMEM)) => {
                                // If we get a memory error, we always try to
                                // allocate the grant since this could be the
                                // first time the grant is getting accessed.
                                match try_allocate_grant(resources, driver_number, process) {
                                    AllocResult::NewAllocation => {
                                        // If we actually allocated a new grant,
                                        // try again and honor the result.
                                        match crate::grant::allow_ro(
                                            process,
                                            driver_number,
                                            subdriver_number,
                                            ro_pbuf,
                                        ) {
                                            Ok(ro_pbuf) => {
                                                let (ptr, len) = ro_pbuf.consume();
                                                SyscallReturn::AllowReadOnlySuccess(ptr, len)
                                            }
                                            Err((ro_pbuf, err)) => {
                                                let (ptr, len) = ro_pbuf.consume();
                                                SyscallReturn::AllowReadOnlyFailure(err, ptr, len)
                                            }
                                        }
                                    }
                                    alloc_failure => {
                                        // We didn't actually create a new
                                        // alloc, so just error.
                                        match (config::CONFIG.trace_syscalls, alloc_failure) {
                                            (true, AllocResult::NoAllocation) => {
                                                debug!("[{:?}] WARN driver #{:x} did not allocate grant",
                                                                           process.processid(), driver_number);
                                            }
                                            (true, AllocResult::SameAllocation) => {
                                                debug!("[{:?}] ERROR driver #{:x} allocated wrong grant counts",
                                                                           process.processid(), driver_number);
                                            }
                                            _ => {}
                                        }
                                        let (ptr, len) = ro_pbuf.consume();
                                        SyscallReturn::AllowReadOnlyFailure(err, ptr, len)
                                    }
                                }
                            }
                            Err((ro_pbuf, err)) => {
                                let (ptr, len) = ro_pbuf.consume();
                                SyscallReturn::AllowReadOnlyFailure(err, ptr, len)
                            }
                        }
                    }
                    Err(allow_error) => {
                        // There was an error creating the
                        // [`ReadOnlyProcessBuffer`]. Report back to the process
                        // with the original parameters.
                        SyscallReturn::AllowReadOnlyFailure(allow_error, allow_address, allow_size)
                    }
                };

                if config::CONFIG.trace_syscalls {
                    debug!(
                        "[{:?}] read-only allow({:#x}, {}, @{:#x}, {}) = {:?}",
                        process.processid(),
                        driver_number,
                        subdriver_number,
                        allow_address as usize,
                        allow_size,
                        res
                    );
                }

                process.set_syscall_return_value(res);
            }
            Syscall::Exit {
                which,
                completion_code,
            } => match which {
                // 进程调用了 `exit-terminate` 系统调用。
                0 => process.terminate(Some(completion_code as u32)),
                // 该进程称为“exit-restart”系统调用。
                1 => process.try_restart(Some(completion_code as u32)),
                // 进程调用了 Exitsystem 调用类的无效变体。
                _ => process.set_syscall_return_value(SyscallReturn::Failure(ErrorCode::NOSUPPORT)),
            },
        }
    }
}
