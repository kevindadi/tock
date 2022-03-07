//! Tock 系统调用号定义和与架构无关的接口特征。

use core::convert::TryFrom;
use core::fmt::Write;

use crate::errorcode::ErrorCode;
use crate::process;

pub use crate::syscall_driver::{CommandReturn, SyscallDriver};

/// 辅助函数将 u64 拆分为更高和更低的 u32。
///
/// 用于在32位平台上编码64位宽的系统调用返回值。
#[inline]
fn u64_to_be_u32s(src: u64) -> (u32, u32) {
    let src_bytes = src.to_be_bytes();
    let src_msb = u32::from_be_bytes([src_bytes[0], src_bytes[1], src_bytes[2], src_bytes[3]]);
    let src_lsb = u32::from_be_bytes([src_bytes[4], src_bytes[5], src_bytes[6], src_bytes[7]]);

    (src_msb, src_lsb)
}

// ---------- 系统调用参数解码 ----------

/// 根据 Tock ABI 中指定的标识符枚举系统调用类。
///
/// 这些被编码为 8 位值，因为在某些架构上，值可以在指令本身中编码。
#[repr(u8)]
#[derive(Copy, Clone, Debug)]
pub enum SyscallClass {
    Yield = 0,
    Subscribe = 1,
    Command = 2,
    ReadWriteAllow = 3,
    ReadOnlyAllow = 4,
    Memop = 5,
    Exit = 6,
    UserspaceReadableAllow = 7,
}

/// 根据 Tock ABI 中指定的 Yield 标识符值枚举 yield 系统调用。
#[derive(Copy, Clone, Debug)]
pub enum YieldCall {
    NoWait = 0,
    Wait = 1,
}

// 只要没有解决方案 https://github.com/rust-lang/rfcs/issues/2783 被集成到标准库中就需要
impl TryFrom<u8> for SyscallClass {
    type Error = u8;

    fn try_from(syscall_class_id: u8) -> Result<SyscallClass, u8> {
        match syscall_class_id {
            0 => Ok(SyscallClass::Yield),
            1 => Ok(SyscallClass::Subscribe),
            2 => Ok(SyscallClass::Command),
            3 => Ok(SyscallClass::ReadWriteAllow),
            4 => Ok(SyscallClass::ReadOnlyAllow),
            5 => Ok(SyscallClass::Memop),
            6 => Ok(SyscallClass::Exit),
            7 => Ok(SyscallClass::UserspaceReadableAllow),
            i => Err(i),
        }
    }
}

/// TRD 104 中定义的解码系统调用。
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Syscall {
    /// 表示Yield 系统调用类的结构。 `which` 是产量标识符值，`address` 是无等待字段。
    Yield { which: usize, address: *mut u8 },

    /// 表示对Subscribe系统调用类的调用的结构。 `driver_number`是驱动标识符，`subdriver_number`是订阅标识符，
    /// `upcall_ptr`是upcall指针，`appdata`是应用数据。
    Subscribe {
        driver_number: usize,
        subdriver_number: usize,
        upcall_ptr: *mut (),
        appdata: usize,
    },

    /// 表示Command系统调用类调用的结构。
    /// `driver_number` 是驱动程序标识符，`subdriver_number` 是命令标识符。
    Command {
        driver_number: usize,
        subdriver_number: usize,
        arg0: usize,
        arg1: usize,
    },

    /// 表示调用 ReadWriteAllow 系统调用类的结构。 `driver_number` 是驱动标识符，
    /// `subdriver_number` 是缓冲区标识符，`allow_address` 是地址，`allow_size` 是大小。
    ReadWriteAllow {
        driver_number: usize,
        subdriver_number: usize,
        allow_address: *mut u8,
        allow_size: usize,
    },

    /// 表示调用 ReadWriteAllow 系统调用类的结构，但具有共享内核和应用程序访问权限。 `
    /// driver_number` 是驱动标识符，`subdriver_number` 是缓冲区标识符，
    /// `allow_address` 是地址，`allow_size` 是大小。
    UserspaceReadableAllow {
        driver_number: usize,
        subdriver_number: usize,
        allow_address: *mut u8,
        allow_size: usize,
    },

    /// 表示 ReadOnlyAllow 系统调用类的结构。 `driver_number` 是驱动标识符，
    /// `subdriver_number` 是缓冲区标识符，`allow_address` 是地址，`allow_size` 是大小。
    ReadOnlyAllow {
        driver_number: usize,
        subdriver_number: usize,
        allow_address: *const u8,
        allow_size: usize,
    },

    /// 表示 Memop 系统调用类的调用的结构。 `operand` 是操作，`arg0` 是操作参数。
    Memop { operand: usize, arg0: usize },

    /// 表示调用 Exit 系统调用类的结构。 `which` 是退出标识符，而 `completion_code` 是传递给内核的完成代码。
    Exit {
        which: usize,
        completion_code: usize,
    },
}

