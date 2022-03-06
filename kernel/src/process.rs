//! Tock-compatible进程的类型。

use core::cell::Cell;
use core::fmt;
use core::fmt::Write;
use core::ptr::NonNull;
use core::str;

use crate::capabilities;
use crate::errorcode::ErrorCode;
use crate::ipc;
use crate::kernel::Kernel;
use crate::platform::mpu::{self};
use crate::processbuffer::{ReadOnlyProcessBuffer, ReadWriteProcessBuffer};
use crate::syscall::{self, Syscall, SyscallReturn};
use crate::upcall::UpcallId;
use tock_tbf::types::CommandPermissions;

// 通过 `kernel::process::` 导出所有与进程相关的类型。
pub use crate::process_policies::{
    PanicFaultPolicy, ProcessFaultPolicy, RestartFaultPolicy, StopFaultPolicy,
    StopWithDebugFaultPolicy, ThresholdRestartFaultPolicy, ThresholdRestartThenPanicFaultPolicy,
};
pub use crate::process_printer::{ProcessPrinter, ProcessPrinterContext, ProcessPrinterText};
pub use crate::process_standard::ProcessStandard;
pub use crate::process_utilities::{load_processes, load_processes_advanced, ProcessLoadError};

/// 用户空间进程标识符
///
/// 这应该被视为一种不透明类型，可用于表示board上的进程，而无需实际引用“进程”对象。
/// 拥有这个 `ProcessId` 引用类型对于管理 Rust 中的所有权和类型问题很有用，
/// 但更重要的是，`ProcessId` 用作Capsule保存指向应用程序的指针的工具。
///
/// 由于 `ProcessId` 实现了 `Copy`，拥有 `ProcessId` 并不确保 `ProcessId` 引用的进程仍然有效。
/// 该进程可能已被删除、终止或作为新进程重新启动。 因此，所有在内核中使用 `ProcessId`
/// 都必须检查 `ProcessId` 是否仍然有效。 如前所述，此检查在调用 .index() 时
/// 自动发生通过返回类型：`Option<usize>`。
/// `.index()` 将返回进程数组中进程的索引，但如果进程不再存在，则返回 `None`。
///
/// 在内核 crate 之外，`ProcessId` 的持有者可能希望使用 `.id()` 来检索可以通过
/// UART 总线或系统调用接口进行通信的进程的简单标识符。 此调用保证为“ProcessId”
/// 返回一个合适的标识符，但不检查相应的应用程序是否仍然存在。
///
/// 这种类型还为Capsule提供了一个与进程交互的接口，否则它们将不会引用“进程”。
/// 通过这个接口可以进行非常有限的操作，因为Capsule不需要知道任何给定过程的细节。
/// 然而，某些信息使某些Capsule成为可能。
/// 例如，Capsule可以使用 `get_editable_flash_range()` 函数，这样它们就可以安全地
/// 允许应用修改自己的闪存。
#[derive(Clone, Copy)]
pub struct ProcessId {
    /// 对主要内核结构的引用。 这是检查被引用应用程序的
    /// 某些属性（如其可编辑边界）所必需的，也是检查索引是否有效所必需的。
    pub(crate) kernel: &'static Kernel,

    /// kernel.PROCESSES[] 数组中存储此应用程序状态的索引。
    /// 这有助于快速查找流程并有助于实施 IPC。
    ///
    /// 这个值是 crate 可见的，可以在 sched.rs 中启用优化。 其他用户应改为调用 `.index()`。
    pub(crate) index: usize,

    /// 此进程的唯一标识符。
    /// 这可用于在需要单个数字的情况下引用进程，例如在跨系统调用接口引用特定应用程序时。
    ///
    /// (index, identifier) 的组合用于检查这个 `ProcessId` 引用的应用程序是否仍然有效。
    /// 如果给定索引处进程中存储的标识符与此处保存的值不匹配，则进程移动或以其他方式结束，
    /// 并且此“ProcessId”不再有效。
    identifier: usize,
}

impl PartialEq for ProcessId {
    fn eq(&self, other: &ProcessId) -> bool {
        self.identifier == other.identifier
    }
}

impl Eq for ProcessId {}

