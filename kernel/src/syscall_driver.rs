//! 用户空间进程的系统调用接口。
//!
//! 驱动程序实现这些接口以将操作公开给进程。
//!
//! # 系统调用概述
//!
//! Tock 支持六个系统调用。
//! `allow_readonly`、`allow_readwrite`、`subscribe`、`yield` 和 `memop` 系统调用由核心内核处理，
//! 而 `command` 由驱动程序实现。
//! 主要系统调用：
//!
//!   * `subscribe` 将upcall传递给驱动程序，
//!     当事件发生或感兴趣的数据可用时，它可以稍后在进程upcall该驱动程序。
//!
//!   * `command` 告诉驱动程序立即做某事。
//!
//!   * `allow_readwrite` 为驱动程序提供对应用程序缓冲区的读写访问。
//!
//!   * `allow_userspace_readable` 为驱动程序提供对仍与应用程序共享的应用程序缓冲区的读写访问权限。
//!
//!   * `allow_readonly` 为驱动程序提供对应用程序缓冲区的只读访问。
//!
//! ## 将系统调用映射到驱动程序
//!
//! 这三个系统调用中的每一个都至少需要两个参数。 第一个是_驱动程序标识符_，
//! 它告诉调度程序将系统调用转发到哪个驱动程序。 第二个参数是__syscall number_，
//! 驱动程序使用它来区分具有不同驱动程序特定含义的调用实例（例如，“subscribe”表示“data received”
//! 与“subscribe”表示“send completed”）。 _driver identifiers_和驱动程序之间的映射由特定平台确定，
//! 而_syscall number_是特定于驱动程序的。
//!
//! Tock 中的一个约定是，`command` 系统调用的_driver minor number_0 始终可用于通过检查返回代码
//! 来确定正在运行的内核是否支持驱动程序。 如果返回值大于或等于零，则驱动程序存在。
//! 通常这是由只返回 0 的空命令实现的，但在某些情况下，该命令还可以返回更多信息，例如支持的设备数量。如LED
//!
//! # The `yield` system call class
//!
//! 虽然驱动程序不处理 `yield` 系统调用，但了解它们以及它们如何与 `subscribe` 交互很重要，
//! 后者向内核注册了upcall函数。 当进程调用 `yield` 系统调用时，内核会检查该进程是否有任何挂起
//! 的upcall。 如果有挂起的 upcall，它会将一个 upcall 压入进程堆栈。 如果没有挂起的 upcall，
//! `yield-wait` 将导致进程休眠，直到触发 upcall，而 `yield-no-wait` 立即返回。
//!
//! # Method result types
//!
//! 每个驱动程序方法都有一组有限的有效返回类型。 每个方法都有一个对应于成功的返回类型和一个对应于失败的返回类型。
//! 对于 `subscribe` 和 `allow` 系统调用，这些调用的每个实例的这些返回类型都是相同的。
//! 然而，“command”系统调用的每个实例都有自己指定的返回类型。 例如，请求时间戳的命令可能会在成功时返回 32 位数字，
//! 在失败时会返回错误代码，而请求以微秒为单位的时间的命令可能会返回 64 位数字和 32 位时区成功时编码，失败时错误代码。
//!
//! 这些结果类型表示为安全的 Rust 类型。 核心内核（调度程序和系统调用调度程序）负责将这些类型编码到 Tock 系统调用 ABI 规范中。

use core::convert::TryFrom;

use crate::errorcode::ErrorCode;
use crate::process;
use crate::process::ProcessId;
use crate::processbuffer::UserspaceReadableProcessBuffer;
use crate::syscall::SyscallReturn;

/// `command` 驱动方法的可能返回值，如 TRD104 中所指定。
///
/// 这只是 [`SyscallReturn`](SyscallReturn) 的包装，因为 `command` diver方法可能只返回原始整数类型作为。
///
/// 重要的是，此包装器只能在 [`SyscallReturn`](SyscallReturn) 的变体上构造，
/// 这些变体被认为对于capsule构造和返回应用程序是安全的
/// （例如，不是 [`SubscribeSuccess`](crate:: syscall::SyscallReturn::SubscribeSuccess))。
/// 这意味着内部值**必须**保持私有。
pub struct CommandReturn(SyscallReturn);
impl CommandReturn {
    pub(crate) fn into_inner(self) -> SyscallReturn {
        self.0
    }