impl Syscall {
    /// 用于将从应用程序传回的原始值转换为 Tock 中的 `Syscall` 类型的辅助函数，
    /// 表示系统调用调用的类型化版本。 如果值没有指定有效的系统调用，该方法返回 None。
    ///
    /// 不同的架构对于进程和内核交换数据有不同的ABI。
    /// 用于 CortexM 和 RISCV 微控制器的 32 位 ABI 在 TRD104 中指定。
    pub fn from_register_arguments(
        syscall_number: u8,
        r0: usize,
        r1: usize,
        r2: usize,
        r3: usize,
    ) -> Option<Syscall> {
        match SyscallClass::try_from(syscall_number) {
            Ok(SyscallClass::Yield) => Some(Syscall::Yield {
                which: r0,
                address: r1 as *mut u8,
            }),
            Ok(SyscallClass::Subscribe) => Some(Syscall::Subscribe {
                driver_number: r0,
                subdriver_number: r1,
                upcall_ptr: r2 as *mut (),
                appdata: r3,
            }),
            Ok(SyscallClass::Command) => Some(Syscall::Command {
                driver_number: r0,
                subdriver_number: r1,
                arg0: r2,
                arg1: r3,
            }),
            Ok(SyscallClass::ReadWriteAllow) => Some(Syscall::ReadWriteAllow {
                driver_number: r0,
                subdriver_number: r1,
                allow_address: r2 as *mut u8,
                allow_size: r3,
            }),
            Ok(SyscallClass::UserspaceReadableAllow) => Some(Syscall::UserspaceReadableAllow {
                driver_number: r0,
                subdriver_number: r1,
                allow_address: r2 as *mut u8,
                allow_size: r3,
            }),
            Ok(SyscallClass::ReadOnlyAllow) => Some(Syscall::ReadOnlyAllow {
                driver_number: r0,
                subdriver_number: r1,
                allow_address: r2 as *const u8,
                allow_size: r3,
            }),
            Ok(SyscallClass::Memop) => Some(Syscall::Memop {
                operand: r0,
                arg0: r1,
            }),
            Ok(SyscallClass::Exit) => Some(Syscall::Exit {
                which: r0,
                completion_code: r1,
            }),
            Err(_) => None,
        }
    }
}

// ---------- 系统调用返回值编码 ----------

/// TRD104 中描述的系统调用返回类型变体标识符的枚举。
///
/// 每个变体都与相应的变体标识符相关联，该标识符将与返回值一起传递给用户空间。
#[repr(u32)]
#[derive(Copy, Clone, Debug)]
pub enum SyscallReturnVariant {
    Failure = 0,
    FailureU32 = 1,
    FailureU32U32 = 2,
    FailureU64 = 3,
    Success = 128,
    SuccessU32 = 129,
    SuccessU32U32 = 130,
    SuccessU64 = 131,
    SuccessU32U32U32 = 132,
    SuccessU64U32 = 133,
}

