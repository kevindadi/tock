//! Interface for configuring the Memory Protection Unit.

use crate::process::ProcessId;
use core::cmp;
use core::fmt::{self, Display};

/// 用户模式访问权限。
#[derive(Copy, Clone)]
pub enum Permissions {
    ReadWriteExecute,
    ReadWriteOnly,
    ReadExecuteOnly,
    ReadOnly,
    ExecuteOnly,
}

/// MPU region.
///
/// 这是一个受 MPU 保护的连续地址空间。
#[derive(Copy, Clone, PartialEq, Eq)]
pub struct Region {
    /// 区域开始的内存地址.
    ///
    /// 为了获得最大的兼容性，我们使用 u8 指针,
    /// 但是请注意，许多内存保护单元对 MPU 保护的内存区域有非常严格的对齐要求.
    start_address: *const u8,

    /// MPU 区域中的内存字节数.
    size: usize,
}

impl Region {
    /// 创建一个具有给定起点和长度（以字节为单位）的新 MPU 区域。
    pub fn new(start_address: *const u8, size: usize) -> Region {
        Region {
            start_address: start_address,
            size: size,
        }
    }

    /// Getter：获取MPU区域的起始地址。
    pub fn start_address(&self) -> *const u8 {
        self.start_address
    }

    /// Getter：以字节为单位检索区域的长度。
    pub fn size(&self) -> usize {
        self.size
    }
}

/// `MPU` trait 实现中`MpuConfig` 类型的默认类型的 Null 类型。
/// 这种自定义类型允许我们使用空实现来实现 `Display` 以满足对 `type MpuConfig` 的约束。
#[derive(Default)]
pub struct MpuConfigDefault {}

impl Display for MpuConfigDefault {
    fn fmt(&self, _f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Ok(())
    }
}

/// 特定内存保护单元实现需要实现的通用特征。
///
/// 此特性融合了相对通用的 MPU 功能，这些功能应该在不同的 MPU 实现中通用，
/// 以及 Tock 支持保护应用程序所需的更具体的要求。 虽然可能需要一个不太特定于 Tock
/// 的接口，但由于有时复杂的对齐规则和 MPU 硬件施加的其他限制，一些 Tock
/// 细节必须传递到该接口中。 这使得 MPU 实现在满足保护要求时具有更大的灵活性，
/// 并且还允许 MPU 在决定将某些应用程序内存区域放置在何处时指定内核使用的一些地址，
/// 以便 MPU 可以适当地为这些内存区域提供保护。
pub trait MPU {
    /// 为 MPU 定义特定配置的 MPU 特定状态。 也就是说，这应该包含所有必需的状态，
    /// 以便实现可以传递这种类型的对象，并且它应该能够正确和完整地配置 MPU。
    ///
    /// 此状态将在每个进程的基础上保存，作为缓存所有进程设置的一种方式。
    /// 当内核切换到新进程时，它将使用该进程的“MpuConfig”来快速配置 MPU。
    ///
    /// 它是 `Default`，因此我们可以在创建进程时创建空状态，以及 `Display`，
    /// 以便 `panic!()` 输出可以显示当前状态以帮助调试。
    type MpuConfig: Default + Display;

    /// Clears the MPU.
    ///
    /// 此功能将尽可能清除 MPU 实施的任何访问控制。
    /// 在某些硬件上，在 MPU 锁定后无法重置它，在这种情况下，此功能不会更改这些区域。
    fn clear_mpu(&self) {}

    /// Enables the MPU for userspace apps.
    ///
    /// 为用户空间应用启用MPU。此功能必须启用对MPU保护的各个区域的权限限制。
    fn enable_app_mpu(&self) {}

    /// Disables the MPU for userspace apps.
    ///
    /// 如果此功能会干扰内核，则必须禁用以前为应用程序设置的任何访问控制。
    /// 这将在内核开始执行之前调用，因为在某些平台上，MPU 规则也适用于特权代码，
    /// 因此必须禁用某些 MPU 配置，内核才能有效地管理进程。
    fn disable_app_mpu(&self) {}

    /// 返回 MPU 支持的最大区域数。
    fn number_total_regions(&self) -> usize {
        0
    }

