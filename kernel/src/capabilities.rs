//! Special restricted capabilities.
//!
//! Rust 提供了一种机制，通过 `unsafe` 关键字限制某些操作只能由受信任的代码使用。
//! 这非常有用，但不能提供非常精细的访问：代码可以访问_所有_`不安全`的东西，也可以不访问。
//!
//! Capabilitys是 Tock 中提供更细粒度访问的机制。 对于敏感操作（例如可能违反隔离的操作），
//! 调用者必须具有特定的能力。
//! 类型系统确保调用者确实具有能力，并且使用“不安全”来确保调用者不能自己创建Capability类型。
//!
//! 功能从受信任的代码（即可以调用“不安全”的代码）传递给模块。
//!
//! Capabilities被表示为“不安全”的特征。 只有可以使用“不安全”机制的代码才能实例化提供“不安全”特征的对象。
//! 需要某些功能的函数需要传递一个提供正确功能特征的对象。 对象本身不必标记为“不安全”。
//!
//! 创建一个表达能力的对象很简单：
//!
//! ```
//! use kernel::capabilities::ProcessManagementCapability;
//!
//! struct ProcessMgmtCap;
//! unsafe impl ProcessManagementCapability for ProcessMgmtCap {}
//! ```
//!
//! Now anything that has a ProcessMgmtCap can call any function that requires
//! the `ProcessManagementCapability` capability.
//!
//! Requiring a certain capability is also straightforward:
//!
//! ```ignore
//! pub fn manage_process<C: ProcessManagementCapability>(_c: &C) {
//!    unsafe {
//!        ...
//!    }
//! }
//! ```
//!
//! 任何调用 `manage_process` 的东西
//! 都必须引用某个提供 `ProcessManagementCapability` 特征的对象，这证明它具有正确的能力。

/// `ProcessManagementCapability` 允许持有者控制Process执行，
/// 例如与创建、重新启动和以其他方式管理Process相关的操作。
pub unsafe trait ProcessManagementCapability {}

/// `MainLoopCapability` 允许持有者开始执行以及管理 Tock 中的主调度程序循环。
/// 这是在board的 main.rs 文件中启动内核所必需的。
/// 它还允许“Process”的外部实现来更新主循环使用的内核结构中的状态。
pub unsafe trait MainLoopCapability {}

/// `MemoryAllocationCapability` 允许持有者分配内存，例如通过创建Grant。
pub unsafe trait MemoryAllocationCapability {}

/// `ExternalProcessCapability` 允许持有者使用从kernel crate 外部成功实现 `Process` 所需的内核资源。
/// 其中许多操作非常敏感，即它们不能仅仅公开。 特别是，某些对象可以在内核之外使用，但必须限制构造函数。
pub unsafe trait ExternalProcessCapability {}

/// `UdpDriverCapability` 允许持有者使用仅由 UDP 驱动程序允许的两个功能。
/// 第一个是 udp_send.rs 中的 `driver_send_to()` 函数，它不需要绑定到单个端口，
/// 因为驱动程序自己管理应用程序的端口绑定。 第二个是 udp_port_table.rs 中的 set_user_ports() 函数，
/// 它为 UDP 端口表提供了对 UDP 驱动程序的引用，以便它可以检查哪些端口已被应用程序绑定。
pub unsafe trait UdpDriverCapability {}

/// `CreatePortTableCapability` 允许持有者实例化 UdpPortTable 结构的新副本。
/// 这个结构应该只有一个实例，所以这个能力根本不应该分发给Capsule，因为端口表应该只被内核实例化一次
pub unsafe trait CreatePortTableCapability {}

/// `NetworkCapabilityCreationCapability` 持有者为网络堆栈的 IP 和 UDP 层
/// 实例化 `NetworkCapability`S 和可见性能力。
/// Capsule永远不会拥有这种能力，尽管它可能拥有通过这种Capability创建的能力。
pub unsafe trait NetworkCapabilityCreationCapability {}
