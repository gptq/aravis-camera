//! 连接相机并采集一帧图像。
//!
//! 运行：
//! ```bash
//! cargo run -p aravis-camera --example grab_frame
//! # 或指定相机 IP：
//! cargo run -p aravis-camera --example grab_frame -- 192.168.1.100
//! ```

fn main() -> aravis_camera::Result<()> {
    env_logger::init();

    // 从命令行参数获取相机 ID（可选）
    let args: Vec<String> = std::env::args().collect();
    let camera_id = args.get(1).map(|s| s.as_str());

    println!(
        "Connecting to camera: {}",
        camera_id.unwrap_or("(first available)")
    );

    let cam = aravis_camera::GigECamera::new(camera_id)?;

    // 打印设备信息
    println!("  Model:  {}", cam.model_name().unwrap_or_default());
    println!("  Vendor: {}", cam.vendor_name().unwrap_or_default());
    println!("  ID:     {}", cam.device_id().unwrap_or_default());

    // 打开相机
    let guard = cam.open_guard()?;

    // 尝试设置 RGB 格式
    match guard.set_rgb() {
        Ok(()) => println!("  Pixel format: RGB"),
        Err(e) => {
            println!("  RGB not available ({}), using default", e);
        }
    }

    // 打印当前设置
    if let Ok(format) = guard.pixel_format() {
        println!("  Current pixel format: {}", format);
    }
    if let Ok(exposure) = guard.exposure_time() {
        println!("  Exposure time: {:.1} us", exposure);
    }
    if let Ok(region) = guard.region() {
        println!(
            "  Region: ({}, {}) {}x{}",
            region.0, region.1, region.2, region.3
        );
    }

    // GigE 自动协商包大小
    if guard.is_gv_device() {
        if let Ok(()) = guard.gv_auto_packet_size() {
            println!("  Auto packet size negotiated");
        }
    }

    // 采集一帧
    println!("\nAcquiring frame...");
    let frame = guard.get_frame()?;

    let (h, w, c) = frame.shape();
    println!("  Frame: {}x{} ({} channels, {} bpp)", w, h, c, frame.bits_per_pixel);
    println!("  Data size: {} bytes", frame.data_size());
    println!("  Pixel format: {}", frame.pixel_format);

    println!("\nDone!");
    Ok(())
}