/// TRD104 中指定的可能的系统调用返回变量的枚举。
///
/// 此结构对原始类型进行操作，例如固定长度的整数和指针。 它由调度程序构建并传递给架构以编码到寄存器中，
/// 使用提供的 [`encode_syscall_return`](SyscallReturn::encode_syscall_return) 方法。
///
/// Capsule不使用这个结构。 Capsules 使用更高级别的 Rust 类型
/// （例如 [`ReadWriteProcessBuffer`](crate::processbuffer::ReadWriteProcessBuffer) 和 `GrantKernelData`）
/// 或围绕此结构的包装器（[`CommandReturn`](crate::syscall_driver::CommandReturn)）安全可构造变体的可用构造函数。
#[derive(Copy, Clone, Debug)]
pub enum SyscallReturn {
    /// Generic error case
    Failure(ErrorCode),
    /// Generic error case, with an additional 32-bit data field
    FailureU32(ErrorCode, u32),
    /// Generic error case, with two additional 32-bit data fields
    FailureU32U32(ErrorCode, u32, u32),
    /// Generic error case, with an additional 64-bit data field
    FailureU64(ErrorCode, u64),
    /// Generic success case
    Success,
    /// Generic success case, with an additional 32-bit data field
    SuccessU32(u32),
    /// Generic success case, with two additional 32-bit data fields
    SuccessU32U32(u32, u32),
    /// Generic success case, with three additional 32-bit data fields
    SuccessU32U32U32(u32, u32, u32),
    /// Generic success case, with an additional 64-bit data field
    SuccessU64(u64),
    /// Generic success case, with an additional 32-bit and 64-bit
    /// data field
    SuccessU64U32(u64, u32),

    // 调度程序使用以下这些类型，以便它可以以独立于架构（指针宽度）的方式将值返回给用户空间。
    // 内核传递这些类型（而不是 ProcessBuffer 或 Upcall）有两个原因。 首先，由于内核/调度器对这些类型的生命周期
    // 和安全性做出了承诺，它不想将它们泄露给其他代码。 其次，如果订阅或允许调用传递无效值（指针超出有效内存），
    // 内核无法构造ProcessBuffer或Upcall类型但需要能够返回失败。
    // 读/写允许成功案例，将先前允许的缓冲区和大小返回给进程。
    AllowReadWriteSuccess(*mut u8, usize),
    /// Read/Write allow failure case, returns the passed allowed
    /// buffer and size to the process.
    AllowReadWriteFailure(ErrorCode, *mut u8, usize),

    /// Shared Read/Write allow success case, returns the previous allowed
    /// buffer and size to the process.
    UserspaceReadableAllowSuccess(*mut u8, usize),
    /// Shared Read/Write allow failure case, returns the passed allowed
    /// buffer and size to the process.
    UserspaceReadableAllowFailure(ErrorCode, *mut u8, usize),

    /// Read only allow success case, returns the previous allowed
    /// buffer and size to the process.
    AllowReadOnlySuccess(*const u8, usize),
    /// Read only allow failure case, returns the passed allowed
    /// buffer and size to the process.
    AllowReadOnlyFailure(ErrorCode, *const u8, usize),

    /// Subscribe success case, returns the previous upcall function
    /// pointer and application data.
    SubscribeSuccess(*const (), usize),
    /// Subscribe failure case, returns the passed upcall function
    /// pointer and application data.
    SubscribeFailure(ErrorCode, *const (), usize),
}

impl SyscallReturn {
    /// 将一个 CommandReturn（它是 SyscallReturn 的一个子集的包装器）转换为一个 SyscallReturn。
    ///
    /// 这允许 CommandReturn 仅包含可以从 Command 返回的 SyscallReturn 的变体，
    /// 同时以一种廉价的方式将其作为 SyscallReturn 来处理，以用于更通用的代码路径。
    pub(crate) fn from_command_return(res: CommandReturn) -> Self {
        res.into_inner()
    }

    /// Returns true if the `SyscallReturn` is any success type.
    pub(crate) fn is_success(&self) -> bool {
        match self {
            SyscallReturn::Success => true,
            SyscallReturn::SuccessU32(_) => true,
            SyscallReturn::SuccessU32U32(_, _) => true,
            SyscallReturn::SuccessU32U32U32(_, _, _) => true,
            SyscallReturn::SuccessU64(_) => true,
            SyscallReturn::SuccessU64U32(_, _) => true,
            SyscallReturn::AllowReadWriteSuccess(_, _) => true,
            SyscallReturn::UserspaceReadableAllowSuccess(_, _) => true,
            SyscallReturn::AllowReadOnlySuccess(_, _) => true,
            SyscallReturn::SubscribeSuccess(_, _) => true,
            SyscallReturn::Failure(_) => false,
            SyscallReturn::FailureU32(_, _) => false,
            SyscallReturn::FailureU32U32(_, _, _) => false,
            SyscallReturn::FailureU64(_, _) => false,
            SyscallReturn::AllowReadWriteFailure(_, _, _) => false,
            SyscallReturn::UserspaceReadableAllowFailure(_, _, _) => false,
            SyscallReturn::AllowReadOnlyFailure(_, _, _) => false,
            SyscallReturn::SubscribeFailure(_, _, _) => false,
        }
    }