    /// Command error
    pub fn failure(rc: ErrorCode) -> Self {
        CommandReturn(SyscallReturn::Failure(rc))
    }

    /// Command error with an additional 32-bit data field
    pub fn failure_u32(rc: ErrorCode, data0: u32) -> Self {
        CommandReturn(SyscallReturn::FailureU32(rc, data0))
    }

    /// Command error with two additional 32-bit data fields
    pub fn failure_u32_u32(rc: ErrorCode, data0: u32, data1: u32) -> Self {
        CommandReturn(SyscallReturn::FailureU32U32(rc, data0, data1))
    }

    /// Command error with an additional 64-bit data field
    pub fn failure_u64(rc: ErrorCode, data0: u64) -> Self {
        CommandReturn(SyscallReturn::FailureU64(rc, data0))
    }

    /// Successful command
    pub fn success() -> Self {
        CommandReturn(SyscallReturn::Success)
    }

    /// Successful command with an additional 32-bit data field
    pub fn success_u32(data0: u32) -> Self {
        CommandReturn(SyscallReturn::SuccessU32(data0))
    }

    /// Successful command with two additional 32-bit data fields
    pub fn success_u32_u32(data0: u32, data1: u32) -> Self {
        CommandReturn(SyscallReturn::SuccessU32U32(data0, data1))
    }

    /// Successful command with three additional 32-bit data fields
    pub fn success_u32_u32_u32(data0: u32, data1: u32, data2: u32) -> Self {
        CommandReturn(SyscallReturn::SuccessU32U32U32(data0, data1, data2))
    }

    /// Successful command with an additional 64-bit data field
    pub fn success_u64(data0: u64) -> Self {
        CommandReturn(SyscallReturn::SuccessU64(data0))
    }

    /// Successful command with an additional 64-bit and 32-bit data field
    pub fn success_u64_u32(data0: u64, data1: u32) -> Self {
        CommandReturn(SyscallReturn::SuccessU64U32(data0, data1))
    }
}

impl From<Result<(), ErrorCode>> for CommandReturn {
    fn from(rc: Result<(), ErrorCode>) -> Self {
        match rc {
            Ok(()) => CommandReturn::success(),
            _ => CommandReturn::failure(ErrorCode::try_from(rc).unwrap()),
        }
    }
}

impl From<process::Error> for CommandReturn {
    fn from(perr: process::Error) -> Self {
        CommandReturn::failure(perr.into())
    }
}

/// 实现 TRD104 中指定的peripheral diver系统调用的capsule的Trait。
/// 内核将从用户空间传递的值转换为 Rust 类型，并包括哪个进程正在进行调用。
/// 所有这些系统调用只执行很少的同步工作； 长时间运行的计算或 I/O 应该是分阶段的，并带有指示其完成的调用。
///
/// 这些方法中的每一个的确切实例（哪些标识符是有效的以及它们代表什么）特定于peripheral system call diver。
///
/// 关于订阅的注意事项：upcall订阅完全由核心内核处理，因此capsule没有订阅功能可以实现。
#[allow(unused_variables)]
pub trait SyscallDriver {
    /// 对进程执行短同步操作或启动长时间运行的分阶段操作的系统调用（其完成由upcall发出信号）。
    /// 命令 0 是一个保留命令，用于检测是否安装了peripheral system call diver，并且必须始终返回 CommandReturn::Success。
    fn command(
        &self,
        command_num: usize,
        r2: usize,
        r3: usize,
        process_id: ProcessId,
    ) -> CommandReturn {
        CommandReturn::failure(ErrorCode::NOSUPPORT)
    }

    /// 进程的系统调用将缓冲区（UserspaceReadableProcessBuffer）传递给内核可以读取或写入的内核。
    /// 内核仅在检查整个缓冲区是否在进程可以读写的内存中后才调用此方法。
    ///
    /// 这与 `allow_readwrite()` 的不同之处在于，一旦缓冲区被传递给内核，应用程序就可以读取缓冲区。
    /// 有关如何安全完成此操作的更多详细信息，请参阅用户空间可读允许系统调用 TRDXXX。
    fn allow_userspace_readable(
        &self,
        app: ProcessId,
        which: usize,
        slice: UserspaceReadableProcessBuffer,
    ) -> Result<UserspaceReadableProcessBuffer, (UserspaceReadableProcessBuffer, ErrorCode)> {
        Err((slice, ErrorCode::NOSUPPORT))
    }

