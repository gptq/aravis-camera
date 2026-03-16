//! 相机搜索与网络配置模块。
//!
//! 提供搜索 GigE 相机、获取设备信息、设置相机 IP 等功能。

use crate::error::{CameraError, Result};

/// 搜索到的相机设备信息。
#[derive(Debug, Clone)]
pub struct CameraInfo {
    /// 设备标识符（Aravis 格式）。
    pub id: String,
    /// 物理地址（MAC 或 IP）。
    pub physical_id: String,
    /// 网络地址。
    pub address: String,
    /// 厂商名称。
    pub vendor: String,
    /// 型号名称。
    pub model: String,
    /// 协议类型（`"GigEVision"` 或 `"USB3Vision"`）。
    pub protocol: String,
}

/// 搜索网络中所有可用的 GigE / USB3 相机。
///
/// 内部调用 `Aravis::get_device_list()` 获取设备列表。
///
/// # Example
/// ```ignore
/// let cameras = aravis_camera::discover_cameras()?;
/// for cam in &cameras {
///     println!("{}: {} {}", cam.id, cam.vendor, cam.model);
/// }
/// ```
pub fn discover_cameras() -> Result<Vec<CameraInfo>> {
    let aravis = crate::camera::ensure_aravis_initialized();

    let devices = aravis.get_device_list();
    if devices.is_empty() {
        log::info!("no cameras found on the network");
        return Ok(Vec::new());
    }

    let cameras: Vec<CameraInfo> = devices
        .iter()
        .map(|dev| CameraInfo {
            id: dev.id.to_string_lossy().into_owned(),
            physical_id: dev.physical_id.to_string_lossy().into_owned(),
            address: dev.address.to_string_lossy().into_owned(),
            vendor: dev.vendor.to_string_lossy().into_owned(),
            model: dev.model.to_string_lossy().into_owned(),
            protocol: dev.protocol.to_string_lossy().into_owned(),
        })
        .collect();

    log::info!("discovered {} camera(s)", cameras.len());
    Ok(cameras)
}

/// 获取所有已发现相机的 ID 列表。
pub fn get_all_camera_ids() -> Result<Vec<String>> {
    let cameras = discover_cameras()?;
    Ok(cameras.into_iter().map(|c| c.id).collect())
}

/// 获取连接到目标 IP 所使用的本机网卡 IP。
///
/// 通过 UDP 连接来探测路由选择的本地地址。
pub fn get_host_ip_by_target_ip(target_ip: &str) -> Result<String> {
    use std::net::UdpSocket;

    let socket = UdpSocket::bind("0.0.0.0:0")?;
    socket.connect(format!("{target_ip}:80"))?;
    let local_addr = socket.local_addr()?;
    Ok(local_addr.ip().to_string())
}

/// 强制设置相机 IP 地址（Persistent IP）。
///
/// 通过 GenICam 标准寄存器设置持久 IP。
///
/// # Arguments
/// * `device_id` - 设备标识符或当前 IP
/// * `ip` - 新 IP 地址
/// * `subnet` - 子网掩码
/// * `gateway` - 网关地址
pub fn force_ip(device_id: &str, ip: &str, subnet: &str, gateway: &str) -> Result<()> {
    use aravis::prelude::*;

    crate::camera::ensure_aravis_initialized();

    let camera = aravis::Camera::new(Some(device_id))?;
    let device = camera.device().ok_or(CameraError::DeviceNotOpen)?;

    let ip_int = ip_str_to_u32(ip);
    let subnet_int = ip_str_to_u32(subnet);
    let gateway_int = ip_str_to_u32(gateway);

    device.set_integer_feature_value("GevCurrentIPConfigurationLLA", 0)?;
    device.set_integer_feature_value("GevCurrentIPConfigurationDHCP", 0)?;
    device.set_integer_feature_value("GevCurrentIPConfigurationPersistentIP", 1)?;
    device.set_integer_feature_value("GevPersistentIPAddress", ip_int as i64)?;
    device.set_integer_feature_value("GevPersistentSubnetMask", subnet_int as i64)?;
    device.set_integer_feature_value("GevPersistentDefaultGateway", gateway_int as i64)?;

    log::info!(
        "force IP set: device={}, ip={}, subnet={}, gateway={}",
        device_id, ip, subnet, gateway
    );
    Ok(())
}

/// IP 字符串转 u32。
///
/// # Example
/// ```
/// # use aravis_camera::ip_str_to_u32;
/// assert_eq!(ip_str_to_u32("192.168.1.100"), 0xC0A80164);
/// ```
pub fn ip_str_to_u32(ip: &str) -> u32 {
    let parts: Vec<u32> = ip.split('.').filter_map(|s| s.parse().ok()).collect();
    if parts.len() == 4 {
        (parts[0] << 24) | (parts[1] << 16) | (parts[2] << 8) | parts[3]
    } else {
        0
    }
}

/// u32 转 IP 字符串。
///
/// # Example
/// ```
/// # use aravis_camera::u32_to_ip_str;
/// assert_eq!(u32_to_ip_str(0xC0A80164), "192.168.1.100");
/// ```
pub fn u32_to_ip_str(ip: u32) -> String {
    format!(
        "{}.{}.{}.{}",
        (ip >> 24) & 0xFF,
        (ip >> 16) & 0xFF,
        (ip >> 8) & 0xFF,
        ip & 0xFF
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ip_str_to_u32() {
        assert_eq!(ip_str_to_u32("192.168.1.100"), 0xC0A80164);
        assert_eq!(ip_str_to_u32("10.0.0.1"), 0x0A000001);
        assert_eq!(ip_str_to_u32("255.255.255.0"), 0xFFFFFF00);
    }

    #[test]
    fn test_u32_to_ip_str() {
        assert_eq!(u32_to_ip_str(0xC0A80164), "192.168.1.100");
        assert_eq!(u32_to_ip_str(0x0A000001), "10.0.0.1");
        assert_eq!(u32_to_ip_str(0xFFFFFF00), "255.255.255.0");
    }

    #[test]
    fn test_ip_roundtrip() {
        let ips = ["192.168.1.100", "10.0.0.1", "172.16.0.254", "0.0.0.0"];
        for ip in &ips {
            assert_eq!(u32_to_ip_str(ip_str_to_u32(ip)), *ip);
        }
    }
}
