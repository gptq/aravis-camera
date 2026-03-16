//! 搜索相机并设置 IP 的示例。
//!
//! ## 运行
//! ```bash
//! # 搜索所有相机
//! cargo run -p aravis-camera --example force_ip
//!
//! # 设置指定设备的 IP
//! cargo run -p aravis-camera --example force_ip -- \
//!     --device "设备ID" \
//!     --ip 192.168.1.100 \
//!     --subnet 255.255.255.0 \
//!     --gateway 192.168.1.1
//! ```

fn main() -> aravis_camera::Result<()> {
    env_logger::init();

    let args: Vec<String> = std::env::args().collect();

    // 无参数时：搜索所有相机
    if args.len() <= 1 {
        return search_only();
    }

    // 有参数时：设置 IP
    set_device_ip(&args)
}

/// 搜索所有相机，显示详细信息。
fn search_only() -> aravis_camera::Result<()> {
    println!("搜索所有 GigE 相机...\n");

    let cameras = aravis_camera::discover_cameras()?;

    if cameras.is_empty() {
        println!("未发现相机。请检查：");
        println!("  1. 相机已通过网线连接");
        println!("  2. 相机与电脑在同一子网");
        println!("  3. 防火墙允许 GigE Vision 流量");
        return Ok(());
    }

    println!("发现 {} 台相机：\n", cameras.len());
    println!(
        "{:<4} {:<35} {:<15} {:<15} {:<12} {:<20}",
        "#", "ID", "地址", "厂商", "型号", "协议"
    );
    println!("{}", "─".repeat(105));

    for (i, cam) in cameras.iter().enumerate() {
        println!(
            "{:<4} {:<35} {:<15} {:<15} {:<12} {:<20}",
            i + 1,
            cam.id,
            cam.address,
            cam.vendor,
            cam.model,
            cam.protocol,
        );
    }

    // 显示本机连接信息
    for cam in &cameras {
        if !cam.address.is_empty() {
            if let Ok(host_ip) = aravis_camera::get_host_ip_by_target_ip(&cam.address) {
                println!("\n本机 → {} 使用网卡 IP: {}", cam.address, host_ip);
            }
        }
    }

    println!("\n要设置相机 IP，请使用参数运行（见 --help）。");
    Ok(())
}

/// 解析参数并设置设备 IP。
fn set_device_ip(args: &[String]) -> aravis_camera::Result<()> {
    let mut device = None;
    let mut ip = None;
    let mut subnet = None;
    let mut gateway = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--device" | "-d" => {
                device = args.get(i + 1).map(|s| s.as_str());
                i += 2;
            }
            "--ip" => {
                ip = args.get(i + 1).map(|s| s.as_str());
                i += 2;
            }
            "--subnet" => {
                subnet = args.get(i + 1).map(|s| s.as_str());
                i += 2;
            }
            "--gateway" => {
                gateway = args.get(i + 1).map(|s| s.as_str());
                i += 2;
            }
            "--help" | "-h" => {
                print_help();
                return Ok(());
            }
            _ => {
                eprintln!("未知参数: {}", args[i]);
                print_help();
                return Ok(());
            }
        }
    }

    let device = device.unwrap_or_else(|| {
        eprintln!("错误: 必须指定 --device");
        std::process::exit(1);
    });
    let ip = ip.unwrap_or_else(|| {
        eprintln!("错误: 必须指定 --ip");
        std::process::exit(1);
    });
    let subnet = subnet.unwrap_or("255.255.255.0");
    let gateway = gateway.unwrap_or("0.0.0.0");

    println!("设置相机 IP:");
    println!("  设备:   {}", device);
    println!("  IP:     {}", ip);
    println!("  子网:   {}", subnet);
    println!("  网关:   {}", gateway);
    println!();

    aravis_camera::force_ip(device, ip, subnet, gateway)?;
    println!("✅ IP 设置成功！");
    println!("注意: 相机可能需要几秒钟重启网络。");

    Ok(())
}

fn print_help() {
    println!("用法: force_ip [选项]");
    println!();
    println!("无参数时搜索所有相机并显示信息。");
    println!();
    println!("选项:");
    println!("  --device, -d <ID>     设备标识符或当前 IP（必须）");
    println!("  --ip <IP>             新 IP 地址（必须）");
    println!("  --subnet <MASK>       子网掩码（默认: 255.255.255.0）");
    println!("  --gateway <GW>        网关地址（默认: 0.0.0.0）");
    println!("  --help, -h            显示帮助");
    println!();
    println!("示例:");
    println!("  force_ip --device 192.168.1.50 --ip 192.168.1.100 --subnet 255.255.255.0");
}
