//! 核心相机控制模块。
//!
//! 提供 `GigECamera` 结构体，封装 Aravis `Camera` 对象，
//! 实现相机初始化、参数设置、图像采集等功能。
//!
//! ## 工业稳定性设计
//!
//! - **Mutex 中毒恢复**：`lock_camera()` 使用 `unwrap_or_else` 而非 `unwrap`
//! - **原子状态**：`is_open` 使用 `AtomicBool`，线程安全
//! - **全局初始化**：Aravis 通过 `OnceLock` 确保只初始化一次
//! - **可配置重试**：`max_retries` 控制重试次数
//! - **自动重连**：`reset()` 会重建 Camera 对象并轮询等待
//! - **参数校验**：曝光/增益等设置前自动 clamp 到合法范围
//! - **健康检查**：`health_check()` 验证相机是否仍在线
//! - **运行统计**：`CameraStats` 记录采集成功/失败/重置次数

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use aravis::prelude::*;

use crate::error::{CameraError, Result};
use crate::frame::Frame;

/// Aravis 全局单例。只初始化一次，handle 持久存储。
static ARAVIS_HANDLE: OnceLock<std::result::Result<aravis::Aravis, String>> = OnceLock::new();

/// 确保 Aravis 库已初始化（线程安全，只执行一次）。
///
/// 返回对全局 `Aravis` handle 的引用，用于 `get_device_list()` 等需要 handle 的操作。
pub(crate) fn ensure_aravis_initialized() -> Result<&'static aravis::Aravis> {
    let init_result = ARAVIS_HANDLE.get_or_init(|| match aravis::Aravis::initialize() {
        Ok(aravis) => {
            log::info!("Aravis library initialized");
            Ok(aravis)
        }
        Err(error) => Err(error.to_string()),
    });
    init_result.as_ref().map_err(|message| {
        CameraError::GenericError(format!("Aravis initialization failed: {message}"))
    })
}

/// 相机运行统计信息。
#[derive(Debug, Clone, Default)]
pub struct CameraStats {
    /// 采集成功的总帧数。
    pub total_frames: u64,
    /// 采集失败的总次数。
    pub failed_frames: u64,
    /// 设备重置的总次数。
    pub total_resets: u64,
    /// 重连的总次数。
    pub total_reconnects: u64,
    /// 最后一次成功采集的时间。
    pub last_frame_time: Option<Instant>,
    /// 相机创建时间。
    pub created_at: Option<Instant>,
}

/// GigE 工业相机控制器。
///
/// 封装 Aravis `Camera` 对象，提供安全的 Rust API。
/// 设计为工业 7×24 长期稳定运行。
pub struct GigECamera {
    /// Aravis Camera 对象，Mutex 保护并发访问。
    inner: Mutex<aravis::Camera>,
    /// 相机是否已打开（开始采集），原子操作保证线程安全。
    is_open: AtomicBool,
    /// 创建相机时使用的标识符（IP 或设备 ID），用于重连。
    device_id: Option<String>,
    /// 帧采集超时（微秒）。
    timeout_us: u64,
    /// 最大重试次数。
    max_retries: usize,
    /// 重试间隔基数（毫秒），实际间隔 = base * 2^attempt。
    retry_delay_base_ms: u64,
    /// 运行统计。
    stats: Mutex<CameraStats>,
    /// Stream 持续采集对象 (用于高帧率连续采集)。
    stream: Mutex<Option<aravis::Stream>>,
}

// Aravis Camera 内部是线程安全的（基于 GObject）
unsafe impl Send for GigECamera {}
unsafe impl Sync for GigECamera {}

impl GigECamera {
    // ────────────────────────────────────────────────────────────
    //  构造与生命周期
    // ────────────────────────────────────────────────────────────

    /// 创建相机实例。
    ///
    /// # Arguments
    /// * `id` - 相机标识符（IP、`"Vendor-Model-Serial"` 格式、或 `None` 使用第一台相机）
    ///
    /// # Example
    /// ```ignore
    /// let cam = GigECamera::new(Some("192.168.1.100"))?;
    /// let cam = GigECamera::new(None)?; // 使用第一台相机
    /// ```
    pub fn new(id: Option<&str>) -> Result<Self> {
        ensure_aravis_initialized()?;

        let camera = aravis::Camera::new(id)?;
        let cam_id = id.unwrap_or("(first available)");
        log::info!("camera created: {}", cam_id);

        Ok(Self {
            inner: Mutex::new(camera),
            is_open: AtomicBool::new(false),
            device_id: id.map(String::from),
            timeout_us: 40_000_000, // 40s 默认超时
            max_retries: 3,
            retry_delay_base_ms: 500,
            stats: Mutex::new(CameraStats {
                created_at: Some(Instant::now()),
                ..Default::default()
            }),
            stream: Mutex::new(None),
        })
    }

