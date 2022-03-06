//! Component通过一个简单的工厂方法接口扩展了 Tock 内核的功能。

/// 组件将 Tock OS 内核的特定外设和特定Capsule初始化封装在工厂方法中，
/// 从而减少重复代码并简化启动顺序。
///
/// `Component` trait 将内核扩展的所有初始化和配置封装在 `finalize()` 函数调用中。
/// `Output` 类型定义了该Component生成的类型。请注意，实例化Component不一定实例化底层的“Output”类型；
/// 相反，它通常在 `finalize()` 方法中实例化。如果实例化和初始化 `Output` 类型需要参数，
/// 这些应该在Component的 `new()` 函数中传递。
pub trait Component {
    /// 一种可选类型，用于指定Component设置输出对象所需的芯片或板特定静态内存。
    /// 这是 `static_init!()` 通常会设置的内存，但通用组件无法为依赖于芯片的
    /// 类型设置静态缓冲区，因此必须手动传入这些缓冲区，而 `StaticInput` 类型使这成为可能。
    type StaticInput;

    /// 这个Component的实现通过`finalize()`生成的类型（例如，Capsule、peripheral）。
    /// 这通常是一个静态引用（`&'static`）。
    type Output;

    /// 返回此Component实现的输出类型实例的工厂方法。 每个 Component 实例只能调用此工厂方法一次。
    /// 在引导序列中用于实例化和初始化 Tock 内核的一部分。
    /// 一些组件需要使用 `static_memory` 参数来允许板初始化代码传递对静态内存的引用，
    /// Component将使用该引用来设置输出类型对象。
    unsafe fn finalize(self, static_memory: Self::StaticInput) -> Self::Output;
}
