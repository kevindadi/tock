//! 用于将应用程序内存传递给内核的数据结构。
//!
//! Tock 进程可以将读写或只读缓冲区传递到内核以供其使用。内核检查读写缓冲区是否存在于
//! 进程的 RAM 地址空间中，以及只读缓冲区是否存在于其 RAM 或闪存地址空间中。
//! 这些缓冲区与 allow_read_write() 和 allow_read_only() 系统调用共享。
//!
//! 读写和只读调用分别映射到高级 Rust 类型 [`ReadWriteProcessBuffer`] 和
//! [`ReadOnlyProcessBuffer`]。 可以通过在进程缓冲区结构上实现的 [`ReadableProcessBuffer`]
//! 和 [`WriteableProcessBuffer`] Trait访问内存区域。
//!
//! 对缓冲区结构的每次访问都需要进行liveness检查，以确保进程内存仍然有效。
//! 对于更传统的界面，用户可以将缓冲区转换为 [`ReadableProcessSlice`] 或
//! [`WriteableProcessSlice`] 并在其操作的lifetime中使用它们。
//! 但是，用户不能保存对这些slices的live-lived reference。

use core::cell::Cell;
use core::marker::PhantomData;
use core::ops::{Deref, Index, Range, RangeFrom, RangeTo};

use crate::capabilities;
use crate::process::{self, ProcessId};
use crate::ErrorCode;

/// 将进程缓冲区的内部表示转换为 ReadableProcessSlice。
///
/// 无论 `ptr` 的值如何，此函数都会自动将零长度的进程缓冲区转换为有效的零大小的 Rust 切片。
///
/// # Safety requirements
///
/// 在`len != 0`的情况下，内存`[ptr; ptr + len)` 必须在单个进程的地址空间内，并且 `ptr` 必须非零。
/// 该内存区域必须映射为_可读_，并且可选地映射为_可写_和_可执行_。
/// 它必须在整个生命周期“a”内分配在单个进程的地址空间中。
///
/// 多个重叠的 [`ReadableProcessSlice`] 或 [`WriteableProcessSlice`] 同时在范围内是合理的. 引用规范
unsafe fn raw_processbuf_to_roprocessslice<'a>(
    ptr: *const u8,
    len: usize,
) -> &'a ReadableProcessSlice {
    // 将对 Cell<u8> 切片的引用转换为对 ReadableProcessSlice 的引用。 因为 ReadableProcessSlice
    // 是围绕 [ReadableProcessByte] 的 #[repr(transparent)] 包装器，它是围绕 [Cell<u8>] 的
    // #[repr(transparent)] 包装器，它是 #[repr(transparent) ] 包裹 [UnsafeCell<u8>]，
    // 最后 #[repr(transparent)] 包裹 [u8]
    core::mem::transmute::<&[u8], &ReadableProcessSlice>(
        // Rust 对指针有效性[1] 有非常严格的要求，这也部分适用于长度为 0 的访问。如果缓冲区长度为 0，
        // 我们允许应用程序提供任意指针，但 Rust 切片不允许这样做。
        // 例如，空指针_从不_有效，即使对于大小为零的访问也是如此。
        //
        // 要获得一个不指向有效（已分配）内存的指针，但对于大小为零的访问是安全的，我们必须调用 NonNull::dangling()。
        // 生成的指针保证对齐良好，并支持大小为零的访问所需的保证。
        //
        // [1]: https://doc.rust-lang.org/core/ptr/index.html#safety
        match len {
            0 => core::slice::from_raw_parts(core::ptr::NonNull::<u8>::dangling().as_ptr(), 0),
            _ => core::slice::from_raw_parts(ptr, len),
        },
    )
}

/// Convert an process buffers's internal representation to a
/// WriteableProcessSlice.
///
/// This function will automatically convert zero-length process
/// buffers into valid zero-sized Rust slices regardless of the value
/// of `ptr`.
///
/// # Safety requirements
///
/// In the case of `len != 0`, the memory `[ptr; ptr + len)` must be
/// within a single process' address space, and `ptr` must be
/// nonzero. This memory region must be mapped as _readable_ and
/// _writable_, and optionally _executable_. It must be allocated
/// within a single process' address space for the entire lifetime
/// `'a`.
///
/// No other mutable or immutable Rust reference pointing to an
/// overlapping memory region, which is not also created over
/// `UnsafeCell`, may exist over the entire lifetime `'a`. Even though
/// this effectively returns a slice of [`Cell`]s, writing to some
/// memory through a [`Cell`] while another reference is in scope is
/// unsound. Because a process is free to modify its memory, this is
/// -- in a broader sense -- true for all process memory.
///
/// However, it is sound for multiple overlapping
/// [`ReadableProcessSlice`]s or [`WriteableProcessSlice`]s to be in
/// scope at the same time.
unsafe fn raw_processbuf_to_rwprocessslice<'a>(
    ptr: *mut u8,
    len: usize,
) -> &'a WriteableProcessSlice {
    // Transmute a reference to a slice of Cell<u8>s into a reference
    // to a ReadableProcessSlice. This is possible as
    // ReadableProcessSlice is a #[repr(transparent)] wrapper around a
    // [ReadableProcessByte], which is a #[repr(transparent)] wrapper
    // around a [Cell<u8>], which is a #[repr(transparent)] wrapper
    // around an [UnsafeCell<u8>], which finally #[repr(transparent)]
    // wraps a [u8]
    core::mem::transmute::<&[u8], &WriteableProcessSlice>(
        // Rust has very strict requirements on pointer validity[1]
        // which also in part apply to accesses of length 0. We allow
        // an application to supply arbitrary pointers if the buffer
        // length is 0, but this is not allowed for Rust slices. For
        // instance, a null pointer is _never_ valid, not even for
        // accesses of size zero.
        //
        // To get a pointer which does not point to valid (allocated)
        // memory, but is safe to construct for accesses of size zero,
        // we must call NonNull::dangling(). The resulting pointer is
        // guaranteed to be well-aligned and uphold the guarantees
        // required for accesses of size zero.
        //
        // [1]: https://doc.rust-lang.org/core/ptr/index.html#safety
        match len {
            0 => core::slice::from_raw_parts_mut(core::ptr::NonNull::<u8>::dangling().as_ptr(), 0),
            _ => core::slice::from_raw_parts_mut(ptr, len),
        },
    )
}