    /// 设置帧采集超时时间。
    pub fn set_timeout(&mut self, timeout: Duration) {
        self.timeout_us = timeout.as_micros() as u64;
    }

    /// 设置最大重试次数（默认 3）。
    pub fn set_max_retries(&mut self, retries: usize) {
        self.max_retries = retries;
    }

    /// 设置重试间隔基数（默认 500ms，实际间隔 = base × 2^attempt）。
    pub fn set_retry_delay_base(&mut self, base_ms: u64) {
        self.retry_delay_base_ms = base_ms;
    }

    /// 打开相机：配置为连续采集模式。
    ///
    /// 配置步骤：
    /// 1. 关闭触发模式（使用连续/自由采集）
    ///
    /// 注意: GigE 网络参数（包大小、包间延时）由调用者负责配置，
    /// 此方法不再硬编码任何网络参数。
    pub fn open(&self) -> Result<()> {
        let cam = self.lock_camera();

        // 设置为连续采集模式（关闭触发，让 acquisition() 直接获取帧）
        for selector in &["FrameStart", "FrameBurstStart", "AcquisitionStart"] {
            if cam.set_string("TriggerSelector", selector).is_ok() {
                let _ = cam.set_string("TriggerMode", "Off");
            }
        }
        log::debug!("trigger mode disabled for continuous acquisition");

        drop(cam);

        self.is_open.store(true, Ordering::Release);
        log::info!("camera opened, ready for acquisition (continuous mode)");
        Ok(())
    }

    /// 关闭相机：停止采集并清理 Stream。
    ///
    /// 如果使用的是 Stream 模式，会停止采集并释放 Stream。
    pub fn close(&self) -> Result<()> {
        if !self.is_open.load(Ordering::Acquire) {
            return Ok(());
        }

        // 关闭 Stream
        self.close_stream();

        self.is_open.store(false, Ordering::Release);
        log::info!("camera closed");
        Ok(())
    }

