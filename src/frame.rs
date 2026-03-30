//! 图像帧数据结构。
//!
//! 提供从 Aravis `Buffer` 到自有数据的转换，包含 Bayer 去马赛克功能。

use crate::demosaic::{self, BayerPattern};
use crate::error::{CameraError, Result};

/// 从相机采集到的一帧图像。
#[derive(Debug, Clone)]
pub struct Frame {
    /// 图像宽度（像素）。
    pub width: u32,
    /// 图像高度（像素）。
    pub height: u32,
    /// 像素格式名称（如 `BayerGR8`, `Mono8`, `RGB8`）。
    pub pixel_format: String,
    /// 每像素位数。
    pub bits_per_pixel: u32,
    /// 行步长（字节）。通常等于 width × (bpp/8)，但可能包含 padding。
    pub stride: u32,
    /// 原始图像数据。
    pub data: Vec<u8>,
    /// 相机时间戳（纳秒，来自相机内部时钟）。
    pub timestamp_ns: u64,
    /// 系统时间戳（纳秒，帧到达主机的时间）。
    pub system_timestamp_ns: u64,
    /// 帧 ID（GigE 相机自增序号，0 表示无效）。
    pub frame_id: u64,
}

impl Frame {
    /// 从 Aravis `Buffer` 创建 `Frame`。
    ///
    /// # 参数
    /// - `buffer`: Aravis Buffer 对象
    /// - `pixel_format_name`: 像素格式可读名称（从 Camera::pixel_format() 获取）
    ///
    /// 会复制 buffer 中的图像数据到自有内存。
    pub fn from_buffer(buffer: &aravis::Buffer, pixel_format_name: &str) -> Result<Self> {
        let width = buffer.image_width();
        let height = buffer.image_height();

        if width <= 0 || height <= 0 {
            return Err(CameraError::InvalidBuffer);
        }

        let pixel_format_raw = buffer.image_pixel_format();
        let bits = aravis::bits_per_pixel(pixel_format_raw);

        // 获取原始数据指针和长度
        let (data_ptr, data_len) = buffer.data();
        if data_ptr.is_null() || data_len == 0 {
            return Err(CameraError::InvalidBuffer);
        }

        // SAFETY: data_ptr 和 data_len 来自 buffer.data()，
        // 在 buffer 生命周期内指针有效，且 data_len 不超过分配大小。
        // 我们立即复制到 Vec，不持有裸指针引用。
        let data = unsafe { std::slice::from_raw_parts(data_ptr, data_len) }.to_vec();

        let w = width as u32;
        let h = height as u32;
        let bpp = bits as u32;

        // 计算行步长：如果数据长度大于 width*height*(bpp/8)，可能有 padding
        let bytes_per_pixel = bpp.div_ceil(8);
        let expected_no_padding = (w * h * bytes_per_pixel) as usize;
        let stride = if data.len() > expected_no_padding && h > 1 {
            // 有 padding：stride = total_data / height（向上取整到行边界）
            (data.len() / h as usize) as u32
        } else {
            w * bytes_per_pixel
        };

        // 格式名：优先使用传入的可读名，回退到十六进制
        let format_name = if pixel_format_name.is_empty() {
            format!("0x{:08X}", pixel_format_raw.raw())
        } else {
            pixel_format_name.to_string()
        };

        Ok(Frame {
            width: w,
            height: h,
            pixel_format: format_name,
            bits_per_pixel: bpp,
            stride,
            data,
            timestamp_ns: buffer.timestamp(),
            system_timestamp_ns: buffer.system_timestamp(),
            frame_id: buffer.frame_id(),
        })
    }

    // ────────────────────────────────────────────────────────────
    //  格式检测
    // ────────────────────────────────────────────────────────────

    /// 是否为 Bayer 格式（需要去马赛克）。
    pub fn is_bayer(&self) -> bool {
        BayerPattern::detect(&self.pixel_format).is_some()
    }

    /// 是否为 RGB 图像（24bpp）。
    pub fn is_rgb(&self) -> bool {
        self.bits_per_pixel == 24
    }

    /// 是否为灰度图像（8bpp 且非 Bayer）。
    pub fn is_mono(&self) -> bool {
        self.bits_per_pixel == 8 && !self.is_bayer()
    }

    // ────────────────────────────────────────────────────────────
    //  图像转换
    // ────────────────────────────────────────────────────────────

