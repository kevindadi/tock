//! Core Tock Kernel
//!
//! 内核 crate 实现了 Tock 的核心功能以及许多Chip、capsule和board使用的共享代码。
//! 它还包含硬件接口层 (HIL) 定义。
//!
//! 大多数“不安全”代码都在这个内核包中。
//!
//!
//! ## 核心内核可见性
//!
//! 作为 Tock 操作系统中的root  crate，这个 crate 服务于多个目的：
//!
//! 1. 它包括核心内核的逻辑，包括进程管理、Grant、调度等。
//!
//! 2. 它包括硬件和其他设备抽象的重要接口。 这些通常位于 HIL 和平台文件夹中
//!
//! 3. 它包括内核中其他地方使用的实用程序函数，
//!    通常由多个不同的 crate 使用，因此在核心内核 crate 中共享实现是有意义的。
//!
//! 由于核心内核的这些不同特性，管理各种对象和函数的可见性有点棘手。
//! 通常，内核 crate 只公开它绝对需要的内容。
//! 但是，在三种情况下，此 crate 中的资源_必须_被公开。
//!
//! 1. 必须公开共享的实用程序函数和结构。这些被标记为 pub 并被许多其他内核 crate 使用。
//!
//!    然而，一些实用程序对象和抽象会暴露内存不安全行为。 这些被标记为“不安全”，
//!    并且需要一个“不安全”块才能使用它们。 其中一个例子是“StaticRef”，
//!    它用于访问内存映射的 I/O 寄存器。 由于仅通过内存地址访问地址可能非常不安全，
//!    因此实例化 `StaticRef` 需要一个 `unsafe` 块。
//!
//! 2. 核心内核类型通常必须pub，因为操作系统的其他层需要使用它们。
//!    但是，通常只暴露一个很小的接口，使用该接口不会损害整个系统或核心内核。
//!    这些函数也被标记为“pub”。 例如，“ProcessBuffer”抽象必须暴露给Capsule，
//!    以使用进程和内核之间的共享内存。 但是，构造函数是不公开的，
//!    暴露给Capsule的 API 非常有限，并且受到 Rust 类型系统的限制。
//!    构造函数和其他敏感接口仅限于在内核 crate 内使用，并标记为 pub(crate)。
//!
//!    在某些情况下，必须公开更敏感的核心内核接口。 例如，内核公开了一个用于在内核
//!    中启动主调度循环的函数。 由于board crate必须能够在所有初始化完成后启动此循环，
//!    因此必须公开内核循环函数并标记为“pub”。 但是，此接口通常使用起来并不安全，
//!    因为第二次启动循环会损害整个系统的稳定性。 再次调用启动循环函数也不一定是内存不安全的，
//!    所以我们不将其标记为“不安全”。 相反，我们要求调用者持有一个“Capability”来调用公共但敏感的函数。
//!    更多信息在 `capabilities.rs` 中。 这允许内核 crate 仍然将函数公开，同时限制它们的使用。
//!    另一个例子是 `Grant` 构造函数，它必须在核心内核之外调用.但除了在board setup期间不应调用。
//!
//! 3. 某些内部核心内核接口也必须公开.这些对于恰好在内核 crate 之外的 crate 中实现的
//!    核心内核的扩展是必需的。 例如，“Process”的其他实现可能存在于内核 crate 之外。
//!    要成功实现一个新的 `Process` 需要访问某些内核内核 API，
//!    并且这些 API 必须标记为 `pub` 以便外部 crate 可以访问它们。
//!
//!    这些接口非常敏感，因此我们再次要求调用者拥有调用它们的Capability。
//!    这有助于限制它们的使用，并清楚地表明调用它们需要special permission。
//!    此外，为了将这些用于核心内核功能的外部扩展的接口与其他公共但敏感的接口（上面的第 2 项）
//!    区分开来，我们将名称 _external 附加到函数名称中。
//!
//!    需要注意的是，目前在内核 crate 之外的核心内核扩展很少。
//!    这意味着我们不必为这个用例所需的所有接口创建 `_extern` 函数。
//!    随着新用例的发现，我们可能不得不创建新接口。

#![feature(core_intrinsics, const_fn_trait_bound)]
#![warn(unreachable_pub)]
#![no_std]

// 定义内核主要和次要版本
pub const MAJOR: u16 = 2;
pub const MINOR: u16 = 0;

pub mod capabilities;
pub mod collections;
pub mod component;
pub mod debug;
pub mod deferred_call;
pub mod dynamic_deferred_call;
pub mod errorcode;
pub mod grant;
pub mod hil;
pub mod introspection;
pub mod ipc;
pub mod platform;
pub mod process;
pub mod processbuffer;
pub mod scheduler;
pub mod syscall;
pub mod upcall;
pub mod utilities;

mod config;
mod kernel;
mod memop;
mod process_policies;
mod process_printer;
mod process_standard;
mod process_utilities;
mod syscall_driver;

// 核心资源公开为 `kernel::Type`.
pub use crate::errorcode::ErrorCode;
pub use crate::kernel::Kernel;
pub use crate::process::ProcessId;
pub use crate::scheduler::Scheduler;