impl fmt::Debug for ProcessId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.identifier)
    }
}

impl ProcessId {
    /// Create a new `ProcessId` object based on the app identifier and its
    /// index in the processes array.
    pub(crate) fn new(kernel: &'static Kernel, identifier: usize, index: usize) -> ProcessId {
        ProcessId {
            kernel: kernel,
            identifier: identifier,
            index: index,
        }
    }

    /// Create a new `ProcessId` object based on the app identifier and its
    /// index in the processes array.
    ///
    /// This constructor is public but protected with a capability so that
    /// external implementations of `Process` can use it.
    pub fn new_external(
        kernel: &'static Kernel,
        identifier: usize,
        index: usize,
        _capability: &dyn capabilities::ExternalProcessCapability,
    ) -> ProcessId {
        ProcessId {
            kernel: kernel,
            identifier: identifier,
            index: index,
        }
    }

    /// Get the location of this app in the processes array.
    ///
    /// This will return `Some(index)` if the identifier stored in this
    /// `ProcessId` matches the app saved at the known index. If the identifier
    /// does not match then `None` will be returned.
    pub(crate) fn index(&self) -> Option<usize> {
        // Do a lookup to make sure that the index we have is correct.
        if self.kernel.processid_is_valid(self) {
            Some(self.index)
        } else {
            None
        }
    }

    /// 获取此 `ProcessId` 引用的应用程序的 `usize` 唯一标识符。
    ///
    /// 通常不应该使用这个函数，而是代码应该只使用 `ProcessId` 对象本身来引用系统上的各种应用程序。
    /// 然而，当使用内核之外的东西引用特定应用程序时，仅获取一个 `usize` 标识符特别有用，
    /// 例如对于需要具体数字的用户空间（例如 IPC）或 tockloader（例如用于调试）。
    ///
    /// 请注意，这将始终为最初引用的应用程序返回保存的唯一标识符，即使该应用程序不再存在。
    /// 例如，应用程序可能已重新启动，或者可能已被内核终止或删除。
    /// 因此，调用 `id()` _不是_检查应用程序是否仍然存在的有效方法。
    pub fn id(&self) -> usize {
        self.identifier
    }

    /// 返回应用程序拥有并可以写入的falsh区域的开始和结束的完整地址。
    /// 包括应用程序的代码和数据以及应用程序末尾的任何填充。
    /// 它不包括 TBF 标头或内核用于任何潜在book-keeping的任何空间。
    pub fn get_editable_flash_range(&self) -> (usize, usize) {
        self.kernel.process_map_or((0, 0), *self, |process| {
            let addresses = process.get_addresses();
            (addresses.flash_non_protected_start, addresses.flash_end)
        })
    }
}

/// 此Trait表示 Tock 调度程序可以调度的通用进程。
pub trait Process {
    /// 返回进程的标识符。
    fn processid(&self) -> ProcessId;

    /// 向进程队列计入一个“任务”。 这将被添加到每个进程的缓冲区并由调度程序执行。
    /// `Task`s 是应用程序应该运行的一些功能，例如 upcall 或 IPC 调用。
    ///
    /// 如果 `Task` 成功入队，此函数返回 `Ok(())`。
    /// 如果进程不再存在，则返回 `Err(ErrorCode::NODEVICE)`。
    /// 如果由于内部任务队列空间不足导致任务无法入队，则返回 `Err(ErrorCode::NOMEM)`。
    /// 其他返回值必须被视为内核内部错误。
    fn enqueue_task(&self, task: Task) -> Result<(), ErrorCode>;

    /// 返回此进程是否已准备好执行。
    fn ready(&self) -> bool;

    /// 如果有任何任务（upcalls/IPC 请求）在队列中，则返回。
    fn has_tasks(&self) -> bool;

    /// 从队列的最前面移除调度的操作，并返回给调度器处理。
    ///
    /// 如果此进程的队列中没有“任务”，这将返回“None”。
    fn dequeue_task(&self) -> Option<Task>;

    /// 返回待处理任务的数量。 如果为 0，则 `dequeue_task()` 将在调用时返回 `None`。
    fn pending_tasks(&self) -> usize;

