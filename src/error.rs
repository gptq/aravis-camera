//! 错误处理模块。
//!
//! 定义统一的 `CameraError` 枚举，封装所有可能的错误类型。

/// 相机操作结果类型别名。
pub type Result<T> = std::result::Result<T, CameraError>;

/// 相机操作错误。
#[derive(Debug, thiserror::Error)]
pub enum CameraError {
    /// Aravis / glib 底层错误。
    #[error("aravis error: {0}")]
    Aravis(#[from] aravis::glib::Error),

    /// 未找到相机。
    #[error("no camera found")]
    NoCameraFound,

    /// 相机未打开。
    #[error("camera is not open")]
    DeviceNotOpen,

    /// 获取帧超时。
    #[error("frame acquisition timed out")]
    Timeout,

    /// 不支持的像素格式。
    #[error("unsupported pixel format: {0}")]
    UnsupportedPixelFormat(String),

    /// Buffer 中无有效图像数据。
    #[error("buffer contains no valid image data")]
    InvalidBuffer,

    /// IO 错误。
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// 相机连接丢失，需要重连。
    #[error("camera connection lost: {0}")]
    ConnectionLost(String),

    /// 重连失败（超过最大重试次数）。
    #[error("reconnect failed after {0} attempts")]
    ReconnectFailed(usize),

    /// 参数值超出范围。
    #[error("parameter '{name}' value {value} out of range [{min}, {max}]")]
    ParameterOutOfRange {
        name: String,
        value: f64,
        min: f64,
        max: f64,
    },

    /// 采集超时。
    #[error("acquisition timed out after {0}s")]
    AcquisitionTimeout(u64),

    /// Aravis 通用错误 (非 glib::Error)。
    #[error("aravis: {0}")]
    AravisError(String),

    /// 通用错误（如图像处理、数据校验等）。
    #[error("{0}")]
    GenericError(String),
}