    /// 创建 RAII 守卫，自动管理相机打开/关闭。
    ///
    /// # Example
    /// ```ignore
    /// let cam = GigECamera::new(None)?;
    /// {
    ///     let guard = cam.open_guard()?;
    ///     let frame = guard.get_frame()?;
    /// } // guard drop → 自动关闭
    /// ```
    pub fn open_guard(&self) -> Result<CameraGuard<'_>> {
        self.open()?;
        Ok(CameraGuard { camera: self })
    }

    /// 相机是否已打开。
    pub fn is_open(&self) -> bool {
        self.is_open.load(Ordering::Acquire)
    }

    // ────────────────────────────────────────────────────────────
    //  图像采集
    // ────────────────────────────────────────────────────────────

    /// 采集一帧图像。
    ///
    /// 使用 Aravis 一站式 `acquisition()` 方法：
    /// 内部自动 create_stream → push_buffer → start_acquisition
    /// → software_trigger → pop_buffer → stop_acquisition。
    ///
    /// 每次调用独立完成一次完整采集流程，稳定可靠。
    /// 返回的 `Frame` 中 `pixel_format` 为可读名（如 `BayerGR8`）。
    pub fn get_frame(&self) -> Result<Frame> {
        self.ensure_open()?;

        let cam = self.lock_camera();

        // 读取像素格式可读名（用于 Bayer 检测等）
        let fmt_name = cam
            .pixel_format_as_string()
            .map(|s| s.to_string())
            .unwrap_or_default();

        // 使用 Aravis 一站式采集（自动管理流生命周期）
        let buffer = cam.acquisition(self.timeout_us)?;

        drop(cam);

        let frame = Frame::from_buffer(&buffer, &fmt_name)?;

        // 更新统计
        let mut stats = self.lock_stats();
        stats.total_frames += 1;
        stats.last_frame_time = Some(Instant::now());

        Ok(frame)
    }

    /// 带自动重试和指数退避的帧采集。
    ///
    /// 重试策略：
    /// - 第 1 次失败：等待 base_ms 后重试
    /// - 第 2 次失败：等待 base_ms×2 后重置相机并重试
    /// - 第 N 次失败：等待 base_ms×2^(N-1) 后重置并重试
    pub fn robust_get_frame(&self) -> Result<Frame> {
        for attempt in 0..=self.max_retries {
            match self.get_frame() {
                Ok(frame) => return Ok(frame),
                Err(e) if attempt < self.max_retries => {
                    // 更新失败统计
                    self.lock_stats().failed_frames += 1;

                    let delay_ms = self.retry_delay_base_ms * (1u64 << attempt.min(6));
                    log::warn!(
                        "get_frame failed (attempt {}/{}): {}, retrying in {}ms...",
                        attempt + 1,
                        self.max_retries,
                        e,
                        delay_ms
                    );
                    std::thread::sleep(Duration::from_millis(delay_ms));

                    // 第 2 次及以后尝试重置相机
                    if attempt > 0 {
                        if let Err(reset_err) = self.reset() {
                            log::error!("reset failed during retry: {}", reset_err);
                        }
                    }
                }
                Err(e) => {
                    self.lock_stats().failed_frames += 1;
                    log::error!(
                        "get_frame failed after {} attempts: {}",
                        self.max_retries + 1,
                        e
                    );
                    return Err(e);
                }
            }
        }
        Err(CameraError::ReconnectFailed(self.max_retries))
    }

    /// 在已打开的 Stream 上执行一次软触发并等待对应帧返回。
    ///
    /// 适合工业现场的 software trigger 模式：
    /// Stream 在启动阶段保持打开，触发时仅发送一次 software_trigger，
    /// 随后从现有 Stream 弹出一帧，避免一站式 acquisition() 在某些相机上的
    /// 超长首帧延迟和 ROI 失配问题。
    pub fn trigger_stream_frame(&self) -> Result<Frame> {
        self.ensure_open()?;
        if !self.is_streaming() {
            return Err(CameraError::DeviceNotOpen);
        }

        self.software_trigger()?;
        self.stream_pop_frame()
    }

    /// 带自动重试的 software trigger + Stream pop。
    ///
    /// 每次重试都会重新发送 software trigger，而不是复用上一次触发，
    /// 这样在超时或不完整帧后，下一次尝试仍然对应新的曝光周期。
    pub fn robust_trigger_stream_frame(&self) -> Result<Frame> {
        for attempt in 0..=self.max_retries {
            match self.trigger_stream_frame() {
                Ok(frame) => return Ok(frame),
                Err(e) if attempt < self.max_retries => {
                    self.lock_stats().failed_frames += 1;

                    let delay_ms = self.retry_delay_base_ms * (1u64 << attempt.min(6));
                    log::warn!(
                        "trigger_stream_frame failed (attempt {}/{}): {}, retrying in {}ms...",
                        attempt + 1,
                        self.max_retries,
                        e,
                        delay_ms
                    );
                    std::thread::sleep(Duration::from_millis(delay_ms));
                }
                Err(e) => {
                    self.lock_stats().failed_frames += 1;
                    log::error!(
                        "trigger_stream_frame failed after {} attempts: {}",
                        self.max_retries + 1,
                        e
                    );
                    return Err(e);
                }
            }
        }

        Err(CameraError::ReconnectFailed(self.max_retries))
    }

    /// 重置相机设备。
    ///
    /// 1. 停止采集
    /// 2. 发送 DeviceReset 命令
    /// 3. 轮询等待相机就绪（创建新 Camera 对象）
    /// 4. 重新打开
    pub fn reset(&self) -> Result<()> {
        log::info!("resetting camera...");

        // 先关闭
        let _ = self.close();

        // 发送重置命令（可能失败，因为连接可能已断）
        {
            let cam = self.lock_camera();
            if let Err(e) = cam.execute_command("DeviceReset") {
                log::warn!("DeviceReset command failed (expected): {}", e);
            }
        }

        // 更新统计
        self.lock_stats().total_resets += 1;

        // 轮询等待相机就绪，而非硬编码睡眠
        let device_id = self.device_id.as_deref();
        let mut connected = false;

        for attempt in 1..=self.max_retries * 2 {
            std::thread::sleep(Duration::from_secs(2));
            log::info!("waiting for camera to restart... (attempt {})", attempt);

            match aravis::Camera::new(device_id) {
                Ok(new_camera) => {
                    // 成功重建连接，替换底层对象
                    *self.lock_camera() = new_camera;
                    connected = true;
                    log::info!("camera reconnected after reset");
                    break;
                }
                Err(e) => {
                    log::debug!("camera not yet ready (attempt {}): {}", attempt, e);
                }
            }
        }

        if !connected {
            return Err(CameraError::ReconnectFailed(self.max_retries * 2));
        }

        // 重新打开
        self.open()?;
        log::info!("camera reset complete");
        Ok(())
    }

    /// 尝试重新连接相机（网线重插、相机重启后调用）。
    ///
    /// 与 `reset()` 不同，此方法不发送 DeviceReset 命令，
    /// 直接尝试创建新的 Camera 连接。
    pub fn reconnect(&self) -> Result<()> {
        log::info!("reconnecting to camera...");
        let _ = self.close();

        self.lock_stats().total_reconnects += 1;

        let device_id = self.device_id.as_deref();
        let new_camera = aravis::Camera::new(device_id)
            .map_err(|e| CameraError::ConnectionLost(format!("reconnect failed: {e}")))?;

        *self.lock_camera() = new_camera;
        self.open()?;
        log::info!("camera reconnected");
        Ok(())
    }

    /// 检查相机是否仍然在线（不影响采集状态）。
    ///
    /// 通过读取 `DeviceModelName` 节点来验证通信是否正常。
    /// 适用于定期心跳检查。
    pub fn health_check(&self) -> Result<()> {
        let cam = self.lock_camera();
        cam.model_name()?;
        Ok(())
    }

    /// 获取运行统计信息的拷贝。
    pub fn stats(&self) -> CameraStats {
        self.lock_stats().clone()
    }

    /// 获取设备标识符（创建时传入的 ID）。
    pub fn device_id(&self) -> Option<&str> {
        self.device_id.as_deref()
    }

    // ────────────────────────────────────────────────────────────
    //  曝光控制
    // ────────────────────────────────────────────────────────────

    /// 获取当前曝光时间（微秒）。
    pub fn exposure_time(&self) -> Result<f64> {
        let cam = self.lock_camera();
        Ok(cam.exposure_time()?)
    }

    /// 设置手动曝光时间（微秒）。
    ///
    /// 自动关闭自动曝光，并将值 clamp 到相机支持的范围。
    pub fn set_exposure_time(&self, us: f64) -> Result<()> {
        let cam = self.lock_camera();
        cam.set_exposure_time_auto(aravis::Auto::Off)?;

        // 参数校验：clamp 到合法范围
        let (min, max) = cam.exposure_time_bounds()?;
        let clamped = us.clamp(min, max);
        if (clamped - us).abs() > f64::EPSILON {
            log::warn!(
                "exposure time {:.1} us clamped to [{:.1}, {:.1}]",
                us,
                min,
                max
            );
        }

        cam.set_exposure_time(clamped)?;
        log::debug!("exposure time set to {:.1} us", clamped);
        Ok(())
    }

    /// 设置手动曝光时间（秒）。
    pub fn set_exposure_time_by_second(&self, seconds: f64) -> Result<()> {
        self.set_exposure_time(seconds * 1_000_000.0)
    }

    /// 获取曝光时间（秒）。
    pub fn exposure_time_by_second(&self) -> Result<f64> {
        Ok(self.exposure_time()? * 1e-6)
    }

    /// 设置自动曝光模式。
    ///
    /// # Arguments
    /// * `mode` - `aravis::Auto::Off` / `aravis::Auto::Once` / `aravis::Auto::Continuous`
    pub fn set_exposure_auto(&self, mode: aravis::Auto) -> Result<()> {
        let cam = self.lock_camera();
        cam.set_exposure_time_auto(mode)?;
        log::debug!("exposure auto mode set to {:?}", mode);
        Ok(())
    }

    /// 获取曝光时间范围 (min, max)，单位微秒。
    pub fn exposure_time_bounds(&self) -> Result<(f64, f64)> {
        let cam = self.lock_camera();
        Ok(cam.exposure_time_bounds()?)
    }

    // ────────────────────────────────────────────────────────────
    //  增益控制
    // ────────────────────────────────────────────────────────────

    /// 获取当前增益值。
    pub fn gain(&self) -> Result<f64> {
        let cam = self.lock_camera();
        Ok(cam.gain()?)
    }

    /// 设置增益值。
    ///
    /// 自动关闭自动增益，并将值 clamp 到合法范围。
    pub fn set_gain(&self, gain: f64) -> Result<()> {
        let cam = self.lock_camera();
        cam.set_gain_auto(aravis::Auto::Off)?;

        let (min, max) = cam.gain_bounds()?;
        let clamped = gain.clamp(min, max);
        if (clamped - gain).abs() > f64::EPSILON {
            log::warn!("gain {} clamped to [{}, {}]", gain, min, max);
        }

        cam.set_gain(clamped)?;
        log::debug!("gain set to {}", clamped);
        Ok(())
    }

    /// 设置自动增益模式。
    pub fn set_gain_auto(&self, mode: aravis::Auto) -> Result<()> {
        let cam = self.lock_camera();
        cam.set_gain_auto(mode)?;
        Ok(())
    }

    /// 获取增益范围 (min, max)。
    pub fn gain_bounds(&self) -> Result<(f64, f64)> {
        let cam = self.lock_camera();
        Ok(cam.gain_bounds()?)
    }

    // ────────────────────────────────────────────────────────────
    //  像素格式
    // ────────────────────────────────────────────────────────────

    /// 获取当前像素格式字符串。
    pub fn pixel_format(&self) -> Result<String> {
        let cam = self.lock_camera();
        Ok(cam.pixel_format_as_string()?.to_string())
    }

    /// 通过字符串名称设置像素格式。
    ///
    /// # Example
    /// ```ignore
    /// cam.set_pixel_format("RGB8Packed")?;
    /// cam.set_pixel_format("Mono8")?;
    /// cam.set_pixel_format("BayerRG8")?;
    /// ```
    pub fn set_pixel_format(&self, format: &str) -> Result<()> {
        let cam = self.lock_camera();
        cam.set_pixel_format_from_string(format)?;
        log::debug!("pixel format set to {}", format);
        Ok(())
    }

    /// 获取相机支持的所有像素格式。
    pub fn available_pixel_formats(&self) -> Result<Vec<String>> {
        let cam = self.lock_camera();
        let formats = cam.dup_available_pixel_formats_as_strings()?;
        Ok(formats.iter().map(|s| s.to_string()).collect())
    }

    /// 快捷设置 RGB8 格式。
    pub fn set_rgb(&self) -> Result<()> {
        let rgb_names = ["RGB8Packed", "RGB8", "BGR8"];
        let available = self.available_pixel_formats()?;

        for name in &rgb_names {
            if available.iter().any(|f| f == name) {
                return self.set_pixel_format(name);
            }
        }

        Err(CameraError::UnsupportedPixelFormat(
            "no RGB8 format available".to_string(),
        ))
    }

    /// 快捷设置 Bayer RAW 格式。
    ///
    /// # Arguments
    /// * `bit` - 位深（8, 10, 12, 16）
    pub fn set_raw(&self, bit: u32) -> Result<()> {
        let patterns = ["BayerGB", "BayerGR", "BayerRG", "BayerBG"];
        let available = self.available_pixel_formats()?;

        for pattern in &patterns {
            let name = format!("{pattern}{bit}");
            if available.iter().any(|f| f == &name) {
                return self.set_pixel_format(&name);
            }
            // 尝试 Packed 版本
            if !bit.is_multiple_of(8) {
                let packed_name = format!("{pattern}{bit}Packed");
                if available.iter().any(|f| f == &packed_name) {
                    return self.set_pixel_format(&packed_name);
                }
            }
        }

        Err(CameraError::UnsupportedPixelFormat(format!(
            "no Bayer {bit}bit format available"
        )))
    }

    // ────────────────────────────────────────────────────────────
    //  分辨率 / ROI
    // ────────────────────────────────────────────────────────────

    /// 获取当前 ROI (x, y, width, height)。
    pub fn region(&self) -> Result<(i32, i32, i32, i32)> {
        let cam = self.lock_camera();
        Ok(cam.region()?)
    }

    /// 设置 ROI 区域。
    pub fn set_region(&self, x: i32, y: i32, width: i32, height: i32) -> Result<()> {
        let cam = self.lock_camera();
        cam.set_region(x, y, width, height)?;
        log::debug!("region set to ({}, {}, {}x{})", x, y, width, height);
        Ok(())
    }

    /// 获取传感器全尺寸 (width, height)。
    pub fn sensor_size(&self) -> Result<(i32, i32)> {
        let cam = self.lock_camera();
        Ok(cam.sensor_size()?)
    }

    /// 设置 binning。
    pub fn set_binning(&self, dx: i32, dy: i32) -> Result<()> {
        let cam = self.lock_camera();
        cam.set_binning(dx, dy)?;
        log::debug!("binning set to ({}, {})", dx, dy);
        Ok(())
    }

    /// 获取当前 binning (dx, dy)。
    pub fn binning(&self) -> Result<(i32, i32)> {
        let cam = self.lock_camera();
        Ok(cam.binning()?)
    }

    // ────────────────────────────────────────────────────────────
    //  GigE 网络参数
    //
    //  注意: aravis 底层某些 GigE 方法含有 C 级别的 assert，
    //  在特定相机型号上可能 panic。所有 GigE 方法都使用
    //  catch_unwind 防护，确保不会崩溃整个进程。
    // ────────────────────────────────────────────────────────────

    /// 自动协商最优网络包大小。
    ///
    /// 仅对 GigE 相机有效。如果底层 aravis 断言失败，
    /// 返回错误而非 panic。
    pub fn gv_auto_packet_size(&self) -> Result<()> {
        let cam = self.lock_camera();
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| cam.gv_auto_packet_size())) {
            Ok(Ok(())) => {
                log::debug!("GigE auto packet size negotiated");
                Ok(())
            }
            Ok(Err(e)) => Err(CameraError::Aravis(e)),
            Err(_) => {
                log::warn!("gv_auto_packet_size: aravis internal assertion failed, skipping");
                Err(CameraError::ConnectionLost(
                    "gv_auto_packet_size internal assertion failed".to_string(),
                ))
            }
        }
    }

    /// 设置 GigE 包延时（纳秒）。
    ///
    /// 防止多相机同时采集丢包。
    pub fn gv_set_packet_delay(&self, delay_ns: i64) -> Result<()> {
        let cam = self.lock_camera();
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            cam.gv_set_packet_delay(delay_ns)
        })) {
            Ok(Ok(())) => {
                log::debug!("GigE packet delay set to {} ns", delay_ns);
                Ok(())
            }
            Ok(Err(e)) => Err(CameraError::Aravis(e)),
            Err(_) => {
                log::warn!("gv_set_packet_delay: aravis internal assertion failed");
                Err(CameraError::ConnectionLost(
                    "gv_set_packet_delay internal assertion failed".to_string(),
                ))
            }
        }
    }

    /// 获取当前 GigE 包延时。
    pub fn gv_packet_delay(&self) -> Result<i64> {
        let cam = self.lock_camera();
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| cam.gv_get_packet_delay())) {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(e)) => Err(CameraError::Aravis(e)),
            Err(_) => {
                log::warn!("gv_packet_delay: aravis internal assertion failed");
                Err(CameraError::ConnectionLost(
                    "gv_packet_delay internal assertion failed".to_string(),
                ))
            }
        }
    }

    /// 设置 GigE 包大小。
    pub fn gv_set_packet_size(&self, size: i32) -> Result<()> {
        let cam = self.lock_camera();
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            cam.gv_set_packet_size(size)
        })) {
            Ok(Ok(())) => {
                log::debug!("GigE packet size set to {}", size);
                Ok(())
            }
            Ok(Err(e)) => Err(CameraError::Aravis(e)),
            Err(_) => {
                log::warn!("gv_set_packet_size: aravis internal assertion failed");
                Err(CameraError::ConnectionLost(
                    "gv_set_packet_size internal assertion failed".to_string(),
                ))
            }
        }
    }

    /// 获取当前 GigE 包大小。
    pub fn gv_packet_size(&self) -> Result<u32> {
        let cam = self.lock_camera();
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| cam.gv_get_packet_size())) {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(e)) => Err(CameraError::Aravis(e)),
            Err(_) => {
                log::warn!("gv_packet_size: aravis internal assertion failed");
                Err(CameraError::ConnectionLost(
                    "gv_packet_size internal assertion failed".to_string(),
                ))
            }
        }
    }

    /// 是否为 GigE 设备。
    pub fn is_gv_device(&self) -> bool {
        let cam = self.lock_camera();
        cam.is_gv_device()
    }

    // ────────────────────────────────────────────────────────────
    //  帧率
    // ────────────────────────────────────────────────────────────

    /// 获取当前帧率。
    pub fn frame_rate(&self) -> Result<f64> {
        let cam = self.lock_camera();
        Ok(cam.frame_rate()?)
    }

    /// 设置帧率。
    pub fn set_frame_rate(&self, fps: f64) -> Result<()> {
        let cam = self.lock_camera();
        cam.set_frame_rate(fps)?;
        log::debug!("frame rate set to {} fps", fps);
        Ok(())
    }

    /// 启用/禁用帧率限制。
    pub fn set_frame_rate_enable(&self, enable: bool) -> Result<()> {
        let cam = self.lock_camera();
        cam.set_frame_rate_enable(enable)?;
        Ok(())
    }

    // ────────────────────────────────────────────────────────────
    //  触发控制
    // ────────────────────────────────────────────────────────────

    /// 发送软触发命令。
    pub fn software_trigger(&self) -> Result<()> {
        let cam = self.lock_camera();
        cam.software_trigger()?;
        Ok(())
    }

    /// 设置触发源。
    pub fn set_trigger_source(&self, source: &str) -> Result<()> {
        let cam = self.lock_camera();
        cam.set_trigger_source(source)?;
        log::debug!("trigger source set to {}", source);
        Ok(())
    }

    /// 获取可用的触发源列表。
    pub fn available_trigger_sources(&self) -> Result<Vec<String>> {
        let cam = self.lock_camera();
        let sources = cam.dup_available_trigger_sources()?;
        Ok(sources.iter().map(|s| s.to_string()).collect())
    }

    // ────────────────────────────────────────────────────────────
    //  设备信息
    // ────────────────────────────────────────────────────────────

    /// 获取相机型号名称。
    pub fn model_name(&self) -> Result<String> {
        let cam = self.lock_camera();
        Ok(cam.model_name()?.to_string())
    }

    /// 获取相机厂商名称。
    pub fn vendor_name(&self) -> Result<String> {
        let cam = self.lock_camera();
        Ok(cam.vendor_name()?.to_string())
    }

    /// 获取 Aravis 层面的设备 ID。
    pub fn aravis_device_id(&self) -> Result<String> {
        let cam = self.lock_camera();
        Ok(cam.device_id()?.to_string())
    }

    // ────────────────────────────────────────────────────────────
    //  通用参数访问（GenICam 节点）
    // ────────────────────────────────────────────────────────────

    /// 获取整型参数值。
    pub fn get_integer(&self, feature: &str) -> Result<i64> {
        let cam = self.lock_camera();
        Ok(cam.integer(feature)?)
    }

    /// 设置整型参数值。
    pub fn set_integer(&self, feature: &str, value: i64) -> Result<()> {
        let cam = self.lock_camera();
        cam.set_integer(feature, value)?;
        Ok(())
    }

    /// 获取浮点型参数值。
    pub fn get_float(&self, feature: &str) -> Result<f64> {
        let cam = self.lock_camera();
        Ok(cam.float(feature)?)
    }

    /// 设置浮点型参数值。
    pub fn set_float(&self, feature: &str, value: f64) -> Result<()> {
        let cam = self.lock_camera();
        cam.set_float(feature, value)?;
        Ok(())
    }

    /// 获取布尔型参数值。
    pub fn get_boolean(&self, feature: &str) -> Result<bool> {
        let cam = self.lock_camera();
        Ok(cam.boolean(feature)?)
    }

    /// 设置布尔型参数值。
    pub fn set_boolean(&self, feature: &str, value: bool) -> Result<()> {
        let cam = self.lock_camera();
        cam.set_boolean(feature, value)?;
        Ok(())
    }

    /// 获取字符串型参数值。
    pub fn get_string(&self, feature: &str) -> Result<String> {
        let cam = self.lock_camera();
        Ok(cam.string(feature)?.to_string())
    }

    /// 设置字符串型参数值。
    pub fn set_string(&self, feature: &str, value: &str) -> Result<()> {
        let cam = self.lock_camera();
        cam.set_string(feature, value)?;
        Ok(())
    }

    /// 执行命令型参数。
    pub fn execute_command(&self, feature: &str) -> Result<()> {
        let cam = self.lock_camera();
        cam.execute_command(feature)?;
        Ok(())
    }

    /// 获取枚举参数的可用字符串值列表。
    pub fn available_enumerations(&self, feature: &str) -> Result<Vec<String>> {
        let cam = self.lock_camera();
        let values = cam.dup_available_enumerations_as_strings(feature)?;
        Ok(values.iter().map(|s| s.to_string()).collect())
    }

    /// 检查某个特性是否可用。
    pub fn is_feature_available(&self, feature: &str) -> Result<bool> {
        let cam = self.lock_camera();
        Ok(cam.is_feature_available(feature)?)
    }

    // ────────────────────────────────────────────────────────────
    //  内部辅助
    // ────────────────────────────────────────────────────────────

    /// 安全锁定 Camera Mutex。
    ///
    /// 如果 Mutex 被中毒（其他线程持有锁时 panic），
    /// 自动恢复而非级联 panic，保证系统不崩溃。
    fn lock_camera(&self) -> std::sync::MutexGuard<'_, aravis::Camera> {
        self.inner.lock().unwrap_or_else(|poisoned| {
            log::warn!("camera mutex was poisoned, recovering");
            poisoned.into_inner()
        })
    }

    /// 安全锁定 Stats Mutex。
    fn lock_stats(&self) -> std::sync::MutexGuard<'_, CameraStats> {
        self.stats.lock().unwrap_or_else(|poisoned| {
            log::warn!("stats mutex was poisoned, recovering");
            poisoned.into_inner()
        })
    }

    /// 确保相机已打开。
    fn ensure_open(&self) -> Result<()> {
        if self.is_open.load(Ordering::Acquire) {
            Ok(())
        } else {
            Err(CameraError::DeviceNotOpen)
        }
    }

    // ────────────────────────────────────────────────────────────
    //  Stream 持续采集模式 (高帧率)
    // ────────────────────────────────────────────────────────────

    /// 安全锁定 Stream Mutex。
    fn lock_stream(&self) -> std::sync::MutexGuard<'_, Option<aravis::Stream>> {
        self.stream.lock().unwrap_or_else(|poisoned| {
            log::warn!("stream mutex was poisoned, recovering");
            poisoned.into_inner()
        })
    }

    /// 打开 Stream 持续采集模式（高帧率）。
    ///
    /// 与 `get_frame()` 的一站式模式不同，Stream 模式：
    /// 1. 创建一个持久 Stream
    /// 2. 预分配 N 个 Buffer 推入 Stream
    /// 3. 启动连续采集
    /// 4. 后续调用 `stream_pop_frame()` 从 Stream 获取帧
    ///
    /// 这避免了每帧重建 Stream 的开销，FPS 可达相机最大速率。
    ///
    /// # Arguments
    /// * `n_buffers` - 预分配缓冲区数量（建议 10-50）
    pub fn open_stream(&self, n_buffers: usize) -> Result<()> {
        self.ensure_open()?;

        // 先关闭旧 stream
        self.close_stream();

        let cam = self.lock_camera();

        // 获取 payload 大小
        let payload = cam.payload()?;
        log::info!("stream: payload size = {} bytes", payload);

        // 创建 Stream
        let stream = cam
            .create_stream()
            .map_err(|e| CameraError::AravisError(format!("create_stream failed: {e}")))?;

        // 预分配 Buffer 并推入 Stream
        for _ in 0..n_buffers {
            let buffer = aravis::Buffer::new_allocate(payload as usize);
            stream.push_buffer(buffer);
        }
        log::info!("stream: pushed {} buffers", n_buffers);

        // 启动连续采集
        cam.start_acquisition()?;
        log::info!("stream: acquisition started (continuous)");

        drop(cam);

        // 保存 Stream 引用
        *self.lock_stream() = Some(stream);

        Ok(())
    }

    /// 从 Stream 获取一帧（阻塞等待，带超时）。
    ///
    /// 需要先调用 `open_stream()` 启动 Stream。
    /// 获取帧后自动将 Buffer 回收到 Stream 中（push back），保证循环使用。
    pub fn stream_pop_frame(&self) -> Result<Frame> {
        self.ensure_open()?;

        let stream_guard = self.lock_stream();
        let stream = stream_guard.as_ref().ok_or_else(|| {
            CameraError::DeviceNotOpen // Stream 未打开
        })?;

        // 读取像素格式
        let fmt_name = {
            let cam = self.lock_camera();
            cam.pixel_format_as_string()
                .map(|s| s.to_string())
                .unwrap_or_default()
        };

        // 从 Stream 获取 Buffer（带超时）
        let buffer = stream
            .timeout_pop_buffer(self.timeout_us)
            .ok_or_else(|| CameraError::AcquisitionTimeout(self.timeout_us / 1_000_000))?;

        // 检查 buffer 状态 (避免访问损坏/不完整帧触发 C assertion)
        let status = buffer.status();
        if status != aravis::BufferStatus::Success {
            // 回收损坏的 buffer
            stream.push_buffer(buffer);
            return Err(CameraError::AravisError(format!(
                "buffer status: {:?} (packet loss or incomplete)",
                status
            )));
        }

        // 构建 Frame
        let frame = Frame::from_buffer(&buffer, &fmt_name)?;

        // Buffer 回收（push back 以循环使用）
        stream.push_buffer(buffer);

        // 更新统计
        let mut stats = self.lock_stats();
        stats.total_frames += 1;
        stats.last_frame_time = Some(Instant::now());

        Ok(frame)
    }

    /// 从 Stream 获取一帧（带重试的鲁棒版本）。
    pub fn robust_stream_pop_frame(&self) -> Result<Frame> {
        for attempt in 0..=self.max_retries {
            match self.stream_pop_frame() {
                Ok(frame) => return Ok(frame),
                Err(e) if attempt < self.max_retries => {
                    self.lock_stats().failed_frames += 1;
                    let delay_ms = self.retry_delay_base_ms * (1u64 << attempt.min(6));
                    log::warn!(
                        "stream_pop_frame failed (attempt {}/{}): {}, retrying in {}ms...",
                        attempt + 1,
                        self.max_retries,
                        e,
                        delay_ms
                    );
                    std::thread::sleep(Duration::from_millis(delay_ms));
                }
                Err(e) => {
                    self.lock_stats().failed_frames += 1;
                    return Err(e);
                }
            }
        }
        Err(CameraError::ReconnectFailed(self.max_retries))
    }

    /// 关闭 Stream 采集。
    ///
    /// 停止连续采集并释放 Stream 和缓冲区。
    pub fn close_stream(&self) {
        let mut stream_guard = self.lock_stream();
        if stream_guard.is_some() {
            // 停止采集
            if let Ok(cam) = self.inner.lock() {
                if let Err(e) = cam.stop_acquisition() {
                    log::warn!("stop_acquisition failed: {}", e);
                }
            }
            *stream_guard = None;
            log::info!("stream closed");
        }
    }

    /// 是否已打开 Stream。
    pub fn is_streaming(&self) -> bool {
        self.lock_stream().is_some()
    }
}

impl Drop for GigECamera {
    fn drop(&mut self) {
        if self.is_open.load(Ordering::Acquire) {
            if let Err(e) = self.close() {
                log::error!("error closing camera on drop: {}", e);
            }
        }
    }
}

/// RAII 相机守卫。
///
/// `Drop` 时自动关闭相机。
pub struct CameraGuard<'a> {
    camera: &'a GigECamera,
}

impl<'a> CameraGuard<'a> {
    /// 获取内部相机引用。
    pub fn camera(&self) -> &GigECamera {
        self.camera
    }
}

impl<'a> std::ops::Deref for CameraGuard<'a> {
    type Target = GigECamera;

    fn deref(&self) -> &Self::Target {
        self.camera
    }
}

impl<'a> Drop for CameraGuard<'a> {
    fn drop(&mut self) {
        if let Err(e) = self.camera.close() {
            log::error!("error closing camera guard: {}", e);
        }
    }
}