    /// 从任务队列中删除给定 upcall id 的所有在排队的 upcall。
    fn remove_pending_upcalls(&self, upcall_id: UpcallId);

    /// 返回进程所处的当前状态。常见状态是“running”或“yielded”。
    fn get_state(&self) -> State;

    /// 将此进程从running状态移动到yielded状态。
    ///
    /// 如果该进程之前没有运行，这将失败（即不做任何事情）。
    fn set_yielded_state(&self);

    /// 将此进程从running或yielded状态移动到stop状态。
    ///
    /// 如果进程没有running或yielded，这将失败（即不做任何事情）。
    fn stop(&self);

    /// 将此stop的进程移回其original状态。
    ///
    /// 这会将进程从 `StoppedRunning` -> `Running` 或 `StoppedYielded` -> `Yielded` 转换。
    fn resume(&self);

    /// 将此进程置于故障状态。 这将触发此过程发生的“FaultResponse”。
    fn set_fault_state(&self);

    /// 返回此进程已重新启动的次数。
    fn get_restart_count(&self) -> usize;

    /// 获取进程的名称。 用于IPC。
    fn get_process_name(&self) -> &'static str;

    /// 如果进程先前已终止，则获取完成代码。
    ///
    /// 如果进程从未终止，则没有机会设置完成代码，这将返回“None”。
    ///
    /// 如果进程先前已经终止，这将返回 `Some()`。 如果进程最后一次终止它没有提供完成代码
    /// （例如进程出现故障），那么这将返回 `Some(None)`。
    /// 如果进程最后一次终止它确实提供了一个完成代码，这将返回 `Some(Some(completion_code))`。
    fn get_completion_code(&self) -> Option<Option<u32>>;

    /// 停止并清除进程的状态，将其置于“Terminated”状态。
    ///
    /// 这将结束该过程，但不会重置它，以便它可以重新启动并再次运行。
    /// 相反，此函数释放此进程的授权和任何排队的任务，
    /// 但保留有关进程和其他状态的调试信息不变。
    ///
    /// 当一个进程终止时，应该为该进程存储一个可选的“completion_code”。
    /// 如果进程提供了完成代码（例如，通过退出系统调用），
    /// 则应使用完成代码“Some(u32)”调用此函数。
    /// 如果内核正在终止进程并且因此没有来自进程的完成代码，它应该提供“None”。
    fn terminate(&self, completion_code: Option<u32>);

    /// 通过不再安排进程运行来停止进程。终止并尝试重新启动进程。 进程和当前应用程序总是终止。
    /// 内核可以根据自己的策略，使用相同的进程重新启动应用程序，将进程重用于另一个应用程序，
    /// 或者简单地终止进程和应用程序。
    ///
    /// 该函数可以在进程处于任何状态时调用。
    /// 它尝试重置所有进程状态并重新初始化它，以便可以重用它。
    ///
    /// 重新启动应用程序可能会因两个一般原因而失败：
    ///
    /// 1. 内核根据其策略选择不重新启动应用程序。
    ///
    /// 2. 内核决定重新启动应用程序但未能这样做，因为无法再为进程配置某些状态。
    ///    例如，进程的系统调用状态无法初始化。
    ///
    /// 在 `restart()` 运行之后，进程要么排队运行其应用程序的 `_start` 函数，
    /// 要么终止，要么排队运行不同应用程序的 `_start` 函数。
    ///
    /// 由于进程将在重新启动之前终止，因此此函数接受可选的 `completion_code`。
    /// 如果进程提供了完成代码（例如，通过退出系统调用），那么应该使用 `Some(u32)` 调用它。
    /// 如果内核试图重新启动进程并且进程没有提供完成代码，那么应该使用 `None` 调用它。
    fn try_restart(&self, completion_code: Option<u32>);

    // memop操作

    /// 更改程序中断的位置并重新分配覆盖程序内存的 MPU 区域。
    ///
    /// 如果进程不再处于活动状态，这将失败并出现错误。
    /// 一个不活动的进程在没有被重置的情况下不会再次运行，此时更改内存指针是无效的。
    fn brk(&self, new_break: *const u8) -> Result<*const u8, Error>;

