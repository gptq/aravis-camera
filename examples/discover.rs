//! 搜索所有 GigE 相机并打印设备信息。
//!
//! 运行：
//! ```bash
//! cargo run -p aravis-camera --example discover
//! ```

fn main() -> aravis_camera::Result<()> {
    env_logger::init();

    println!("Searching for GigE cameras...\n");

    let cameras = aravis_camera::discover_cameras()?;

    if cameras.is_empty() {
        println!("No cameras found.");
        return Ok(());
    }

    println!("Found {} camera(s):\n", cameras.len());
    println!("{:<5} {:<30} {:<20} {:<20}", "#", "ID", "Vendor", "Model");
    println!("{}", "-".repeat(75));

    for (i, cam) in cameras.iter().enumerate() {
        println!(
            "{:<5} {:<30} {:<20} {:<20}",
            i + 1,
            cam.id,
            cam.vendor,
            cam.model,
        );
    }

    Ok(())
}
