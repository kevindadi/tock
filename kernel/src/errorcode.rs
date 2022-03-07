//! Standard errors in Tock.

use core::convert::TryFrom;

/// Standard errors in Tock.
///
/// 与 [`Result<(), ErrorCode>`](crate::Result<(), ErrorCode>) 相比，它没有任何成功案例，
/// 因此更适合 Tock 2.0 系统调用接口，其中成功负载和错误不会被打包到同一个 32 位宽的寄存器中。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum ErrorCode {
    // 保留值，当no error/success应以与 ErrorCode 相同的数字表示形式编码时
    //
    // Ok(()) = 0,
    /// Generic failure condition
    FAIL = 1,
    /// 底层系统繁忙； 重试
    BUSY = 2,
    /// 请求的状态已设置
    ALREADY = 3,
    /// 组件断电
    OFF = 4,
    /// Reservation required before use 使用前需要初始化(预约)
    RESERVE = 5,
    /// 传递了一个无效的参数
    INVAL = 6,
    /// 传递的参数太大
    SIZE = 7,
    /// 被call取消的操作
    CANCEL = 8,
    /// 所需内存不可用
    NOMEM = 9,
    /// 不支持操作
    NOSUPPORT = 10,
    /// 设备不可用
    NODEVICE = 11,
    /// 物理设备未安装
    UNINSTALLED = 12,
    /// 数据包传输未确认
    NOACK = 13,
}

impl From<ErrorCode> for usize {
    fn from(err: ErrorCode) -> usize {
        err as usize
    }
}

impl TryFrom<Result<(), ErrorCode>> for ErrorCode {
    type Error = ();

    fn try_from(rc: Result<(), ErrorCode>) -> Result<Self, Self::Error> {
        match rc {
            Ok(()) => Err(()),
            Err(ErrorCode::FAIL) => Ok(ErrorCode::FAIL),
            Err(ErrorCode::BUSY) => Ok(ErrorCode::BUSY),
            Err(ErrorCode::ALREADY) => Ok(ErrorCode::ALREADY),
            Err(ErrorCode::OFF) => Ok(ErrorCode::OFF),
            Err(ErrorCode::RESERVE) => Ok(ErrorCode::RESERVE),
            Err(ErrorCode::INVAL) => Ok(ErrorCode::INVAL),
            Err(ErrorCode::SIZE) => Ok(ErrorCode::SIZE),
            Err(ErrorCode::CANCEL) => Ok(ErrorCode::CANCEL),
            Err(ErrorCode::NOMEM) => Ok(ErrorCode::NOMEM),
            Err(ErrorCode::NOSUPPORT) => Ok(ErrorCode::NOSUPPORT),
            Err(ErrorCode::NODEVICE) => Ok(ErrorCode::NODEVICE),
            Err(ErrorCode::UNINSTALLED) => Ok(ErrorCode::UNINSTALLED),
            Err(ErrorCode::NOACK) => Ok(ErrorCode::NOACK),
        }
    }
}

impl From<ErrorCode> for Result<(), ErrorCode> {
    fn from(ec: ErrorCode) -> Self {
        match ec {
            ErrorCode::FAIL => Err(ErrorCode::FAIL),
            ErrorCode::BUSY => Err(ErrorCode::BUSY),
            ErrorCode::ALREADY => Err(ErrorCode::ALREADY),
            ErrorCode::OFF => Err(ErrorCode::OFF),
            ErrorCode::RESERVE => Err(ErrorCode::RESERVE),
            ErrorCode::INVAL => Err(ErrorCode::INVAL),
            ErrorCode::SIZE => Err(ErrorCode::SIZE),
            ErrorCode::CANCEL => Err(ErrorCode::CANCEL),
            ErrorCode::NOMEM => Err(ErrorCode::NOMEM),
            ErrorCode::NOSUPPORT => Err(ErrorCode::NOSUPPORT),
            ErrorCode::NODEVICE => Err(ErrorCode::NODEVICE),
            ErrorCode::UNINSTALLED => Err(ErrorCode::UNINSTALLED),
            ErrorCode::NOACK => Err(ErrorCode::NOACK),
        }
    }
}

/// 将 `Result<(), ErrorCode>` 转换为用户空间的 StatusCode (usize)。
///
/// StatusCode 是一个有用的“伪类型”（在 Tock 中没有称为 StatusCode 的实际 Rust 类型），
/// 原因有三个：
///
/// 1. 它可以用一个单一的`usize`来表示。
///    这使得 StatusCode 可以轻松地通过内核和用户空间之间的系统调用接口传递。
///
/// 2. 它扩展了 ErrorCode，但保留了与 ErrorCode 相同的错误到数字的映射。
///    例如，在 StatusCode 和 ErrorCode 中，`SIZE` 错误总是表示为 7。
///
/// 3. 它可以对成功值进行编码，而 ErrorCode 只能对错误进行编码。
///    ErrorCode 中的数字 0 是保留的，用于 StatusCode 中的“SUCCESS”。
///
/// 这个帮助函数将成功/错误类型的 Tock 和 Rust 约定转换为 StatusCode。
/// StatusCode 表示为足以通过 upcall 发送到用户空间的使用大小。
/// 内核和用户空间之间的这种转换和可移植性的关键是只表示错误的`ErrorCode`被分配了固定值，
/// 但不使用约定的值0。 这允许我们在 ReturnCode 中使用 0 作为成功。
pub fn into_statuscode(r: Result<(), ErrorCode>) -> usize {
    match r {
        Ok(()) => 0,
        Err(e) => e as usize,
    }
}