    /// 改变程序中断的位置，重新分配覆盖程序内存的 MPU 区域，并返回之前的中断地址。
    ///
    /// 如果进程不再处于活动状态，这将失败并出现错误。
    /// 一个不活动的进程在没有被重置的情况下不会再次运行，此时更改内存指针是无效的。
    fn sbrk(&self, increment: isize) -> Result<*const u8, Error>;

    /// 在 TBF 标头中为此进程定义了多少可写闪存区域。
    fn number_writeable_flash_regions(&self) -> usize;

    /// 获取从flash开始的偏移量和定义的可写flash区域的大小。
    fn get_writeable_flash_region(&self, region_index: usize) -> (u32, u32);

    /// 调试函数，用于更新内核在此进程的堆栈开始位置。
    /// 进程不需要通过 memop 系统调用来调用它，但它有助于调试进程。
    fn update_stack_start_pointer(&self, stack_pointer: *const u8);

    /// 调试功能来更新进程堆开始的内核。也是可选的。
    fn update_heap_start_pointer(&self, heap_pointer: *const u8);

    /// 从进程内存中的给定偏移量和大小创建一个 [`ReadWriteProcessBuffer`]。
    ///
    /// ## Returns
    ///
    /// 如果成功，此方法返回创建的 [`ReadWriteProcessBuffer`]。
    ///
    /// 如果出现错误，则返回适当的 ErrorCode：
    ///
    /// - 如果内存不包含在进程可访问的内存空间`buf_start_addr`和`size`
    ///   不是有效的读写缓冲区（范围内的任何字节都不能被进程读/写访问），[`ErrorCode::INVAL`]。
    /// - If the process is not active: [`ErrorCode::FAIL`].
    /// - For all other errors: [`ErrorCode::FAIL`].
    fn build_readwrite_process_buffer(
        &self,
        buf_start_addr: *mut u8,
        size: usize,
    ) -> Result<ReadWriteProcessBuffer, ErrorCode>;

    /// 从进程内存中的给定偏移量和大小创建一个 [`ReadOnlyProcessBuffer`]。
    ///
    /// ## Returns
    ///
    /// In case of success, this method returns the created
    /// [`ReadOnlyProcessBuffer`].
    ///
    /// In case of an error, an appropriate ErrorCode is returned:
    ///
    /// - If the memory is not contained in the process-accessible memory space
    ///   / `buf_start_addr` and `size` are not a valid read-only buffer (any
    ///   byte in the range is not read-accessible to the process),
    ///   [`ErrorCode::INVAL`].
    /// - If the process is not active: [`ErrorCode::FAIL`].
    /// - For all other errors: [`ErrorCode::FAIL`].
    fn build_readonly_process_buffer(
        &self,
        buf_start_addr: *const u8,
        size: usize,
    ) -> Result<ReadOnlyProcessBuffer, ErrorCode>;

    /// 在 `addr` 的进程地址空间中将单个字节设置为 `value`。
    /// 如果 `addr` 在当前暴露给进程的 RAM 范围内（因此可由进程本身写入）并且值已设置，
    /// 则返回 true，否则返回 false。
    ///
    ///  ### safety
    ///
    /// 此函数验证要写入的字节是否在进程的可访问内存中。
    /// 但是，为了避免未定义的行为，调用者需要确保在调用此函数之前不存在对进程内存的其他引用。
    unsafe fn set_byte(&self, addr: *mut u8, value: u8) -> bool;

    /// 返回给定 `driver_num` 的此进程的权限。
    ///
    /// 返回的 `CommandPermissions` 将指示是否为单个命令号指定了任何权限。
    /// 如果设置了权限，它们将作为顺序命令号的 64 位 bitmask 返回。 偏移量表示要获得权限的 64 个命令编号的倍数。
    fn get_command_permissions(&self, driver_num: usize, offset: usize) -> CommandPermissions;

    // mpu

    /// 配置 MPU 以使用进程的分配区域。
    ///
    /// 当进程处于非活动状态（即进程不会再次运行）时，调用此函数无效。
    fn setup_mpu(&self);