    /// 转换为 RGB24 数据。
    ///
    /// 根据像素格式自动选择处理方式：
    /// - **Bayer 8-bit** → 双线性插值去马赛克 → RGB24
    /// - **Bayer 10/12/16-bit** → 归一化到 8-bit → 去马赛克 → RGB24
    /// - **Mono8** → 灰度扩展为 RGB24（R=G=B）
    /// - **RGB24** → 直接复制
    /// - 其他 → 返回错误
    ///
    /// # 返回
    /// RGB24 数据（每像素 3 字节，R-G-B 顺序），长度 = width × height × 3
    pub fn to_rgb(&self) -> Result<Vec<u8>> {
        let w = self.width;
        let h = self.height;

        // 1) Bayer 格式 → 去马赛克
        if let Some(pattern) = BayerPattern::detect(&self.pixel_format) {
            return match self.bits_per_pixel {
                8 => demosaic::demosaic_8bit(&self.data, w, h, self.stride, pattern),
                10 | 12 => demosaic::demosaic_16bit_to_8bit(
                    &self.data,
                    w,
                    h,
                    self.stride,
                    pattern,
                    self.bits_per_pixel,
                ),
                16 => demosaic::demosaic_16bit_to_8bit(&self.data, w, h, self.stride, pattern, 16),
                _ => Err(CameraError::GenericError(format!(
                    "不支持 {}bpp 的 Bayer 去马赛克",
                    self.bits_per_pixel
                ))),
            };
        }

        // 2) Mono8 → 灰度扩展 RGB
        if self.bits_per_pixel == 8 {
            let expected = (w * h) as usize;
            let mut rgb = Vec::with_capacity(expected * 3);
            for i in 0..expected.min(self.data.len()) {
                let v = self.data[i];
                rgb.push(v);
                rgb.push(v);
                rgb.push(v);
            }
            // 数据不足时填充黑色
            while rgb.len() < expected * 3 {
                rgb.push(0);
            }
            return Ok(rgb);
        }

        // 3) RGB24 → 直接返回
        if self.bits_per_pixel == 24 {
            let expected = (w * h * 3) as usize;
            let mut rgb = self.data.clone();
            rgb.resize(expected, 0);
            return Ok(rgb);
        }

        // 4) 不支持的格式
        Err(CameraError::GenericError(format!(
            "to_rgb: 不支持的像素格式 {} ({}bpp)",
            self.pixel_format, self.bits_per_pixel
        )))
    }

    /// 图像数据总字节数。
    pub fn data_size(&self) -> usize {
        self.data.len()
    }

    /// 图像形状 (height, width, channels)。
    pub fn shape(&self) -> (u32, u32, u32) {
        let channels = self.bits_per_pixel.div_ceil(8);
        (self.height, self.width, channels)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_frame_shape() {
        let frame = Frame {
            width: 640,
            height: 480,
            pixel_format: "RGB8".to_string(),
            bits_per_pixel: 24,
            stride: 640 * 3,
            data: vec![0u8; 640 * 480 * 3],
            timestamp_ns: 0,
            system_timestamp_ns: 0,
            frame_id: 1,
        };
        assert_eq!(frame.shape(), (480, 640, 3));
        assert!(frame.is_rgb());
        assert!(!frame.is_mono());
        assert!(!frame.is_bayer());
        assert_eq!(frame.data_size(), 640 * 480 * 3);
    }

    #[test]
    fn test_mono_frame() {
        let frame = Frame {
            width: 320,
            height: 240,
            pixel_format: "Mono8".to_string(),
            bits_per_pixel: 8,
            stride: 320,
            data: vec![128u8; 320 * 240],
            timestamp_ns: 1000,
            system_timestamp_ns: 2000,
            frame_id: 42,
        };
        assert!(!frame.is_rgb());
        assert!(frame.is_mono());
        assert!(!frame.is_bayer());

        // Mono8 to_rgb: 应扩展为 RGB
        let rgb = frame.to_rgb().unwrap();
        assert_eq!(rgb.len(), 320 * 240 * 3);
        assert_eq!(rgb[0], 128);
        assert_eq!(rgb[1], 128);
        assert_eq!(rgb[2], 128);
    }

    #[test]
    fn test_bayer_frame() {
        let frame = Frame {
            width: 4,
            height: 4,
            pixel_format: "BayerGR8".to_string(),
            bits_per_pixel: 8,
            stride: 4,
            data: vec![
                50, 200, 55, 210, 30, 100, 35, 110, 60, 190, 65, 195, 25, 105, 28, 108,
            ],
            timestamp_ns: 0,
            system_timestamp_ns: 0,
            frame_id: 1,
        };
        assert!(frame.is_bayer());
        assert!(!frame.is_mono());
        assert!(!frame.is_rgb());

        let rgb = frame.to_rgb().unwrap();
        assert_eq!(rgb.len(), 4 * 4 * 3);
        // 所有像素应有有效 RGB 值
        assert!(rgb.iter().any(|&v| v > 0));
    }
}
