//! 用于在内核中存储编译时配置选项的数据结构。
//!
//! 基于 `const` 对象的配置的基本原理是双重的。
//!
//! - 理论上，Cargo 特征可以用于基于boolean的配置。 但是，这些功能对于non-trivial的用例通常容易出错。
//!  首先，只要依赖关系需要某个特性（即使对于不需要该特性的其他依赖关系），它们就会全局启用。
//!  其次，由未启用的功能控制的代码编译器甚至没有进行类型检查，因此我们可能会由于重构代码
//! （如果在重构期间未测试这些特性）或不兼容的特性组合而导致特性损坏。
//!
//! - cargo特征只能包含bit。 另一方面，constant value可以包含任意类型，
//!   允许基于整数、字符串甚至更复杂的值进行配置。
//!
//! 使用类型化的“const”配置，所有代码路径都由编译器进行类型检查——即使是那些最终被禁用的
//! ——这大大降低了破坏一个特性或特性组合的风险，因为它们在测试中被禁用。
//!
//! 同时，在类型检查之后，编译器可以通过在整个代码中折叠常量来优化死代码，
//! 例如，在“if”块中使用的布尔条件原则上对生成的二进制文件的成本为零——就像而是使用了 Cargo 功能。
//! 对生成的 Tock 代码进行的一些简单实验已经在实践中证实了这一零成本。

/// 保存编译时配置选项的数据结构。
///
/// 要更改配置，请修改此文件末尾定义的“CONFIG”常量对象中的相关值。
pub(crate) struct Config {
    /// 内核是否应该将系统调用跟踪到调试输出。
    ///
    /// If enabled, the kernel will print a message in the debug output for each system call and
    /// upcall, with details including the application ID, and system call or upcall parameters.
    pub(crate) trace_syscalls: bool,

    /// Whether the kernel should show debugging output when loading processes.
    ///
    /// If enabled, the kernel will show from which addresses processes are loaded in flash and
    /// into which SRAM addresses. This can be useful to debug whether the kernel could
    /// successfully load processes, and whether the allocated SRAM is as expected.
    pub(crate) debug_load_processes: bool,

    /// Whether the kernel should output additional debug information on panics.
    ///
    /// If enabled, the kernel will include implementations of `Process::print_full_process()` and
    /// `Process::print_memory_map()` that display the process's state in a human-readable
    /// form.
    // This config option is intended to allow for smaller kernel builds (in
    // terms of code size) where printing code is removed from the kernel
    // binary. Ideally, the compiler would automatically remove
    // printing/debugging functions if they are never called, but due to
    // limitations in Rust (as of Sep 2021) that does not happen if the
    // functions are part of a trait (see
    // https://github.com/tock/tock/issues/2594).
    //
    // Attempts to separate the printing/debugging code from the Process trait
    // have only been moderately successful (see
    // https://github.com/tock/tock/pull/2826 and
    // https://github.com/tock/tock/pull/2759). Until a more complete solution
    // is identified, using configuration constants is the most effective
    // option.
    pub(crate) debug_panics: bool,
}

/// `Config` 的唯一实例，其中定义了编译时配置选项。 这些选项在内核 crate 中可用，可用于相关配置。
/// 值得注意的是，这是 Tock 内核中唯一允许使用 `#[cfg(x)]` 来配置基于 Cargo 功能的代码的位置。
pub(crate) const CONFIG: Config = Config {
    trace_syscalls: cfg!(feature = "trace_syscalls"),
    debug_load_processes: cfg!(feature = "debug_load_processes"),
    debug_panics: !cfg!(feature = "no_debug_panics"),
};