    /// 为进程分配一个新的 MPU 区域，该区域至少为 `min_region_size` 字节，并且位于指定的未分配内存范围内。
    ///
    /// 当进程处于非活动状态（即进程不会再次运行）时，调用此函数无效。
    fn add_mpu_region(
        &self,
        unallocated_memory_start: *const u8,
        unallocated_memory_size: usize,
        min_region_size: usize,
    ) -> Option<mpu::Region>;

    /// 从先前使用 `add_mpu_region` 添加的进程中删除 MPU 区域。
    ///
    /// 当进程处于非活动状态（即进程不会再次运行）时，调用此函数无效。
    fn remove_mpu_region(&self, region: mpu::Region) -> Result<(), ErrorCode>;

    // grants

    /// 从Grant区域分配内存并将引用存储在正确的Grant指针索引中。
    ///
    /// 此函数必须检查执行分配不会导致内核内存中断低于 MPU 允许的进程可访问内存区域的顶部。
    /// 请注意，这可能与实际的 app_brk 不同，因为 MPU 对齐和大小限制可能导致 MPU 强制区域与 app_brk 不同。
    ///
    /// 这将返回 `None` 并在以下情况下失败:
    /// - 进程处于非活动状态
    /// - 没有足够的可用内存来进行分配
    /// - grant_num 无效
    /// - grant_num 已经分配了一个Grant
    fn allocate_grant(
        &self,
        grant_num: usize,
        driver_num: usize,
        size: usize,
        align: usize,
    ) -> Option<NonNull<u8>>;

    /// 检查是否已为此Process分配给定的Grant。
    ///
    /// 如果进程未处于活动状态，则返回“None”。 否则，如果已分配Grant，则返回 `true`，否则返回 `false`。
    fn grant_is_allocated(&self, grant_num: usize) -> Option<bool>;

    /// 从“size”字节长并与“align”字节对齐的Grant区域分配内存。
    /// 这用于创建未记录在Grant指针数组中的自定义Grant，但对于需要额外的特定于进程的动态分配内存的Capsule很有用。
    ///
    /// 如果成功，则返回带有标识符的 Some()，该标识符可与 `enter_custom_grant()`
    /// 一起使用以访问内存和指向必须用于初始化内存的内存的指针。
    fn allocate_custom_grant(
        &self,
        size: usize,
        align: usize,
    ) -> Option<(ProcessCustomGrantIdentifer, NonNull<u8>)>;

    /// 为此Process输入基于“grant_num”的Grant。
    ///
    /// 输入Grant意味着访问存储为Grant的对象的实际内存。
    ///
    /// 如果进程处于非活动状态且“grant_num”无效、未分配Grant或已输入Grant，这将返回“Err”。
    /// 如果这返回“Ok()”，则指针指向先前为此授予分配的内存。
    fn enter_grant(&self, grant_num: usize) -> Result<*mut u8, Error>;

    /// 输入基于“identifier”的自定义Grant
    ///
    /// 这将根据分配自定义Grant时返回的标识符检索指向先前分配的自定义Grant的指针。
    ///
    /// 如果自定义Grant不再可访问，或者进程处于非活动状态，这将返回错误。
    fn enter_custom_grant(&self, identifier: ProcessCustomGrantIdentifer)
        -> Result<*mut u8, Error>;

    /// 与`enter_grant()`相反。 用于表示不再输入Grant。
    ///
    /// 如果 `grant_num` 有效，则此函数不会失败。 如果 `grant_num` 无效，此函数将不执行任何操作。
    /// 如果进程处于非活动状态，则授权无效且未输入或未输入，此函数将不执行任何操作。
    fn leave_grant(&self, grant_num: usize);

    /// 如果进程处于活动状态，则返回分配的Grant指针数。 这不包括自定义Grant。
    /// 这用于确定在调用 `SyscallDriver::allocate_grant()` 后是否分配了新的Grant。
    ///
    /// 用于调试/检查系统。
    fn grant_allocated_count(&self) -> Option<usize>;

    /// 如果存在与给定驱动程序编号关联的Grant，则获取与给定驱动程序编号关联的Grant编号 (grant_num)。
    fn lookup_grant_from_driver_num(&self, driver_num: usize) -> Result<usize, Error>;