    /// Allocates a new MPU region.
    ///
    /// 实现必须在指定的未分配内存范围内分配至少 `min_region_size` 字节大小的
    /// MPU 区域，并具有指定的用户模式权限，并将其存储在 `config` 中。
    /// 分配的区域不能与任何已经存储在 `config` 中的区域重叠。
    ///
    /// # Arguments
    ///
    /// - `unallocated_memory_start`: start of unallocated memory
    /// - `unallocated_memory_size`:  size of unallocated memory
    /// - `min_region_size`:          minimum size of the region
    /// - `permissions`:              permissions for the region
    /// - `config`:                   MPU region configuration
    ///
    /// # Return Value
    ///
    /// 返回分配的 MPU 区域的开始和大小。 如果分配 MPU 区域不可行，则返回 None。
    #[allow(unused_variables)]
    fn allocate_region(
        &self,
        unallocated_memory_start: *const u8,
        unallocated_memory_size: usize,
        min_region_size: usize,
        permissions: Permissions,
        config: &mut Self::MpuConfig,
    ) -> Option<Region> {
        if min_region_size > unallocated_memory_size {
            None
        } else {
            Some(Region::new(unallocated_memory_start, min_region_size))
        }
    }

    /// 删除应用程序拥有的内存中的 MPU 区域。
    ///
    /// 实现必须删除与 region 参数匹配的 MPU 区域（如果存在）。
    /// 如果没有完全匹配的区域，则实现可能会返回错误。
    /// 实现者不应删除 app_memory_region，如果提供了该区域，则应返回错误。
    ///
    /// # Arguments
    ///
    /// - `region`:    a region previously allocated with `allocate_region`
    /// - `config`:    MPU region configuration
    ///
    /// # Return Value
    ///
    /// 如果指定区域未完全映射到指定的进程，则返回错误
    #[allow(unused_variables)]
    fn remove_memory_region(&self, region: Region, config: &mut Self::MpuConfig) -> Result<(), ()> {
        Ok(())
    }

    /// 选择进程内存的位置，并分配一个覆盖应用程序拥有部分的 MPU 区域。
    ///
    /// 实现必须选择一个连续的内存块，其大小至少为“min_memory_size”字节，
    /// 并且完全位于指定的未分配内存范围内。
    ///
    /// 它还必须分配具有以下属性的 MPU 区域：
    ///
    /// 1. 该区域至少覆盖内存块开头的第一个“initial_app_memory_size”字节。
    /// 2. 该区域不与最后的“initial_kernel_memory_size”字节重叠。
    /// 3. 该区域具有 `permissions` 指定的用户模式权限。
    ///
    /// 未来应用程序拥有的内存的结束地址会增加，因此实现应该选择进程内存块的位置，
    /// 以便 MPU 区域可以随之增长。 实现必须将分配的区域存储在 `config` 中。
    /// 分配的区域不能与任何已经存储在 `config` 中的区域重叠。
    ///
    /// # Arguments
    ///
    /// - `unallocated_memory_start`:   start of unallocated memory
    /// - `unallocated_memory_size`:    size of unallocated memory
    /// - `min_memory_size`:            minimum total memory to allocate for process
    /// - `initial_app_memory_size`:    initial size of app-owned memory
    /// - `initial_kernel_memory_size`: initial size of kernel-owned memory
    /// - `permissions`:                permissions for the MPU region
    /// - `config`:                     MPU region configuration
    ///
    /// # Return Value
    ///
    /// 此函数返回为进程选择的内存块的起始地址和大小。 如果无法找到内存块或分配 MPU 区域，
    /// 或者如果函数已被调用，则返回 None。 如果 None 返回，则不会进行任何更改。
    #[allow(unused_variables)]
    fn allocate_app_memory_region(
        &self,
        unallocated_memory_start: *const u8,
        unallocated_memory_size: usize,
        min_memory_size: usize,
        initial_app_memory_size: usize,
        initial_kernel_memory_size: usize,
        permissions: Permissions,
        config: &mut Self::MpuConfig,
    ) -> Option<(*const u8, usize)> {
        let memory_size = cmp::max(
            min_memory_size,
            initial_app_memory_size + initial_kernel_memory_size,
        );
        if memory_size > unallocated_memory_size {
            None
        } else {
            Some((unallocated_memory_start, memory_size))
        }
    }

