//! 系统调用 MEMOP 系列的实现

use crate::process::Process;
use crate::syscall::SyscallReturn;
use crate::ErrorCode;

/// Handle the `memop` syscall.
///
/// ### `memop_num`
///
/// - `0`：BRK。 更改程序break的位置并返回 SyscallReturn
/// - `1`: SBRK. 改变程序break的位置，返回之前的break地址
/// - `2`: 获取应用程序 RAM 分配的起始地址
/// - `3`: 获取指向应用程序RAM分配结束后第一个地址的地址.
/// - `4`: 获取应用程序flash区域的起始地址。 这是 TBF 标头所在的位置.
/// - `5`: 获取指向应用程序flash区域结束后第一个地址的地址.
/// - `6`: 获取应用的Grant区域最低地址的地址.
/// - `7`: 获取此应用程序标头中定义的可写flash区域的数量.
/// - `8`: 获取由 r1 从 0 开始索引的可写区域的起始地址.
///        失败时返回 (void*) -1，表示选定的可写区域不存在.
/// - `9`: 获取由 r1 索引的可写区域的结束地址.失败时返回 (void*) -1，表示选定的可写区域不存在。
/// - `10`: 指定应用程序堆栈的开始位置.这告诉内核应用程序将其堆栈的开始放在哪里.
///         这不是正确操作所必需的,但可以在应用程序崩溃时进行更好的调试.
/// - `11`: 指定应用程序堆的起始位置,这告诉内核应用程序将其堆的开始放在哪里.
///         这不是正确操作所必需的,但可以在应用程序崩溃时进行更好的调试.
pub(crate) fn memop(process: &dyn Process, op_type: usize, r1: usize) -> SyscallReturn {
    match op_type {
        // Op Type 0: BRK
        0 /* BRK */ => {
            process.brk(r1 as *const u8)
                .map(|_| SyscallReturn::Success)
                .unwrap_or(SyscallReturn::Failure(ErrorCode::NOMEM))
        },

        // Op Type 1: SBRK
        1 /* SBRK */ => {
            process.sbrk(r1 as isize)
                .map(|addr| SyscallReturn::SuccessU32(addr as u32))
                .unwrap_or(SyscallReturn::Failure(ErrorCode::NOMEM))
        },

        // Op Type 2: Process memory start
        2 => SyscallReturn::SuccessU32(process.get_addresses().sram_start as u32),

        // Op Type 3: Process memory end
        3 => SyscallReturn::SuccessU32(process.get_addresses().sram_end as u32),

        // Op Type 4: Process flash start
        4 => SyscallReturn::SuccessU32(process.get_addresses().flash_start as u32),

        // Op Type 5: Process flash end
        5 => SyscallReturn::SuccessU32(process.get_addresses().flash_end as u32),

        // Op Type 6: Grant region begin
        6 => SyscallReturn::SuccessU32(process.get_addresses().sram_grant_start as u32),

        // Op Type 7: Number of defined writeable regions in the TBF header.
        7 => SyscallReturn::SuccessU32(process.number_writeable_flash_regions() as u32),

        // Op Type 8: The start address of the writeable region indexed by r1.
        8 => {
            let flash_start = process.get_addresses().flash_start as u32;
            let (offset, size) = process.get_writeable_flash_region(r1);
            if size == 0 {
                SyscallReturn::Failure(ErrorCode::FAIL)
            } else {
                SyscallReturn::SuccessU32(flash_start + offset)
            }
        }

        // Op Type 9: The end address of the writeable region indexed by r1.
        // Returns (void*) -1 on failure, meaning the selected writeable region
        // does not exist.
        9 => {
            let flash_start = process.get_addresses().flash_start as u32;
            let (offset, size) = process.get_writeable_flash_region(r1);
            if size == 0 {
                SyscallReturn::Failure(ErrorCode::FAIL)
            } else {
                SyscallReturn::SuccessU32(flash_start + offset + size)
            }
        }

        // Op Type 10: Specify where the start of the app stack is.
        10 => {
            process.update_stack_start_pointer(r1 as *const u8);
            SyscallReturn::Success
        }

        // Op Type 11: Specify where the start of the app heap is.
        11 => {
            process.update_heap_start_pointer(r1 as *const u8);
            SyscallReturn::Success
        }

        _ => SyscallReturn::Failure(ErrorCode::NOSUPPORT),
    }
}