    // subscribe

    /// 验证 Upcall 函数指针是否在进程可访问内存中。
    ///
    /// 如果 upcall 函数指针对该进程有效，则返回 `true`，否则返回 `false`。
    fn is_valid_upcall_function_pointer(&self, upcall_fn: NonNull<()>) -> bool;

    // 特定于体系结构的进程的功能

    /// 设置进程在系统调用后再次开始执行时应该看到的返回值。
    ///
    /// 当进程处于非活动状态（即进程不会再次运行）时，调用此函数无效。
    ///
    /// 如果 UKB 实现无法正确设置返回值，这可能会失败。 这是如何发生的一个例子：
    ///
    /// 1. UKB 实现使用进程的堆栈在内核空间和用户空间之间传输值。
    /// 2. 进程调用 memop.brk 并将其可访问的内存区域减少到其当前堆栈之下。
    /// 3. UKB 实现不能再在栈上设置返回值，因为进程不再有权访问它的栈。
    ///
    /// 如果失败，进程将进入故障状态。
    fn set_syscall_return_value(&self, return_value: SyscallReturn);

    /// 设置进程恢复时要执行的功能。
    ///
    /// 当进程处于非活动状态（即进程不会再次运行）时，调用此函数无效。
    fn set_process_function(&self, callback: FunctionCall);

    /// 上下文切换到特定进程。
    ///
    /// 如果进程处于非活动状态且无法切换到，这将返回“None”。
    fn switch_to(&self) -> Option<syscall::ContextSwitchReason>;

    /// 返回与各种进程数据结构在内存中的位置相关的进程状态信息。
    fn get_addresses(&self) -> ProcessAddresses;

    /// 返回与各种进程数据结构在内存中的大小相关的进程状态信息。
    fn get_sizes(&self) -> ProcessSizes;

    /// 将存储的状态作为二进制 blob 写入 `out` 切片。 返回成功时写入 `out` 的字节数。
    ///
    /// 如果 `out` 太短而无法保存存储的状态二进制表示，则返回 `ErrorCode::SIZE`。
    /// 在内部错误时返回 `ErrorCode::FAIL`。
    fn get_stored_state(&self, out: &mut [u8]) -> Result<usize, ErrorCode>;

    /// 打印出进程的完整状态：它的内存映射、它的上下文和内存保护单元 (MPU) 的状态。
    fn print_full_process(&self, writer: &mut dyn Write);

    // debug

    /// Returns how many syscalls this app has called.
    fn debug_syscall_count(&self) -> usize;

    /// Returns how many upcalls for this process have been dropped.
    fn debug_dropped_upcall_count(&self) -> usize;

    /// Returns how many times this process has exceeded its timeslice.
    fn debug_timeslice_expiration_count(&self) -> usize;

    /// Increment the number of times the process has exceeded its timeslice.
    fn debug_timeslice_expired(&self);

    /// Increment the number of times the process called a syscall and record
    /// the last syscall that was called.
    fn debug_syscall_called(&self, last_syscall: Syscall);

    /// Return the last syscall the process called. Returns `None` if the
    /// process has not called any syscalls or the information is unknown.
    fn debug_syscall_last(&self) -> Option<Syscall>;
}

