//! 相机参数配置示例。
//!
//! 演示如何搜索相机、连接并设置各种参数（曝光、分辨率、像素格式），
//! 这是其它 Rust 项目调用 `aravis-camera` 的典型代码。
//!
//! ## 运行
//! ```bash
//! cargo run -p aravis-camera --example set_params
//! # 或指定相机 IP：
//! cargo run -p aravis-camera --example set_params -- 192.168.1.100
//! ```

fn main() -> aravis_camera::Result<()> {
    env_logger::init();

    // ──────────────────────────────────────────────────────
    //  第一步：搜索相机
    // ──────────────────────────────────────────────────────
    println!("=== 1. 搜索所有 GigE 相机 ===\n");
    let cameras = aravis_camera::discover_cameras()?;
    if cameras.is_empty() {
        println!("未发现相机，请检查网络连接。");
        return Ok(());
    }
    for cam in &cameras {
        println!(
            "  [{}] {} {} (IP: {}, 协议: {})",
            cam.id, cam.vendor, cam.model, cam.address, cam.protocol
        );
    }

    // ──────────────────────────────────────────────────────
    //  第二步：连接相机
    // ──────────────────────────────────────────────────────
    let args: Vec<String> = std::env::args().collect();
    let camera_id = args.get(1).map(|s| s.as_str());

    println!(
        "\n=== 2. 连接相机: {} ===\n",
        camera_id.unwrap_or("(第一台)")
    );
    let cam = aravis_camera::GigECamera::new(camera_id)?;
    println!("  型号: {}", cam.model_name().unwrap_or_default());
    println!("  厂商: {}", cam.vendor_name().unwrap_or_default());

    // ──────────────────────────────────────────────────────
    //  第三步：GigE 网络优化
    // ──────────────────────────────────────────────────────
    if cam.is_gv_device() {
        println!("\n=== 3. GigE 网络优化 ===\n");
        cam.gv_auto_packet_size()?;
        println!("  自动协商包大小完成");

        // 可选：设置包延时（多相机时防丢包）
        // cam.gv_set_packet_delay(1000)?;
    }

    // ──────────────────────────────────────────────────────
    //  第四步：设置曝光时间
    // ──────────────────────────────────────────────────────
    println!("\n=== 4. 设置曝光时间 ===\n");

    // 查看曝光范围
    if let Ok((min, max)) = cam.exposure_time_bounds() {
        println!("  曝光范围: {:.1} ~ {:.1} us", min, max);
    }

    // 设置 10ms 手动曝光
    cam.set_exposure_time(10000.0)?;
    println!("  已设置曝光: {:.1} us", cam.exposure_time()?);

    // 也可以用秒为单位：
    // cam.set_exposure_time_by_second(0.01)?;

    // 或设置自动曝光：
    // cam.set_exposure_auto(aravis::Auto::Continuous)?;

    // ──────────────────────────────────────────────────────
    //  第五步：设置像素格式
    // ──────────────────────────────────────────────────────
    println!("\n=== 5. 设置像素格式 ===\n");

    // 查看可用格式
    if let Ok(formats) = cam.available_pixel_formats() {
        println!("  可用格式: {:?}", formats);
    }

    // 设置 RGB 格式
    match cam.set_rgb() {
        Ok(()) => println!("  已设置: RGB8"),
        Err(e) => println!("  RGB 不可用 ({}), 尝试 RAW...", e),
    }

    // 或设置 Bayer RAW 格式：
    // cam.set_raw(8)?;   // 8-bit Bayer
    // cam.set_raw(12)?;  // 12-bit Bayer

    // 或指定精确格式：
    // cam.set_pixel_format("Mono8")?;
    // cam.set_pixel_format("BayerRG12")?;

    // ──────────────────────────────────────────────────────
    //  第六步：设置分辨率 / ROI
    // ──────────────────────────────────────────────────────
    println!("\n=== 6. 设置分辨率 ===\n");

    // 查看传感器全尺寸
    if let Ok((w, h)) = cam.sensor_size() {
        println!("  传感器尺寸: {}x{}", w, h);
    }

    // 查看当前 ROI
    if let Ok((x, y, w, h)) = cam.region() {
        println!("  当前 ROI: ({}, {}) {}x{}", x, y, w, h);
    }

    // 设置 ROI（从左上角开始，640x480）
    // cam.set_region(0, 0, 640, 480)?;

    // 设置 binning（2x2 下采样）
    // cam.set_binning(2, 2)?;

    // ──────────────────────────────────────────────────────
    //  第七步：软触发采集
    // ──────────────────────────────────────────────────────
    println!("\n=== 7. 软触发采集 ===\n");

    // open_guard 使用 RAII，离开作用域自动关闭
    {
        let guard = cam.open_guard()?;
        let frame = guard.get_frame()?;

        let (h, w, c) = frame.shape();
        println!(
            "  采集到帧: {}x{}, {} 通道, {} bpp",
            w, h, c, frame.bits_per_pixel
        );
        println!("  数据大小: {} 字节", frame.data_size());
        println!("  像素格式: {}", frame.pixel_format);

        // frame.data 是 Vec<u8>，可以直接用于图像处理
        // 例如保存为文件、传给 OpenCV、送入推理引擎等

        // 可以通过 GenICam 节点名访问任意参数
        if let Ok(val) = guard.get_integer("GevSCPSPacketSize") {
            println!("  GevSCPSPacketSize: {}", val);
        }
    } // guard drop → 自动停止采集

    println!("\n=== 完成 ===");
    Ok(())
}