    /// Encode the system call return value into 4 registers, following
    /// the encoding specified in TRD104. Architectures which do not follow
    /// TRD104 are free to define their own encoding.
    pub fn encode_syscall_return(&self, a0: &mut u32, a1: &mut u32, a2: &mut u32, a3: &mut u32) {
        match self {
            &SyscallReturn::Failure(e) => {
                *a0 = SyscallReturnVariant::Failure as u32;
                *a1 = usize::from(e) as u32;
            }
            &SyscallReturn::FailureU32(e, data0) => {
                *a0 = SyscallReturnVariant::FailureU32 as u32;
                *a1 = usize::from(e) as u32;
                *a2 = data0;
            }
            &SyscallReturn::FailureU32U32(e, data0, data1) => {
                *a0 = SyscallReturnVariant::FailureU32U32 as u32;
                *a1 = usize::from(e) as u32;
                *a2 = data0;
                *a3 = data1;
            }
            &SyscallReturn::FailureU64(e, data0) => {
                let (data0_msb, data0_lsb) = u64_to_be_u32s(data0);
                *a0 = SyscallReturnVariant::FailureU64 as u32;
                *a1 = usize::from(e) as u32;
                *a2 = data0_lsb;
                *a3 = data0_msb;
            }
            &SyscallReturn::Success => {
                *a0 = SyscallReturnVariant::Success as u32;
            }
            &SyscallReturn::SuccessU32(data0) => {
                *a0 = SyscallReturnVariant::SuccessU32 as u32;
                *a1 = data0;
            }
            &SyscallReturn::SuccessU32U32(data0, data1) => {
                *a0 = SyscallReturnVariant::SuccessU32U32 as u32;
                *a1 = data0;
                *a2 = data1;
            }
            &SyscallReturn::SuccessU32U32U32(data0, data1, data2) => {
                *a0 = SyscallReturnVariant::SuccessU32U32U32 as u32;
                *a1 = data0;
                *a2 = data1;
                *a3 = data2;
            }
            &SyscallReturn::SuccessU64(data0) => {
                let (data0_msb, data0_lsb) = u64_to_be_u32s(data0);

                *a0 = SyscallReturnVariant::SuccessU64 as u32;
                *a1 = data0_lsb;
                *a2 = data0_msb;
            }
            &SyscallReturn::SuccessU64U32(data0, data1) => {
                let (data0_msb, data0_lsb) = u64_to_be_u32s(data0);

                *a0 = SyscallReturnVariant::SuccessU64U32 as u32;
                *a1 = data0_lsb;
                *a2 = data0_msb;
                *a3 = data1;
            }
            &SyscallReturn::AllowReadWriteSuccess(ptr, len) => {
                *a0 = SyscallReturnVariant::SuccessU32U32 as u32;
                *a1 = ptr as u32;
                *a2 = len as u32;
            }
            &SyscallReturn::UserspaceReadableAllowSuccess(ptr, len) => {
                *a0 = SyscallReturnVariant::SuccessU32U32 as u32;
                *a1 = ptr as u32;
                *a2 = len as u32;
            }
            &SyscallReturn::AllowReadWriteFailure(err, ptr, len) => {
                *a0 = SyscallReturnVariant::FailureU32U32 as u32;
                *a1 = usize::from(err) as u32;
                *a2 = ptr as u32;
                *a3 = len as u32;
            }
            &SyscallReturn::UserspaceReadableAllowFailure(err, ptr, len) => {
                *a0 = SyscallReturnVariant::FailureU32U32 as u32;
                *a1 = usize::from(err) as u32;
                *a2 = ptr as u32;
                *a3 = len as u32;
            }
            &SyscallReturn::AllowReadOnlySuccess(ptr, len) => {
                *a0 = SyscallReturnVariant::SuccessU32U32 as u32;
                *a1 = ptr as u32;
                *a2 = len as u32;
            }
            &SyscallReturn::AllowReadOnlyFailure(err, ptr, len) => {
                *a0 = SyscallReturnVariant::FailureU32U32 as u32;
                *a1 = usize::from(err) as u32;
                *a2 = ptr as u32;
                *a3 = len as u32;
            }
            &SyscallReturn::SubscribeSuccess(ptr, data) => {
                *a0 = SyscallReturnVariant::SuccessU32U32 as u32;
                *a1 = ptr as u32;
                *a2 = data as u32;
            }
            &SyscallReturn::SubscribeFailure(err, ptr, data) => {
                *a0 = SyscallReturnVariant::FailureU32U32 as u32;
                *a1 = usize::from(err) as u32;
                *a2 = ptr as u32;
                *a3 = data as u32;
            }
        }
    }
}