/// 从进程的Grant区域动态分配的自定义Grant的不透明标识符-Opaque identifier
///
/// 此类型允许 Process 为进程内存中的自定义Grant提供句柄，
/// “ProcessGrant”稍后可以使用该句柄访问自定义Grant内存。
///
/// 我们使用这种类型而不是直接指针，以便任何访问尝试都可以确保进程仍然存在并且有效，并且自定义Grant尚未被释放。
///
/// 此结构的字段是私有的，因此只有 Process 可以创建此标识符。
#[derive(Copy, Clone)]
pub struct ProcessCustomGrantIdentifer {
    pub(crate) offset: usize,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Error {
    /// 该进程已被删除，不再存在。 例如，内核可以停止一个进程并重新声明其资源。
    NoSuchApp,
    /// 进程没有足够的内存来完成请求的操作。
    OutOfMemory,
    /// 提供的内存地址不可访问或对进程无效。
    AddressOutOfBounds,
    /// 该进程处于非活动状态（可能处于故障或退出状态），因此尝试的操作无效。
    InactiveApp,
    /// 这可能表明内核中存在错误，并且内核中的某些状态不一致。
    KernelError,
    /// 表示已经借用了一些process data，例如 Grant。
    AlreadyInUse,
}

impl From<Error> for Result<(), ErrorCode> {
    fn from(err: Error) -> Result<(), ErrorCode> {
        match err {
            Error::OutOfMemory => Err(ErrorCode::NOMEM),
            Error::AddressOutOfBounds => Err(ErrorCode::INVAL),
            Error::NoSuchApp => Err(ErrorCode::INVAL),
            Error::InactiveApp => Err(ErrorCode::FAIL),
            Error::KernelError => Err(ErrorCode::FAIL),
            Error::AlreadyInUse => Err(ErrorCode::FAIL),
        }
    }
}

impl From<Error> for ErrorCode {
    fn from(err: Error) -> ErrorCode {
        match err {
            Error::OutOfMemory => ErrorCode::NOMEM,
            Error::AddressOutOfBounds => ErrorCode::INVAL,
            Error::NoSuchApp => ErrorCode::INVAL,
            Error::InactiveApp => ErrorCode::FAIL,
            Error::KernelError => ErrorCode::FAIL,
            Error::AlreadyInUse => ErrorCode::FAIL,
        }
    }
}

/// 进程可以处于的各种状态。
///
/// 如果”Process”的外部实现想要在外部实现中重用这些Process state，则会将其公开。
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum State {
    /// 进程期望正在运行的代码。
    /// 该进程当前可能没有被调度程序调度，但如果它被调度，则该进程有工作要做。
    Running,

    /// 进程停止执行并返回内核，因为它调用了 `yield` 系统调用。
    /// 这可能意味着它正在等待某个事件发生，但也可能意味着它已经完成并且不需要再次安排。
    Yielded,

    /// 该进程已停止，其先前的状态为正在运行。 如果内核在处于“Running”状态时强制停止进程，
    /// 则使用此选项。 此状态指示内核不要调度该进程，但如果稍后要恢复该进程，
    /// 则应将其放回运行状态，以便正确执行。
    StoppedRunning,

    /// 该过程已停止，并在它产生时停止。 如果需要恢复此过程，则应将其放回“Yield”状态。
    StoppedYielded,

    /// The process faulted and cannot be run.
    Faulted,

    /// 进程以“exit-terminate”系统调用退出，无法运行。
    Terminated,

    /// 该过程从未真正执行过。 这当然发生在板子首次启动并且内核尚未切换到任何进程时。
    /// 如果一个进程被终止并且它的所有状态都被重置，就好像它还没有被执行一样，它也可能发生。
    Unstarted,
}

/// `Process` 使用 `Cell<State>` 的包装器来防止内核工作跟踪和进程状态跟踪中的状态重复引起的错误。
pub(crate) struct ProcessStateCell<'a> {
    state: Cell<State>,
    kernel: &'a Kernel,
}

impl<'a> ProcessStateCell<'a> {
    pub(crate) fn new(kernel: &'a Kernel) -> Self {
        Self {
            state: Cell::new(State::Unstarted),
            kernel,
        }
    }

    pub(crate) fn get(&self) -> State {
        self.state.get()
    }

    pub(crate) fn update(&self, new_state: State) {
        let old_state = self.state.get();

        if old_state == State::Running && new_state != State::Running {
            self.kernel.decrement_work();
        } else if new_state == State::Running && old_state != State::Running {
            self.kernel.increment_work()
        }
        self.state.set(new_state);
    }
}

/// 当进程遇到故障时内核应该采取的行动。
///
/// 当进程执行期间发生异常时（一个常见的例子是进程试图访问其允许区域之外的内存），
/// 系统将陷回内核，并且内核必须在此时决定如何处理该进程。
///
/// 这些行动与决定采取何种行动的政策是分开的。
/// 一个单独的特定于过程的策略应该确定要采取的行动。
#[derive(Copy, Clone)]
pub enum FaultAction {
    /// 生成一个 `panic!()` 调用并使整个系统崩溃。 这对于调试应用程序很有用，因为错误在发生后会立即显示。
    Panic,