/// 用户空间进程内存的可读区域。
///
/// 此 trait 可用于获得对封装在 [`ReadOnlyProcessBuffer`] 或 [`ReadWriteProcessBuffer`] 类型中的内存区域的只读访问权限。
pub trait ReadableProcessBuffer {
    /// 内存区域的长度。
    ///
    /// 如果进程不再存活并且内存已经被回收，这个方法必须返回0。
    ///
    /// # 默认进程缓冲区
    ///
    /// 进程缓冲区的默认实例必须返回 0。
    fn len(&self) -> usize;

    /// 指向用户空间内存区域的第一个字节的指针。
    ///
    /// 如果初始共享内存区域的长度（与 [`len`](ReadableProcessBuffer::len) 的返回值无关）为 0，
    /// 则此函数返回指向地址 `0x0` 的指针。 这是因为进程可能允许长度为 0 的缓冲区不与内核共享内存。
    /// 因为这些缓冲区的长度为零，所以它们可以有任何指针值。 但是，这些 _dummy 地址_ 不应该被泄露，
    /// 因此对于零长度切片，此方法返回 0。
    ///
    /// # 默认进程缓冲区
    ///
    /// 进程缓冲区的默认实例必须返回指向地址 `0x0` 的指针。
    fn ptr(&self) -> *const u8;

    /// 将函数应用于进程缓冲区指向的（只读）进程切片引用。
    ///
    /// 如果进程不再存活并且内存已经被回收，这个方法必须返回`Err(process::Error::NoSuchApp)`。
    ///
    /// # 默认进程缓冲区
    ///
    /// 进程缓冲区的默认实例必须返回 `Err(process::Error::NoSuchApp)` 而不执行传递的闭包。
    fn enter<F, R>(&self, fun: F) -> Result<R, process::Error>
    where
        F: FnOnce(&ReadableProcessSlice) -> R;
}

/// 用户空间进程内存的可读写区域。
///
/// 此特征可用于获得对包装在 [`ReadWriteProcessBuffer`] 中的内存区域的读写访问权限。
///
/// 这是 [`ReadableProcessBuffer`] 的超特征，它具有允许可变访问的方法。
pub trait WriteableProcessBuffer: ReadableProcessBuffer {
    /// 将函数应用于 [`ReadWriteProcessBuffer`] 指向的可变进程切片引用。
    ///
    /// 如果进程不再存活并且内存已经被回收，这个方法必须返回`Err(process::Error::NoSuchApp)`。
    ///
    /// # 默认进程缓冲区
    //
    /// 进程缓冲区的默认实例必须返回 `Err(process::Error::NoSuchApp)` 而不执行传递的闭包。
    fn mut_enter<F, R>(&self, fun: F) -> Result<R, process::Error>
    where
        F: FnOnce(&WriteableProcessSlice) -> R;
}

/// 用户空间进程共享的只读缓冲区

/// 当进程“允许”其内存的特定部分给内核并授予内核对该内存的读取访问权限时，此结构将提供给Capsule。
///
/// 可用于获取 [`ReadableProcessSlice`]，它基于 [`Cell`] 的切片。
/// 这是因为用户空间可以“允许”内存的重叠部分进入不同的 [`ReadableProcessSlice`]。
/// 在 Rust 中至少有一个可变的 Rust 切片以及与重叠内存的只读切片违反了 Rust 的aliasing rules。
/// [`Cell`]切片通过明确支持内部可变性避免了这个问题。尽管如此，在切换到用户空间之前还是需要一个memory barrier，
/// 因为编译器可以自由地重新排序读取和写入，即使通过 [`Cell`]s 也是如此。
pub struct ReadOnlyProcessBuffer {
    ptr: *const u8,
    len: usize,
    process_id: Option<ProcessId>,
}

