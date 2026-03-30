//! # aravis-camera
//!
//! 基于 Aravis 的 GigE Vision 工业相机 Rust 库。
//!
//! 通过开放的 GigE Vision / GenICam 标准协议控制工业相机，
//! 无需厂商私有 SDK。兼容海康、大华等符合 GigE Vision V2.0 标准的相机。
//!
//! ## 核心特性
//!
//! - **零 SDK 依赖**：仅需 `brew install aravis`，无需厂商闭源 SDK
//! - **原生 arm64**：Apple Silicon 原生支持，无需 Rosetta 2
//! - **自动 Bayer 去马赛克**：[`Frame::to_rgb()`] 自动检测 Bayer 格式并转 RGB
//! - **RAII 资源管理**：[`CameraGuard`] 通过 `Drop` 自动关闭相机
//! - **线程安全**：`AtomicBool` + `Mutex` 中毒恢复
//! - **工业级稳定性**：可配置重试、指数退避、自动重连、健康检查
//! - **多相机并发**：[`MultiCamera`] 支持并行采集，单台 panic 不影响其他
//!
//! ## 快速开始：采集彩色图片
//!
//! ```ignore
//! use aravis_camera::{GigECamera, Result};
//!
//! fn main() -> Result<()> {
//!     let cam = GigECamera::new(Some("192.168.2.91"))?;
//!     cam.set_exposure_time(10000.0)?; // 10ms
//!
//!     let guard = cam.open_guard()?;
//!     let frame = guard.get_frame()?;
//!
//!     // 自动 Bayer 去马赛克 → RGB24
//!     let rgb = frame.to_rgb()?;
//!     println!("{}x{}, 格式={}, RGB数据={}字节",
//!         frame.width, frame.height,
//!         frame.pixel_format,  // "BayerGR8"
//!         rgb.len());
//!
//!     Ok(())
//! }
//! ```
//!
//! ## 多相机使用
//!
//! ```ignore
//! use aravis_camera::{MultiCamera, Result};
//!
//! fn main() -> Result<()> {
//!     let multi = MultiCamera::new(&["192.168.2.91", "192.168.2.145"])?;
//!     multi.open_all()?;
//!
//!     let frames = multi.get_all_frames()?;
//!     for (id, frame) in &frames {
//!         let rgb = frame.to_rgb()?;
//!         println!("{}: {}x{} {} → {} RGB bytes",
//!             id, frame.width, frame.height,
//!             frame.pixel_format, rgb.len());
//!     }
//!
//!     Ok(())
//! }
//! ```
//!
//! ## 模块说明
//!
//! | 模块 | 说明 |
//! |------|------|
//! | [`camera`] | 相机控制器：曝光、增益、像素格式、ROI、GigE 网络 |
//! | [`frame`] | 图像帧：`to_rgb()` 自动 Bayer 去马赛克 |
//! | [`demosaic`] | Bayer 去马赛克：双线性插值，支持 8/10/12/16-bit |
//! | [`discovery`] | 相机搜索、IP 设置、ForceIP |
//! | [`multi_camera`] | 多相机并发管理 |
//! | [`error`] | 统一错误处理 |

pub mod camera;
pub mod demosaic;
pub mod discovery;
pub mod error;
pub mod frame;
pub mod multi_camera;

// 公开核心类型
pub use camera::{CameraGuard, CameraStats, GigECamera};
pub use demosaic::BayerPattern;
pub use discovery::{
    discover_cameras, force_ip, get_all_camera_ids, get_host_ip_by_target_ip, ip_str_to_u32,
    u32_to_ip_str, CameraInfo,
};
pub use error::{CameraError, Result};
pub use frame::Frame;
pub use multi_camera::MultiCamera;