    /// 请求为Process分配capsule的Grant。
    ///
    /// 核心内核使用这个函数来指示一个capsule确保它的Grant（如果有的话）被分配给一个特定的进程。
    /// 核心内核需要capsule来启动分配，因为只有capsule知道将存储在Grant中的类型 T（以及 T 的大小）。
    ///
    /// The typical implementation will look like:
    /// ```rust, ignore
    /// fn allocate_grant(&self, processid: ProcessId) -> Result<(), kernel::process::Error> {
    ///    self.apps.enter(processid, |_, _| {})
    /// }
    /// ```
    ///
    /// 没有提供默认实现来帮助防止意外忘记实现此功能。
    ///
    /// 如果一个capsule未能成功实现这个功能，那么从用户空间订阅Driver的调用可能会失败。
    //
    // 包含此功能源于从 Tock 2.0 开始在内核中确保正确上调用交换语义的方法。为了确保Upcall始终正确交换，
    // 所有Upcall处理都在核心内核中完成。 Capsules 只能访问允许他们安排 upcalls 的句柄，但 capsules 不再管理 upcalls。
    //
    // 核心内核将Upcall与Capsule的Grant对象一起存储在进程的Grant区域中。
    // 同时 Tock 2.0 更改要求希望使用 upcall 的Capsule也必须使用Grant。在Grant中存储upcall需要在该过程中为该Capsule分配Grant。
    // 这提出了一个挑战，因为Grant仅在进程实际使用时才动态分配。如果订阅系统调用首先发生，在Capsule分配Grant之前，
    // 内核无法存储upcall。内核无法自行分配Grant，因为它不知道capsule将用于Grant的类型 T
    // （或者更具体地说，内核不知道用于内存分配的 T 的大小）。
    //
    // 关于如何处理这种情况，内核必须在Capsule分配Grant前存储upcall
    //
    // 1. 内核可以为Grant类型 T 分配空间，但不实际初始化它，仅基于 T 的大小。
    //    但是，这需要内核跟踪每个Grant的 T 大小，并且没有方便的地方存储该信息。
    //
    // 2. 内核可以在Grant区域中分别存储upcall和Grant类型。
    //
    //    a. 一种方法是完全动态地存储Upcall。 也就是说，每当新的 subscribe_num 用于特定驱动程序时，
    //       核心内核都会从Grant区域分配新内存来存储它。这会起作用，但管理所有动态调用存储会产生很高的内存和运行时开销。
    //    b. 为了减少跟踪开销，可以将特定驱动程序的所有upcall存储在一起作为一个分配。
    //       每次grant只需要一个额外的指针来指向 upcall 数组。 但是，内核不知道特定驱动程序需要多少次调用，并且没有方便的地方来存储该信息。
    //
    // 3. 内核可以为Grant区域中所有驱动程序的所有调用分配一个固定区域。 当每个grant被创建时，
    //    它可以告诉内核它将使用多少次调用，并且内核可以轻松地跟踪总数。 然后，当一个进程的内存被分配时，
    //    内核会为这么多的调用保留空间。 然而，有两个问题。 内核不知道每个驱动程序单独需要多少次调用，
    //    因此它无法正确索引到该数组以存储每个调用。 其次，调用数组内存将被静态分配，并且会浪费在进程从不使用的驱动程序上。
    //
    //    这种方法的一个版本会假设每个驱动程序有一定数量的向上调用的最大限制。 这将解决索引挑战，
    //    但仍然存在内存开销问题。 它还会通过限制任何capsule可以使用的upcall的数量来限制capusle的灵活性。
    //
    // 4. 内核可以有一些机制来要求一个capsule分配它的grant，并且由于capsule知道 T 的大小
    //    和它使用grant类型的upcall的数量，并且upcall存储可以一起分配。
    //
    // 基于可用的选项，Tock 开发人员决定使用选项 4，并将 `allocate_grant` 方法添加到 `Driver` 特征。
    // 如果内核需要在每个驱动程序的基础上存储额外的状态并因此需要一种机制来强制grant分配，那么这种机制可能会在未来找到更多用途。
    //
    // 这个相同的机制后来也被扩展为处理允许调用。 不需要upcall但使用进程缓冲区的capsule也必须实现此功能。
    fn allocate_grant(&self, process_id: ProcessId) -> Result<(), crate::process::Error>;
}