impl ReadOnlyProcessBuffer {
    /// 在给定的指针和长度上构造一个新的 [`ReadOnlyProcessBuffer`]。
    ///
    /// ＃ 安全要求
    ///
    /// 参考[`ReadOnlyProcessBuffer::new_external`]的安全要求。
    pub(crate) unsafe fn new(ptr: *const u8, len: usize, process_id: ProcessId) -> Self {
        ReadOnlyProcessBuffer {
            ptr,
            len,
            process_id: Some(process_id),
        }
    }

    /// 在给定的指针和长度上构造一个新的 [`ReadOnlyProcessBuffer`]。
    ///
    /// pub 构造函数，需要 [`capabilities::ExternalProcessCapability`] Capability。
    /// 这是为了允许在 `kernel` crate 之外实现 [`Process`](crate::process::Process) trait。
    ///
    /// ＃ 安全要求
    ///
    /// 如果长度为 `0`，则可以将任意指针传递给 `ptr`。它不一定要指向分配的内存，也不一定要满足【Rust 的指针有效性要求】
    /// （https://doc.rust-lang.org/core/ptr/index.html#safety）。
    ///
    /// [`ReadOnlyProcessBuffer`] 必须确保所有长度为 `0` 的 Rust 切片必须在有效（但不一定已分配）的基指针上构造。
    ///
    /// 如果长度不为`0`，则`[ptr; 的内存区域； ptr + len)` 必须是给定 [`ProcessId`] 的进程的有效内存。
    /// 它必须在 [`ReadOnlyProcessBuffer`] 的整个生命周期内进行分配和访问。它不得指向进程可访问内存范围之外的内存，
    /// 或（部分）指向其他进程或内核内存。 `ptr` 必须满足 [Rust 对指针有效性的要求]
    /// (https://doc.rust-lang.org/core/ptr/index.html#safety)，
    /// 特别是在各自的platform上它必须具有 `core::mem::align_of::<u8>()`的最小对齐方式。
    /// 它必须指向映射为_可读_和可选_可写_和_可执行_的内存。
    pub unsafe fn new_external(
        ptr: *const u8,
        len: usize,
        process_id: ProcessId,
        _cap: &dyn capabilities::ExternalProcessCapability,
    ) -> Self {
        Self::new(ptr, len, process_id)
    }

    /// 使用 ReadOnlyProcessBuffer，返回其组成指针和大小。
    /// 这确保不能同时存在“ReadOnlyProcessBuffer”和指向其内部数据的指针。
    ///
    /// `consume` 可以在内核需要跨内核到用户边界传递底层值时使用（例如，将返回值传递给系统调用）。
    pub(crate) fn consume(self) -> (*const u8, usize) {
        (self.ptr, self.len)
    }
}

impl ReadableProcessBuffer for ReadOnlyProcessBuffer {
    fn len(&self) -> usize {
        self.process_id
            .map_or(0, |pid| pid.kernel.process_map_or(0, pid, |_| self.len))
    }

    fn ptr(&self) -> *const u8 {
        if self.len == 0 {
            0x0 as *const u8
        } else {
            self.ptr
        }
    }

    fn enter<F, R>(&self, fun: F) -> Result<R, process::Error>
    where
        F: FnOnce(&ReadableProcessSlice) -> R,
    {
        match self.process_id {
            None => Err(process::Error::NoSuchApp),
            Some(pid) => pid
                .kernel
                .process_map_or(Err(process::Error::NoSuchApp), pid, |_| {
                    // 安全性：`kernel.process_map_or()` 验证进程仍然存在并且它的内存仍然有效。
                    // 特别是，“进程”会跟踪进程“允许”进入内核的内存的“high water mark”。
                    // 因为 `Process` 没有 API 来再次向下移动“high water mark”，一旦 `ProcessBuffer` 被传回内核，
                    // 就会调用它，给定的 `Process` 实现必须假定由曾经允许的“ProcessBuffer”仍在使用中，
                    // 因此在被内核“允许”一次后，将不允许进程释放任何内存。 这保证了缓冲区在这里可以安全地转换为切片。
                    // 有关更多信息，请参阅 tock/tock#2632 上的评论和后续讨论：https://github.com/tock/tock/pull/2632#issuecomment-869974365
                    Ok(fun(unsafe {
                        raw_processbuf_to_roprocessslice(self.ptr, self.len)
                    }))
                }),
        }
    }
}

impl Default for ReadOnlyProcessBuffer {
    fn default() -> Self {
        ReadOnlyProcessBuffer {
            ptr: 0x0 as *mut u8,
            len: 0,
            process_id: None,
        }
    }
}

/// 提供对具有受限生命周期的 ReadOnlyProcessBuffer 的访问。 这会自动取消引用转到 ReadOnlyProcessBuffer
pub struct ReadOnlyProcessBufferRef<'a> {
    buf: ReadOnlyProcessBuffer,
    _phantom: PhantomData<&'a ()>,
}