    /// 尝试清理并重新启动导致故障的进程。
    /// 这会将进程的内存重置为进程启动时的状态，并安排进程从其 init 函数再次运行。
    Restart,

    /// 通过不再安排它运行来停止该进程。
    Stop,
}

/// Tasks that can be enqueued for a process.
///
/// 这对于“Process”的外部实现是公开的。
#[derive(Copy, Clone)]
pub enum Task {
    /// 进程中要执行的函数指针。 通常这是来自capsule的upcall。
    FunctionCall(FunctionCall),
    /// 需要额外设置来配置内存访问的 IPC 操作。
    IPC((ProcessId, ipc::IPCUpcallType)),
}

/// 枚举以确定进程的函数调用是直接来自内核还是来自通过“Driver”实现订阅的upcall。
///
/// An example of a kernel function is the application entry point.
#[derive(Copy, Clone, Debug)]
pub enum FunctionCallSource {
    /// 对于直接来自内核的函数，例如 `init_fn`。
    Kernel,
    /// 对于来自Capsule的功能或“Driver”的任何实现。
    Driver(UpcallId),
}

/// 定义可以传递给进程的upcall的结构。
/// upcall 需要四个参数，它们是 `Driver` 和 upcall 特定的，所以它们在这里一般表示。
///
/// 这四个参数可能会作为前四个寄存器值传递，但这取决于架构。
///
/// `FunctionCall` 还标识了调度它的 upcall（如果有），以便在进程取消订阅此 upcall 时可以取消调度它。
#[derive(Copy, Clone, Debug)]
pub struct FunctionCall {
    pub source: FunctionCallSource,
    pub argument0: usize,
    pub argument1: usize,
    pub argument2: usize,
    pub argument3: usize,
    pub pc: usize,
}

/// 收集与进程elements的内存地址相关的进程状态信息。
pub struct ProcessAddresses {
    /// 进程区域在非易失性内存中的开始地址。
    pub flash_start: usize,
    /// 进程在非易失性存储器中可以访问的区域的开始地址。
    /// 这是在 TBF 标头和内核保留供自己使用的任何其他内存之后。
    pub flash_non_protected_start: usize,
    /// 在非易失性内存中为该进程分配的区域结束后紧接的地址。
    pub flash_end: usize,

    /// 进程在内存中分配区域的开始地址。
    pub sram_start: usize,
    /// 应用程序中断的地址。 这是进程可以访问的内存结束后的地址。
    pub sram_app_brk: usize,
    /// 任何已分配Grant的最低地址。 这是内核代表该进程用于其内部状态的区域的开始。
    pub sram_grant_start: usize,
    /// 紧接在内存中为此进程分配的区域结束后的地址。
    pub sram_end: usize,

    /// 进程堆的起始地址（如果已知）。 注意，管理这完全取决于进程，
    /// 内核依赖于进程显式地通知它这个地址。
    /// 因此，内核可能不知道起始地址，或者它的起始地址不正确。
    pub sram_heap_start: Option<usize>,
    /// 进程堆栈的顶部（或开始）地址（如果已知）。 请注意，管理堆栈完全取决于进程，
    /// 并且内核依赖于进程显式地通知它它从哪里开始堆栈。
    /// 因此，内核可能不知道起始地址，或者它的起始地址不正确。
    pub sram_stack_top: Option<usize>,
    /// 内核看到堆栈指针的最低地址。 请注意，堆栈完全由进程管理，
    /// 进程可能会故意从内核中隐藏这个地址。
    /// 此外，堆栈可能已到达较低地址，这只是进程调用系统调用时看到的最低地址。
    pub sram_stack_bottom: Option<usize>,
}

/// 与各种进程结构在内存中的大小相关的进程状态的集合。
pub struct ProcessSizes {
    /// 用于Grant指针表的字节数。
    pub grant_pointers: usize,
    /// 用于Pending的upcall队列的字节数。
    pub upcall_list: usize,
    /// 用于进程控制块的字节数（即“ProcessX”结构）。
    pub process_control_block: usize,
}