// ---------- 用户空间内核边界 ----------

/// `ContentSwitchReason` 指定进程停止执行和执行返回内核的原因。
#[derive(PartialEq, Copy, Clone)]
pub enum ContextSwitchReason {
    /// Process called a syscall. Also returns the syscall and relevant values.
    SyscallFired { syscall: Syscall },
    /// 进程触发了硬故障处理程序。
    /// 在“平台”可以处理故障并允许应用程序继续运行的情况下，实现仍应保存寄存器。
    /// 有关这方面的更多详细信息，请参阅 `Platform::process_fault_hook()`。
    Fault,
    /// Process interrupted (e.g. by a hardware event)
    Interrupted,
}

/// `UserspaceKernelBoundary` trait 由 Tock 芯片实现的架构组件实现。
/// 此特性允许内核以独立于体系结构的方式在进程之间切换。
///
/// 究竟如何在内核空间和用户空间之间传递调用和返回值是特定于架构的。
/// 该架构可以在切换时使用进程内存来存储状态。 因此，此 trait 中的函数被传递给进程可访问内存的边界，
/// 以便体系结构实现可以验证它正在读取和写入进程可以有效访问的内存。
/// 这些边界通过 `accessible_memory_start` 和 `app_brk` 指针传递。
pub trait UserspaceKernelBoundary {
    /// 一些特定于体系结构的结构，包含在进程未运行时必须保留的每个进程状态。
    /// 例如，用于保留未存储在堆栈中的 CPU 寄存器。
    ///
    /// 实现不应该**依赖 `Default` 构造函数（自定义或派生）来初始化进程的存储状态。
    /// 初始化必须在 `initialize_process()` 函数中进行。
    type StoredState: Default;

    /// 在进程创建期间由内核调用，以通知内核新进程所需的进程可访问 RAM 的最小数量。
    /// 这允许特定于体系结构的进程布局决策，例如堆栈指针初始化。
    ///
    /// 这将返回内核必须分配给进程的进程可访问内存的最小字节数，以便成功的上下文切换成为可能。
    ///
    /// 一些架构可能不需要任何分配的内存，这应该返回 0。
    /// 一般来说，实现应该尝试预先分配最少数量的进程可访问内存（即返回尽可能接近 0）以提供最多过程的灵活性。
    /// 但是，对于在系统调用期间在内核空间和用户空间之间的内存中传递值或需要设置堆栈的架构，返回值将是非零的。
    fn initial_process_app_brk_size(&self) -> usize;

    /// 由内核在分配内存后但在允许开始执行之前调用。允许特定于架构的流程设置，例如分配系统调用堆栈帧。
    ///
    /// 此函数还必须初始化存储的状态（如果需要）。
    ///
    /// 内核通过提供 `accessible_memory_start` 以分配给进程的内存开始调用此函数。它还提供了 `app_brk` 指针，
    /// 它标志着进程可访问内存的结束。内核保证 `accessible_memory_start` 将是字对齐的。
    ///
    /// 如果成功，此函数返回 `Ok()`。如果进程系统调用状态不能用可用内存量初始化，或者由于任何其他原因，它应该返回 `Err()`。
    ///
    /// 这个函数可以在同一个进程中被多次调用。例如，如果一个进程崩溃并要重新启动，则必须调用它。
    /// 或者，如果进程被移动，则可能需要调用它。
    ///
    /// ## safety
    ///
    /// 该函数保证如果需要更改进程内存，它只会更改从 `accessible_memory_start` 开始和 `app_brk` 之前的内存。
    /// 调用者负责保证这些指针对进程有效。
    unsafe fn initialize_process(
        &self,
        accessible_memory_start: *const u8,
        app_brk: *const u8,
        state: &mut Self::StoredState,
    ) -> Result<(), ()>;