impl ReadOnlyProcessBufferRef<'_> {
    /// 在给定的指针和长度上构造一个新的 [`ReadOnlyProcessBufferRef`]，其生命周期源自caller。
    ///
    /// ＃ 安全要求
    ///
    /// 参考[`ReadOnlyProcessBuffer::new_external`]的安全要求。
    /// 派生的生命周期可以帮助强制执行此传入指针只能在特定持续时间内访问的不变量。
    pub(crate) unsafe fn new(ptr: *const u8, len: usize, process_id: ProcessId) -> Self {
        Self {
            buf: ReadOnlyProcessBuffer::new(ptr, len, process_id),
            _phantom: PhantomData,
        }
    }
}

impl Deref for ReadOnlyProcessBufferRef<'_> {
    type Target = ReadOnlyProcessBuffer;
    fn deref(&self) -> &Self::Target {
        &self.buf
    }
}

/// Read-writable buffer shared by a userspace process
///
/// This struct is provided to capsules when a process `allows` a
/// particular section of its memory to the kernel and gives the
/// kernel read and write access to this memory.
///
/// It can be used to obtain a [`WriteableProcessSlice`], which is
/// based around a slice of [`Cell`]s. This is because a userspace can
/// `allow` overlapping sections of memory into different
/// [`WriteableProcessSlice`]. Having at least one mutable Rust slice
/// along with read-only or other mutable slices to overlapping memory
/// in Rust violates Rust's aliasing rules. A slice of [`Cell`]s
/// avoids this issue by explicitly supporting interior
/// mutability. Still, a memory barrier prior to switching to
/// userspace is required, as the compiler is free to reorder reads
/// and writes, even through [`Cell`]s.
pub struct ReadWriteProcessBuffer {
    ptr: *mut u8,
    len: usize,
    process_id: Option<ProcessId>,
}

impl ReadWriteProcessBuffer {
    /// Construct a new [`ReadWriteProcessBuffer`] over a given
    /// pointer and length.
    ///
    /// # Safety requirements
    ///
    /// Refer to the safety requirements of
    /// [`ReadWriteProcessBuffer::new_external`].
    pub(crate) unsafe fn new(ptr: *mut u8, len: usize, process_id: ProcessId) -> Self {
        ReadWriteProcessBuffer {
            ptr,
            len,
            process_id: Some(process_id),
        }
    }

    /// Construct a new [`ReadWriteProcessBuffer`] over a given
    /// pointer and length.
    ///
    /// Publicly accessible constructor, which requires the
    /// [`capabilities::ExternalProcessCapability`] capability. This
    /// is provided to allow implementations of the
    /// [`Process`](crate::process::Process) trait outside of the
    /// `kernel` crate.
    ///
    /// # Safety requirements
    ///
    /// If the length is `0`, an arbitrary pointer may be passed into
    /// `ptr`. It does not necessarily have to point to allocated
    /// memory, nor does it have to meet [Rust's pointer validity
    /// requirements](https://doc.rust-lang.org/core/ptr/index.html#safety).
    /// [`ReadWriteProcessBuffer`] must ensure that all Rust slices
    /// with a length of `0` must be constructed over a valid (but not
    /// necessarily allocated) base pointer.
    ///
    /// If the length is not `0`, the memory region of `[ptr; ptr +
    /// len)` must be valid memory of the process of the given
    /// [`ProcessId`]. It must be allocated and and accessible over
    /// the entire lifetime of the [`ReadWriteProcessBuffer`]. It must
    /// not point to memory outside of the process' accessible memory
    /// range, or point (in part) to other processes or kernel
    /// memory. The `ptr` must meet [Rust's requirements for pointer
    /// validity](https://doc.rust-lang.org/core/ptr/index.html#safety),
    /// in particular it must have a minimum alignment of
    /// `core::mem::align_of::<u8>()` on the respective platform. It
    /// must point to memory mapped as _readable_ and optionally
    /// _writable_ and _executable_.
    pub unsafe fn new_external(
        ptr: *mut u8,
        len: usize,
        process_id: ProcessId,
        _cap: &dyn capabilities::ExternalProcessCapability,
    ) -> Self {
        Self::new(ptr, len, process_id)
    }

    /// Consumes the ReadWriteProcessBuffer, returning its constituent
    /// pointer and size. This ensures that there cannot
    /// simultaneously be both a `ReadWriteProcessBuffer` and a pointer to
    /// its internal data.
    ///
    /// `consume` can be used when the kernel needs to pass the
    /// underlying values across the kernel-to-user boundary (e.g., in
    /// return values to system calls).
    pub(crate) fn consume(self) -> (*mut u8, usize) {
        (self.ptr, self.len)
    }

    /// This is a `const` version of `Default::default` with the same
    /// semantics.
    ///
    /// Having a const initializer allows initializing a fixed-size
    /// array with default values without the struct being marked
    /// `Copy` as such:
    ///
    /// ```
    /// use kernel::processbuffer::ReadWriteProcessBuffer;
    /// const DEFAULT_RWPROCBUF_VAL: ReadWriteProcessBuffer
    ///     = ReadWriteProcessBuffer::const_default();
    /// let my_array = [DEFAULT_RWPROCBUF_VAL; 12];
    /// ```
    pub const fn const_default() -> Self {
        Self {
            ptr: 0x0 as *mut u8,
            len: 0,
            process_id: None,
        }
    }
}

