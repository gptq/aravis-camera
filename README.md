# aravis-camera

基于 [Aravis](https://github.com/AravisProject/aravis) 的 GigE Vision 工业相机 Rust 库。

通过开放的 **GigE Vision / GenICam** 标准协议控制工业相机，无需厂商私有 SDK。
兼容**海康（HikRobot）**、大华等所有符合 GigE Vision V2.0 标准的相机。

## 设计思路

### 为什么选择 Aravis 替代 MVS SDK？

| 维度          | MVS SDK                      | Aravis                     |
|--------------|------------------------------|----------------------------|
| **架构**      | x86_64 only + Rosetta 2     | **arm64 原生**（Apple Silicon）|
| **依赖**      | 闭源 SDK 200MB+              | `brew install aravis`       |
| **协议**      | 海康私有协议                   | GigE Vision V2.0 标准       |
| **跨厂商**    | 仅海康                        | 任意 GigE Vision 相机        |
| **维护成本**   | 手写 FFI 绑定                 | Rust crate 自动绑定          |

### 架构设计

```
┌────────────────────────────────────────────┐
│           你的 Rust / Python 应用            │
├────────────────────────────────────────────┤
│            aravis-camera (本库)             │
│  ┌──────────┬─────────┬──────────────────┐ │
│  │ camera   │ frame   │ discovery        │ │
│  │ 相机控制  │ 帧数据   │ 搜索 / IP 设置   │ │
│  ├──────────┼─────────┼──────────────────┤ │
│  │ demosaic │ multi_camera │ error       │ │
│  │ Bayer去马 │ 多相机并发   │ 错误处理    │ │
│  └──────────┴─────────────┴──────────────┘ │
├────────────────────────────────────────────┤
│        aravis crate (Rust bindings)        │
├────────────────────────────────────────────┤
│     libaravis (C, brew install aravis)     │
├────────────────────────────────────────────┤
│          GigE Vision / GenICam 协议         │
├────────────────────────────────────────────┤
│      海康 / 大华 / Basler / ... 工业相机    │
└────────────────────────────────────────────┘
```

### 核心设计原则

- **RAII 资源管理**：`CameraGuard` 通过 `Drop` trait 确保相机自动关闭
- **线程安全**：`Mutex` 保护所有相机操作，支持多线程并发
- **自动 Bayer 去马赛克**：`Frame.to_rgb()` 自动检测 Bayer 格式并转为 RGB
- **统一错误处理**：`CameraError` 枚举覆盖所有错误类型
- **模块化**：每个功能独立模块，职责清晰，便于维护

## 环境要求

```bash
# 安装 Aravis 和 pkg-config（macOS）
brew install aravis pkg-config
```

## 功能列表

### 图像采集与处理

| 功能 | 函数 | 说明 |
|------|------|------|
| 采集一帧 | `get_frame()` | 软触发 + 获取帧（Frame 中 pixel_format 为可读名） |
| 容错采集 | `robust_get_frame()` | 失败自动重置重试（指数退避） |
| **转为 RGB** | **`frame.to_rgb()`** | **自动 Bayer 去马赛克 / Mono 扩展 / RGB 直通** |
| 是否 Bayer | `frame.is_bayer()` | 检测是否需要去马赛克 |
| 是否灰度 | `frame.is_mono()` | 检测非 Bayer 8bit |
| 是否 RGB | `frame.is_rgb()` | 检测 24bpp |
| 像素格式 | `frame.pixel_format` | 可读名（如 `BayerGR8`、`Mono8`） |

### Bayer 去马赛克（demosaic 模块）

| 功能 | 函数 | 说明 |
|------|------|------|
| 模式检测 | `BayerPattern::detect(name)` | 从格式名自动检测 RGGB/GRBG/GBRG/BGGR |
| 8-bit 去马赛克 | `demosaic_8bit(raw, w, h, stride, pattern)` | 双线性插值 → RGB24 |
| 16-bit 去马赛克 | `demosaic_16bit_to_8bit(raw, w, h, stride, pattern, bits)` | 归一化 + 去马赛克 |

> 支持 stride ≠ width（行对齐 padding），边缘使用 clamp 处理。

### 相机搜索与网络配置

| 功能 | 函数 | 说明 |
|------|------|------|
| 搜索所有相机 | `discover_cameras()` | 列出网络中所有 GigE 相机 |
| 获取相机 ID 列表 | `get_all_camera_ids()` | 返回所有相机标识符 |
| 设置相机 IP | `force_ip(device, ip, subnet, gw)` | 强制设置持久 IP |
| 查询本机网卡 IP | `get_host_ip_by_target_ip(target)` | 获取与目标通信的本机地址 |

### 相机参数设置

| 功能 | 函数 | 说明 |
|------|------|------|
| 曝光时间 | `set_exposure_time(us)` | 微秒，自动 clamp 到有效范围 |
| 曝光范围 | `exposure_time_bounds()` | 获取 (min, max) 微秒 |
| 增益控制 | `set_gain(value)` / `gain()` | 手动增益 |
| 像素格式 | `set_pixel_format("BayerRG8")` | 精确指定格式 |
| 快捷 RGB | `set_rgb()` | 自动选择 RGB8 格式 |
| 快捷 RAW | `set_raw(bit)` | 自动选择 Bayer RAW 格式 |
| 可用格式 | `available_pixel_formats()` | 列出相机支持的所有格式 |
| 分辨率/ROI | `set_region(x, y, w, h)` | 设置感兴趣区域 |
| 传感器尺寸 | `sensor_size()` | 获取传感器全分辨率 |

### GigE 网络优化

| 功能 | 函数 | 说明 |
|------|------|------|
| 自动包大小 | `gv_auto_packet_size()` | 自动协商最优包大小 |
| 包延时 | `gv_set_packet_delay(ns)` | 多相机防丢包 |
| 包大小 | `gv_set_packet_size(size)` | 手动设置（WiFi 建议 1500） |

### 多相机并发

| 功能 | 函数 | 说明 |
|------|------|------|
| 创建多相机 | `MultiCamera::new(&["ip1","ip2"])` | 批量连接 |
| 批量打开 | `open_all()` | 并行打开所有相机 |
| 并行采集 | `get_all_frames()` | `thread::scope` 并行采集 |
| 容错采集 | `robust_get_all_frames()` | 失败自动重试 |

## Rust 调用示例

### 添加依赖

```toml
# 你的项目 Cargo.toml
[dependencies]
aravis-camera = { git = "https://github.com/gptq/aravis-camera.git" }
image = "0.25"  # 可选，用于保存图片
```

### 采集并保存彩色图片（最常用）

```rust
use aravis_camera::{GigECamera, Result};

fn main() -> Result<()> {
    let cam = GigECamera::new(Some("192.168.2.91"))?;
    cam.set_exposure_time(10000.0)?; // 10ms

    let guard = cam.open_guard()?;
    let frame = guard.get_frame()?;

    println!("{}x{}, 格式={}, Bayer={}",
        frame.width, frame.height,
        frame.pixel_format,     // "BayerGR8"
        frame.is_bayer());      // true

    // 自动 Bayer 去马赛克 → RGB24
    let rgb = frame.to_rgb()?;
    let img = image::RgbImage::from_raw(frame.width, frame.height, rgb)
        .expect("构建 RGB 图像失败");
    img.save("capture.png").expect("保存失败");

    println!("已保存 capture.png");
    Ok(())
}
```

### 搜索相机

```rust
use aravis_camera::Result;

fn main() -> Result<()> {
    let cameras = aravis_camera::discover_cameras()?;
    for cam in &cameras {
        println!("{}: {} {} (IP: {})", cam.id, cam.vendor, cam.model, cam.address);
    }
    Ok(())
}
```

### 完整参数配置 + 采集

```rust
use aravis_camera::{GigECamera, Result};

fn main() -> Result<()> {
    let cam = GigECamera::new(Some("192.168.1.100"))?;

    // GigE 网络优化（WiFi 环境建议手动设置）
    if cam.is_gv_device() {
        cam.gv_set_packet_size(1500)?;     // WiFi 安全值
        cam.gv_set_packet_delay(50000)?;   // 50μs 防丢包
    }

    // 设置曝光时间 10ms
    cam.set_exposure_time(10000.0)?;

    // 设置全分辨率
    let (w, h) = cam.sensor_size()?;
    cam.set_region(0, 0, w, h)?;

    // 读取当前像素格式
    let fmt = cam.pixel_format()?;
    println!("当前像素格式: {}", fmt);

    // 打开相机并采集（RAII 自动关闭）
    {
        let guard = cam.open_guard()?;

        // 容错采集（失败自动重试）
        let frame = guard.robust_get_frame()?;
        println!("{}x{}, {}, {} bytes",
            frame.width, frame.height,
            frame.pixel_format,
            frame.data_size());

        // 转为 RGB（Bayer 自动去马赛克）
        let rgb = frame.to_rgb()?;
        println!("RGB 数据: {} bytes", rgb.len());
    }

    Ok(())
}
```

### 多相机并行采集

```rust
use aravis_camera::{MultiCamera, Result};

fn main() -> Result<()> {
    let mut multi = MultiCamera::new(&["192.168.2.91", "192.168.2.145"])?;
    multi.open_all()?;

    let frames = multi.get_all_frames()?;
    for (id, frame) in &frames {
        let rgb = frame.to_rgb()?;
        let img = image::RgbImage::from_raw(frame.width, frame.height, rgb).unwrap();
        img.save(format!("{}.png", id.replace('.', "_"))).unwrap();
        println!("{}: {}x{} {} → 已保存", id, frame.width, frame.height, frame.pixel_format);
    }

    Ok(())
}
```

### 通用 GenICam 参数访问

```rust
use aravis_camera::{GigECamera, Result};

fn main() -> Result<()> {
    let cam = GigECamera::new(None)?;

    // 读取任意 GenICam 参数
    let packet_size = cam.get_integer("GevSCPSPacketSize")?;
    println!("包大小: {}", packet_size);

    // 查看枚举参数的可用值
    let modes = cam.available_enumerations("ExposureAuto")?;
    println!("曝光模式: {:?}", modes);

    Ok(())
}
```

## 帧数据结构（Frame）

```rust
pub struct Frame {
    pub width: u32,              // 图像宽度（像素）
    pub height: u32,             // 图像高度（像素）
    pub pixel_format: String,    // "BayerGR8", "Mono8", "RGB8" 等
    pub bits_per_pixel: u32,     // 8 / 10 / 12 / 16 / 24
    pub stride: u32,             // 行步长（字节），可能含 padding
    pub data: Vec<u8>,           // 原始像素数据
    pub timestamp_ns: u64,       // 相机时间戳（纳秒）
    pub system_timestamp_ns: u64,// 系统时间戳（纳秒）
    pub frame_id: u64,           // 帧 ID
}

// 关键方法
impl Frame {
    fn to_rgb(&self) -> Result<Vec<u8>>;  // 自动转 RGB24
    fn is_bayer(&self) -> bool;           // 是否 Bayer 格式
    fn is_mono(&self) -> bool;            // 是否灰度（非 Bayer）
    fn is_rgb(&self) -> bool;             // 是否 RGB 24bpp
    fn data_size(&self) -> usize;         // 数据字节数
    fn shape(&self) -> (u32, u32, u32);   // (H, W, C)
}
```

### `to_rgb()` 处理逻辑

| 输入格式 | 处理 | 输出 |
|---------|------|------|
| BayerGR8 / BayerRG8 / BayerGB8 / BayerBG8 | 双线性插值去马赛克 | RGB24 |
| BayerXX10 / BayerXX12 | 归一化到 8-bit + 去马赛克 | RGB24 |
| BayerXX16 | 归一化到 8-bit + 去马赛克 | RGB24 |
| Mono8 | 灰度扩展 (R=G=B) | RGB24 |
| RGB24 | 直接返回 | RGB24 |

## WiFi 环境注意事项

在 WiFi（非有线）环境下使用 GigE 相机，需特别注意：

```rust
// WiFi 环境推荐配置
cam.gv_set_packet_size(1500)?;     // MTU 限制
cam.gv_set_packet_delay(50000)?;   // 50μs 包间延时防丢包
cam.set_raw(8)?;                   // 使用 Bayer 8-bit（低带宽）
```

## 项目结构

```
aravis-camera/
├── Cargo.toml                    # 依赖: aravis 0.11, thiserror 2, log 0.4
├── src/
│   ├── lib.rs                    # 公开 API 导出 + crate 文档
│   ├── error.rs                  # CameraError 枚举（含 GenericError）
│   ├── discovery.rs              # 搜索相机 / IP 设置 / ForceIP
│   ├── camera.rs                 # GigECamera 核心控制器
│   │                             #   曝光 / 增益 / 像素格式 / ROI
│   │                             #   软触发 / GigE 网络 / GenICam 参数
│   ├── frame.rs                  # Frame 图像帧（to_rgb / is_bayer）
│   ├── demosaic.rs               # Bayer 去马赛克（双线性插值）
│   │                             #   BayerPattern / 8bit / 16bit / stride
│   └── multi_camera.rs           # MultiCamera 多相机并发管理
├── examples/
│   ├── discover.rs               # 搜索所有相机
│   ├── grab_frame.rs             # 连接 + 采集一帧
│   ├── set_params.rs             # 完整参数配置流程
│   └── force_ip.rs               # 搜索 + 设置相机 IP
└── README.md                     # 本文档
```

## 相关项目

- **aravis-camera-test**：实机集成测试（12 项测试、38 张图片验证）
- **hik-camera**：基于海康 MVS SDK 的版本（需 Rosetta 2）

## License

MIT