    /// 设置进程在系统调用后再次开始执行时应该看到的返回值。 这只会在进程调用系统调用后调用。
    ///
    /// 设置返回值的过程由 `state` 值指定。 `return_value` 是应该传递给进程的值，
    /// 以便当它恢复执行时它知道它调用的系统调用的返回值。
    ///
    /// ## safety
    ///
    /// 该函数保证如果需要更改进程内存，它只会更改从 `accessible_memory_start` 开始和 `app_brk` 之前的内存。
    /// 调用者负责保证这些指针对进程有效。
    unsafe fn set_syscall_return_value(
        &self,
        accessible_memory_start: *const u8,
        app_brk: *const u8,
        state: &mut Self::StoredState,
        return_value: SyscallReturn,
    ) -> Result<(), ()>;

    /// 设置进程恢复时应执行的功能。
    /// 这有两个主要用途：
    /// 1）在进程第一次启动时设置对`_start`的初始函数调用;
    /// 2) 告诉进程在调用 `yield()` 后执行upcall函数.
    ///
    /// **注意：** 此方法不能与 `set_syscall_return_value` 一起调用，因为注入的函数会破坏返回值。
    ///
    /// ### Arguments
    ///
    /// - `accessible_memory_start` 是该进程的进程可访问内存区域的起始地址。
    /// - `app_brk` 是当前进程中断的地址。 这标志着进程可以访问的内存区域的结束。 注意，这并不是分配给进程的整个内存区域的结束。
    ///   此地址之上的一些内存仍分配给进程，但如果进程试图访问它，则会发生 MPU 故障。
    /// - `state` 是该进程的存储状态。
    /// - `upcall` 是进程恢复时应该执行的函数。
    ///
    /// ### Return
    ///
    /// 如果函数已成功加入进程队列，则返回 `Ok(())`。
    /// 如果函数不是，则返回 `Err(())`，可能是因为没有足够的可用内存来执行此操作。
    ///
    /// ### Safety
    ///
    /// 该函数保证如果需要更改进程内存，
    /// 它只会更改从 `accessible_memory_start` 和 `app_brk` 开始的内存。 调用者负责保证这些指针对进程有效。
    unsafe fn set_process_function(
        &self,
        accessible_memory_start: *const u8,
        app_brk: *const u8,
        state: &mut Self::StoredState,
        upcall: process::FunctionCall,
    ) -> Result<(), ()>;

    /// 上下文切换到特定进程。
    ///
    /// 这将返回一个元组中的两个值。
    ///
    /// 1. 一个 `ContextSwitchReason` 指示进程停止执行并切换回内核的原因。
    /// 2. 进程使用的当前堆栈指针。 这是可选的，因为它仅用于在 process.rs 中进行调试。
    /// 通过与 process.rs 共享进程的堆栈指针，用户可以检查状态并查看堆栈深度，这可能对调试有用。
    ///
    /// ### Safety
    ///
    /// 该函数保证如果需要更改进程内存，它只会更改从 `accessible_memory_start` 和 `app_brk` 开始的内存。
    /// 调用者负责保证这些指针对进程有效。
    unsafe fn switch_to_process(
        &self,
        accessible_memory_start: *const u8,
        app_brk: *const u8,
        state: &mut Self::StoredState,
    ) -> (ContextSwitchReason, Option<*const u8>);

    /// 显示由该进程的存储状态标识的进程的体系结构特定（例如 CPU 寄存器或状态标志）数据。
    ///
    /// ### Safety
    ///
    /// 该函数保证如果需要更改进程内存，它只会更改从 `accessible_memory_start` 和 `app_brk` 开始的内存。
    /// 调用者负责保证这些指针对进程有效。
    unsafe fn print_context(
        &self,
        accessible_memory_start: *const u8,
        app_brk: *const u8,
        state: &Self::StoredState,
        writer: &mut dyn Write,
    );

    /// 存储进程的特定架构（例如 CPU 寄存器或状态标志）数据。 成功时返回写入输出的元素数。
    fn store_context(&self, state: &Self::StoredState, out: &mut [u8]) -> Result<usize, ErrorCode>;
}