impl ReadableProcessBuffer for ReadWriteProcessBuffer {
    fn len(&self) -> usize {
        self.process_id
            .map_or(0, |pid| pid.kernel.process_map_or(0, pid, |_| self.len))
    }

    fn ptr(&self) -> *const u8 {
        if self.len == 0 {
            0x0 as *const u8
        } else {
            self.ptr
        }
    }

    fn enter<F, R>(&self, fun: F) -> Result<R, process::Error>
    where
        F: FnOnce(&ReadableProcessSlice) -> R,
    {
        match self.process_id {
            None => Err(process::Error::NoSuchApp),
            Some(pid) => pid
                .kernel
                .process_map_or(Err(process::Error::NoSuchApp), pid, |_| {
                    // Safety: `kernel.process_map_or()` validates that
                    // the process still exists and its memory is still
                    // valid. In particular, `Process` tracks the "high water
                    // mark" of memory that the process has `allow`ed to the
                    // kernel. Because `Process` does not feature an API to
                    // move the "high water mark" down again, which would be
                    // called once a `ProcessBuffer` has been passed back into
                    // the kernel, a given `Process` implementation must assume
                    // that the memory described by a once-allowed
                    // `ProcessBuffer` is still in use, and thus will not
                    // permit the process to free any memory after it has
                    // been `allow`ed to the kernel once. This guarantees
                    // that the buffer is safe to convert into a slice
                    // here. For more information, refer to the
                    // comment and subsequent discussion on tock/tock#2632:
                    // https://github.com/tock/tock/pull/2632#issuecomment-869974365
                    Ok(fun(unsafe {
                        raw_processbuf_to_roprocessslice(self.ptr, self.len)
                    }))
                }),
        }
    }
}

impl WriteableProcessBuffer for ReadWriteProcessBuffer {
    fn mut_enter<F, R>(&self, fun: F) -> Result<R, process::Error>
    where
        F: FnOnce(&WriteableProcessSlice) -> R,
    {
        match self.process_id {
            None => Err(process::Error::NoSuchApp),
            Some(pid) => pid
                .kernel
                .process_map_or(Err(process::Error::NoSuchApp), pid, |_| {
                    // Safety: `kernel.process_map_or()` validates that
                    // the process still exists and its memory is still
                    // valid. In particular, `Process` tracks the "high water
                    // mark" of memory that the process has `allow`ed to the
                    // kernel. Because `Process` does not feature an API to
                    // move the "high water mark" down again, which would be
                    // called once a `ProcessBuffer` has been passed back into
                    // the kernel, a given `Process` implementation must assume
                    // that the memory described by a once-allowed
                    // `ProcessBuffer` is still in use, and thus will not
                    // permit the process to free any memory after it has
                    // been `allow`ed to the kernel once. This guarantees
                    // that the buffer is safe to convert into a slice
                    // here. For more information, refer to the
                    // comment and subsequent discussion on tock/tock#2632:
                    // https://github.com/tock/tock/pull/2632#issuecomment-869974365
                    Ok(fun(unsafe {
                        raw_processbuf_to_rwprocessslice(self.ptr, self.len)
                    }))
                }),
        }
    }
}

impl Default for ReadWriteProcessBuffer {
    fn default() -> Self {
        Self::const_default()
    }
}

/// Provides access to a ReadWriteProcessBuffer with a restricted lifetime.
/// This automatically dereferences into a ReadWriteProcessBuffer
pub struct ReadWriteProcessBufferRef<'a> {
    buf: ReadWriteProcessBuffer,
    _phantom: PhantomData<&'a ()>,
}

impl ReadWriteProcessBufferRef<'_> {
    /// Construct a new [`ReadWriteProcessBufferRef`] over a given pointer and
    /// length with a lifetime derived from the caller.
    ///
    /// # Safety requirements
    ///
    /// Refer to the safety requirements of
    /// [`ReadWriteProcessBuffer::new_external`]. The derived lifetime can
    /// help enforce the invariant that this incoming pointer may only
    /// be access for a certain duration.
    pub(crate) unsafe fn new(ptr: *mut u8, len: usize, process_id: ProcessId) -> Self {
        Self {
            buf: ReadWriteProcessBuffer::new(ptr, len, process_id),
            _phantom: PhantomData,
        }
    }
}

impl Deref for ReadWriteProcessBufferRef<'_> {
    type Target = ReadWriteProcessBuffer;
    fn deref(&self) -> &Self::Target {
        &self.buf
    }
}

/// A shareable region of userspace memory.
///
/// This trait can be used to gain read-write access to memory regions
/// wrapped in a ProcessBuffer type.
// We currently don't need any special functionality in the kernel for this
// type so we alias it as `ReadWriteProcessBuffer`.
pub type UserspaceReadableProcessBuffer = ReadWriteProcessBuffer;