    /// Updates the MPU region for app-owned memory.
    ///
    /// 实现必须为存储在 config 中的应用程序拥有的内存重新分配 MPU 区域，
    /// 以维持 allocate_app_memory_region 中描述的 3 个条件。
    ///
    /// # Arguments
    ///
    /// - `app_memory_break`:    new address for the end of app-owned memory
    /// - `kernel_memory_break`: new address for the start of kernel-owned memory
    /// - `permissions`:         permissions for the MPU region
    /// - `config`:              MPU region configuration
    ///
    /// # Return Value
    ///
    /// 如果更新 MPU 区域不可行，或者从未创建过，则返回错误。
    /// 如果返回错误，则不会对配置进行任何更改。
    #[allow(unused_variables)]
    fn update_app_memory_region(
        &self,
        app_memory_break: *const u8,
        kernel_memory_break: *const u8,
        permissions: Permissions,
        config: &mut Self::MpuConfig,
    ) -> Result<(), ()> {
        if (app_memory_break as usize) > (kernel_memory_break as usize) {
            Err(())
        } else {
            Ok(())
        }
    }

    /// 使用提供的region配置配置 MPU。
    ///
    /// 实现必须确保分配区域未覆盖的所有内存位置在用户模式下不可访问，而在supervisor模式下可访问。
    ///
    /// # Arguments
    ///
    /// - `config`: MPU region configuration
    /// - `app_id`: ProcessId of the process that the MPU is configured for
    #[allow(unused_variables)]
    fn configure_mpu(&self, config: &Self::MpuConfig, app_id: &ProcessId) {}
}

/// 为单元实现默认的 MPU 特征。
impl MPU for () {
    type MpuConfig = MpuConfigDefault;
}

/// 特定内核级内存保护单元实现需要实现的通用特征。
///
/// 这个 trait 提供了通用功能来扩展上面的 MPU trait 以允许内核保护自己。
/// 预计只有有限数量的 SoC 可以支持这一点，这就是为什么它是一个单独的实现。
pub trait KernelMPU {
    /// MPU-specific state that defines a particular configuration for the kernel
    /// MPU.
    /// That is, this should contain all of the required state such that the
    /// implementation can be passed an object of this type and it should be
    /// able to correctly and entirely configure the MPU.
    ///
    /// It is `Default` so we can create empty state when the kernel is
    /// created, and `Display` so that the `panic!()` output can display the
    /// current state to help with debugging.
    type KernelMpuConfig: Default + Display;

    /// Mark a region of memory that the Tock kernel owns.
    ///
    /// This function will optionally set the MPU to enforce the specified
    /// constraints for all accessess (even from the kernel).
    /// This should be used to mark read/write/execute areas of the Tock
    /// kernel to have the hardware enforce those permissions.
    ///
    /// If the KernelMPU trait is supported a board should use this function
    /// to set permissions for all areas of memory the kernel will use.
    /// Once all regions of memory have been allocated, the board must call
    /// enable_kernel_mpu(). After enable_kernel_mpu() is called no changes
    /// to kernel level code permissions can be made.
    ///
    /// Note that kernel level permissions also apply to apps, although apps
    /// will have more constraints applied on top of the kernel ones as
    /// specified by the `MPU` trait.
    ///
    /// Not all architectures support this, so don't assume this will be
    /// implemented.
    ///
    /// # Arguments
    ///
    /// - `memory_start`:             start of memory region
    /// - `memory_size`:              size of unallocated memory
    /// - `permissions`:              permissions for the region
    /// - `config`:                   MPU region configuration
    ///
    /// # Return Value
    ///
    /// Returns the start and size of the requested memory region. If it is
    /// infeasible to allocate the MPU region, returns None. If None is
    /// returned no changes are made.
    #[allow(unused_variables)]
    fn allocate_kernel_region(
        &self,
        memory_start: *const u8,
        memory_size: usize,
        permissions: Permissions,
        config: &mut Self::KernelMpuConfig,
    ) -> Option<Region>;

    /// Enables the MPU for the kernel.
    ///
    /// This function must enable the permission restrictions on the various
    /// kernel regions specified by `allocate_kernel_region()` protected by
    /// the MPU.
    ///
    /// It is expected that this function is called in `main()`.
    ///
    /// Once enabled this cannot be disabled. It is expected there won't be any
    /// changes to the kernel regions after this is enabled.
    #[allow(unused_variables)]
    fn enable_kernel_mpu(&self, config: &mut Self::KernelMpuConfig);
}