/// Read-only wrapper around a [`Cell`]
///
/// This type is used in providing the [`ReadableProcessSlice`]. The
/// memory over which a [`ReadableProcessSlice`] exists must never be
/// written to by the kernel. However, it may either exist in flash
/// (read-only memory) or RAM (read-writeable memory). Consequently, a
/// process may `allow` memory overlapping with a
/// [`ReadOnlyProcessBuffer`] also simultaneously through a
/// [`ReadWriteProcessBuffer`]. Hence, the kernel can have two
/// references to the same memory, where one can lead to mutation of
/// the memory contents. Therefore, the kernel must use [`Cell`]s
/// around the bytes shared with userspace, to avoid violating Rust's
/// aliasing rules.
///
/// This read-only wrapper around a [`Cell`] only exposes methods
/// which are safe to call on a process-shared read-only `allow`
/// memory.
#[repr(transparent)]
pub struct ReadableProcessByte {
    cell: Cell<u8>,
}

impl ReadableProcessByte {
    #[inline]
    pub fn get(&self) -> u8 {
        self.cell.get()
    }
}

/// Readable and accessible slice of memory of a process buffer
///
///
/// The only way to obtain this struct is through a
/// [`ReadWriteProcessBuffer`] or [`ReadOnlyProcessBuffer`].
///
/// Slices provide a more convenient, traditional interface to process
/// memory. These slices are transient, as the underlying buffer must
/// be checked each time a slice is created. This is usually enforced
/// by the anonymous lifetime defined by the creation of the slice.
#[repr(transparent)]
pub struct ReadableProcessSlice {
    slice: [ReadableProcessByte],
}

fn cast_byte_slice_to_process_slice<'a>(
    byte_slice: &'a [ReadableProcessByte],
) -> &'a ReadableProcessSlice {
    // As ReadableProcessSlice is a transparent wrapper around its inner type,
    // [ReadableProcessByte], we can safely transmute a reference to the inner
    // type as a reference to the outer type with the same lifetime.
    unsafe { core::mem::transmute::<&[ReadableProcessByte], &ReadableProcessSlice>(byte_slice) }
}

// Allow a u8 slice to be viewed as a ReadableProcessSlice to allow client code
// to be authored once and accept either [u8] or ReadableProcessSlice.
impl<'a> From<&'a [u8]> for &'a ReadableProcessSlice {
    fn from(val: &'a [u8]) -> Self {
        // # Safety
        //
        // The layout of a [u8] and ReadableProcessSlice are guaranteed to be
        // the same. This also extends the lifetime of the buffer, so aliasing
        // rules are thus maintained properly.
        unsafe { core::mem::transmute(val) }
    }
}

// Allow a mutable u8 slice to be viewed as a ReadableProcessSlice to allow
// client code to be authored once and accept either [u8] or
// ReadableProcessSlice.
impl<'a> From<&'a mut [u8]> for &'a ReadableProcessSlice {
    fn from(val: &'a mut [u8]) -> Self {
        // # Safety
        //
        // The layout of a [u8] and ReadableProcessSlice are guaranteed to be
        // the same. This also extends the mutable lifetime of the buffer, so
        // aliasing rules are thus maintained properly.
        unsafe { core::mem::transmute(val) }
    }
}

impl ReadableProcessSlice {
    /// Copy the contents of a [`ReadableProcessSlice`] into a mutable
    /// slice reference.
    ///
    /// The length of `self` must be the same as `dest`. Subslicing
    /// can be used to obtain a slice of matching length.
    ///
    /// # Panics
    ///
    /// This function will panic if `self.len() != dest.len()`.
    pub fn copy_to_slice(&self, dest: &mut [u8]) {
        // The panic code path was put into a cold function to not
        // bloat the call site.
        #[inline(never)]
        #[cold]
        #[track_caller]
        fn len_mismatch_fail(dst_len: usize, src_len: usize) -> ! {
            panic!(
                "source slice length ({}) does not match destination slice length ({})",
                src_len, dst_len,
            );
        }

        if self.copy_to_slice_or_err(dest).is_err() {
            len_mismatch_fail(dest.len(), self.len());
        }
    }

    /// Copy the contents of a [`ReadableProcessSlice`] into a mutable
    /// slice reference.
    ///
    /// The length of `self` must be the same as `dest`. Subslicing
    /// can be used to obtain a slice of matching length.
    pub fn copy_to_slice_or_err(&self, dest: &mut [u8]) -> Result<(), ErrorCode> {
        // Method implemetation adopted from the
        // core::slice::copy_from_slice method implementation:
        // https://doc.rust-lang.org/src/core/slice/mod.rs.html#3034-3036

        if self.len() != dest.len() {
            Err(ErrorCode::SIZE)
        } else {
            // _If_ this turns out to not be efficiently optimized, it
            // should be possible to use a ptr::copy_nonoverlapping here
            // given we have exclusive mutable access to the destination
            // slice which will never be in process memory, and the layout
            // of &[ReadableProcessByte] is guaranteed to be compatible to
            // &[u8].
            for (i, b) in self.slice.iter().enumerate() {
                dest[i] = b.get();
            }
            Ok(())
        }
    }

    pub fn len(&self) -> usize {
        self.slice.len()
    }

    pub fn iter(&self) -> core::slice::Iter<'_, ReadableProcessByte> {
        self.slice.iter()
    }

    pub fn chunks(
        &self,
        chunk_size: usize,
    ) -> impl core::iter::Iterator<Item = &ReadableProcessSlice> {
        self.slice
            .chunks(chunk_size)
            .map(cast_byte_slice_to_process_slice)
    }

    pub fn get(&self, range: Range<usize>) -> Option<&ReadableProcessSlice> {
        if let Some(slice) = self.slice.get(range) {
            Some(cast_byte_slice_to_process_slice(slice))
        } else {
            None
        }
    }

    pub fn get_from(&self, range: RangeFrom<usize>) -> Option<&ReadableProcessSlice> {
        if let Some(slice) = self.slice.get(range) {
            Some(cast_byte_slice_to_process_slice(slice))
        } else {
            None
        }
    }

    pub fn get_to(&self, range: RangeTo<usize>) -> Option<&ReadableProcessSlice> {
        if let Some(slice) = self.slice.get(range) {
            Some(cast_byte_slice_to_process_slice(slice))
        } else {
            None
        }
    }
}

impl Index<Range<usize>> for ReadableProcessSlice {
    // Subslicing will still yield a ReadableProcessSlice reference
    type Output = Self;

    fn index(&self, idx: Range<usize>) -> &Self::Output {
        cast_byte_slice_to_process_slice(&self.slice[idx])
    }
}

impl Index<RangeTo<usize>> for ReadableProcessSlice {
    // Subslicing will still yield a ReadableProcessSlice reference
    type Output = Self;

    fn index(&self, idx: RangeTo<usize>) -> &Self::Output {
        &self[0..idx.end]
    }
}

impl Index<RangeFrom<usize>> for ReadableProcessSlice {
    // Subslicing will still yield a ReadableProcessSlice reference
    type Output = Self;

    fn index(&self, idx: RangeFrom<usize>) -> &Self::Output {
        &self[idx.start..self.len()]
    }
}

impl Index<usize> for ReadableProcessSlice {
    // Indexing into a ReadableProcessSlice must yield a
    // ReadableProcessByte, to limit the API surface of the wrapped
    // Cell to read-only operations
    type Output = ReadableProcessByte;

    fn index(&self, idx: usize) -> &Self::Output {
        // As ReadableProcessSlice is a transparent wrapper around its
        // inner type, [ReadableProcessByte], we can use the regular
        // slicing operator here with its usual semantics.
        &self.slice[idx]
    }
}

/// Read-writeable and accessible slice of memory of a process buffer
///
/// The only way to obtain this struct is through a
/// [`ReadWriteProcessBuffer`].
///
/// Slices provide a more convenient, traditional interface to process
/// memory. These slices are transient, as the underlying buffer must
/// be checked each time a slice is created. This is usually enforced
/// by the anonymous lifetime defined by the creation of the slice.
#[repr(transparent)]
pub struct WriteableProcessSlice {
    slice: [Cell<u8>],
}

fn cast_cell_slice_to_process_slice<'a>(cell_slice: &'a [Cell<u8>]) -> &'a WriteableProcessSlice {
    // # Safety
    //
    // As WriteableProcessSlice is a transparent wrapper around its inner type,
    // [Cell<u8>], we can safely transmute a reference to the inner type as the
    // outer type with the same lifetime.
    unsafe { core::mem::transmute(cell_slice) }
}

// Allow a mutable u8 slice to be viewed as a WritableProcessSlice to allow
// client code to be authored once and accept either [u8] or
// WriteableProcessSlice.
impl<'a> From<&'a mut [u8]> for &'a WriteableProcessSlice {
    fn from(val: &'a mut [u8]) -> Self {
        // # Safety
        //
        // The layout of a [u8] and WriteableProcessSlice are guaranteed to be
        // the same. This also extends the mutable lifetime of the buffer, so
        // aliasing rules are thus maintained properly.
        unsafe { core::mem::transmute(val) }
    }
}

impl WriteableProcessSlice {
    /// Copy the contents of a [`WriteableProcessSlice`] into a mutable
    /// slice reference.
    ///
    /// The length of `self` must be the same as `dest`. Subslicing
    /// can be used to obtain a slice of matching length.
    ///
    /// # Panics
    ///
    /// This function will panic if `self.len() != dest.len()`.
    pub fn copy_to_slice(&self, dest: &mut [u8]) {
        // The panic code path was put into a cold function to not
        // bloat the call site.
        #[inline(never)]
        #[cold]
        #[track_caller]
        fn len_mismatch_fail(dst_len: usize, src_len: usize) -> ! {
            panic!(
                "source slice length ({}) does not match destination slice length ({})",
                src_len, dst_len,
            );
        }

        if self.copy_to_slice_or_err(dest).is_err() {
            len_mismatch_fail(dest.len(), self.len());
        }
    }

    /// Copy the contents of a [`WriteableProcessSlice`] into a mutable
    /// slice reference.
    ///
    /// The length of `self` must be the same as `dest`. Subslicing
    /// can be used to obtain a slice of matching length.
    pub fn copy_to_slice_or_err(&self, dest: &mut [u8]) -> Result<(), ErrorCode> {
        // Method implemetation adopted from the
        // core::slice::copy_from_slice method implementation:
        // https://doc.rust-lang.org/src/core/slice/mod.rs.html#3034-3036

        if self.len() != dest.len() {
            Err(ErrorCode::SIZE)
        } else {
            // _If_ this turns out to not be efficiently optimized, it
            // should be possible to use a ptr::copy_nonoverlapping here
            // given we have exclusive mutable access to the destination
            // slice which will never be in process memory, and the layout
            // of &[Cell<u8>] is guaranteed to be compatible to &[u8].
            self.slice
                .iter()
                .zip(dest.iter_mut())
                .for_each(|(src, dst)| *dst = src.get());
            Ok(())
        }
    }

    /// Copy the contents of a slice of bytes into a [`WriteableProcessSlice`].
    ///
    /// The length of `src` must be the same as `self`. Subslicing can
    /// be used to obtain a slice of matching length.
    ///
    /// # Panics
    ///
    /// This function will panic if `src.len() != self.len()`.
    pub fn copy_from_slice(&self, src: &[u8]) {
        // Method implemetation adopted from the
        // core::slice::copy_from_slice method implementation:
        // https://doc.rust-lang.org/src/core/slice/mod.rs.html#3034-3036

        // The panic code path was put into a cold function to not
        // bloat the call site.
        #[inline(never)]
        #[cold]
        #[track_caller]
        fn len_mismatch_fail(dst_len: usize, src_len: usize) -> ! {
            panic!(
                "source slice length ({}) does not match destination slice length ({})",
                src_len, dst_len,
            );
        }

        if self.copy_from_slice_or_err(src).is_err() {
            len_mismatch_fail(self.len(), src.len());
        }
    }

    /// Copy the contents of a slice of bytes into a [`WriteableProcessSlice`].
    ///
    /// The length of `src` must be the same as `self`. Subslicing can
    /// be used to obtain a slice of matching length.
    pub fn copy_from_slice_or_err(&self, src: &[u8]) -> Result<(), ErrorCode> {
        // Method implemetation adopted from the
        // core::slice::copy_from_slice method implementation:
        // https://doc.rust-lang.org/src/core/slice/mod.rs.html#3034-3036

        if self.len() != src.len() {
            Err(ErrorCode::SIZE)
        } else {
            // _If_ this turns out to not be efficiently optimized, it
            // should be possible to use a ptr::copy_nonoverlapping here
            // given we have exclusive mutable access to the destination
            // slice which will never be in process memory, and the layout
            // of &[Cell<u8>] is guaranteed to be compatible to &[u8].
            src.iter()
                .zip(self.slice.iter())
                .for_each(|(src, dst)| dst.set(*src));
            Ok(())
        }
    }

    pub fn len(&self) -> usize {
        self.slice.len()
    }

    pub fn iter(&self) -> core::slice::Iter<'_, Cell<u8>> {
        self.slice.iter()
    }

    pub fn chunks(
        &self,
        chunk_size: usize,
    ) -> impl core::iter::Iterator<Item = &WriteableProcessSlice> {
        self.slice
            .chunks(chunk_size)
            .map(cast_cell_slice_to_process_slice)
    }

    pub fn get(&self, range: Range<usize>) -> Option<&WriteableProcessSlice> {
        if let Some(slice) = self.slice.get(range) {
            Some(cast_cell_slice_to_process_slice(slice))
        } else {
            None
        }
    }

    pub fn get_from(&self, range: RangeFrom<usize>) -> Option<&WriteableProcessSlice> {
        if let Some(slice) = self.slice.get(range) {
            Some(cast_cell_slice_to_process_slice(slice))
        } else {
            None
        }
    }

    pub fn get_to(&self, range: RangeTo<usize>) -> Option<&WriteableProcessSlice> {
        if let Some(slice) = self.slice.get(range) {
            Some(cast_cell_slice_to_process_slice(slice))
        } else {
            None
        }
    }
}

impl Index<Range<usize>> for WriteableProcessSlice {
    // Subslicing will still yield a WriteableProcessSlice reference.
    type Output = Self;

    fn index(&self, idx: Range<usize>) -> &Self::Output {
        cast_cell_slice_to_process_slice(&self.slice[idx])
    }
}

impl Index<RangeTo<usize>> for WriteableProcessSlice {
    // Subslicing will still yield a WriteableProcessSlice reference.
    type Output = Self;

    fn index(&self, idx: RangeTo<usize>) -> &Self::Output {
        &self[0..idx.end]
    }
}

impl Index<RangeFrom<usize>> for WriteableProcessSlice {
    // Subslicing will still yield a WriteableProcessSlice reference.
    type Output = Self;

    fn index(&self, idx: RangeFrom<usize>) -> &Self::Output {
        &self[idx.start..self.len()]
    }
}

impl Index<usize> for WriteableProcessSlice {
    // Indexing into a WriteableProcessSlice yields a Cell<u8>, as
    // mutating the memory contents is allowed.
    type Output = Cell<u8>;

    fn index(&self, idx: usize) -> &Self::Output {
        // As WriteableProcessSlice is a transparent wrapper around
        // its inner type, [Cell<u8>], we can use the regular slicing
        // operator here with its usual semantics.
        &self.slice[idx]
    }
}
