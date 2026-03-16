//! 多相机并发管理模块。
//!
//! 提供 `MultiCamera` 结构体，管理多个 `GigECamera` 实例，
//! 支持并行采集和批量操作。
//!
//! ## 工业稳定性设计
//!
//! - **线程 panic 容错**：单台相机 panic 不影响其他相机
//! - **精准重试**：仅重置失败的相机，不中断正常工作的相机
//! - **健康检查**：`health_check_all()` 批量心跳检测

use std::collections::BTreeMap;
use std::thread;

use crate::camera::GigECamera;
use crate::error::{CameraError, Result};
use crate::frame::Frame;

/// 多相机管理器。
///
/// 维护一个按标识符排序的相机集合，支持并行操作。
pub struct MultiCamera {
    cameras: BTreeMap<String, GigECamera>,
}

impl MultiCamera {
    /// 创建多相机管理器。
    ///
    /// # Arguments
    /// * `ids` - 相机标识符列表（IP 或设备 ID）
    ///
    /// # Example
    /// ```ignore
    /// let multi = MultiCamera::new(&["192.168.1.100", "192.168.1.101"])?;
    /// ```
    pub fn new(ids: &[&str]) -> Result<Self> {
        let mut cameras = BTreeMap::new();
        for &id in ids {
            let cam = GigECamera::new(Some(id))?;
            cameras.insert(id.to_string(), cam);
        }
        Ok(Self { cameras })
    }

    /// 从已发现的相机自动创建。
    pub fn from_discovered() -> Result<Self> {
        let ids = crate::discovery::get_all_camera_ids()?;
        if ids.is_empty() {
            return Err(CameraError::NoCameraFound);
        }
        let id_refs: Vec<&str> = ids.iter().map(|s| s.as_str()).collect();
        Self::new(&id_refs)
    }

    /// 打开所有相机。
    pub fn open_all(&self) -> Result<()> {
        for (id, cam) in &self.cameras {
            cam.open().map_err(|e| {
                log::error!("failed to open camera {}: {}", id, e);
                e
            })?;
        }
        Ok(())
    }

    /// 关闭所有相机。
    pub fn close_all(&self) -> Result<()> {
        let mut last_error = None;
        for (id, cam) in &self.cameras {
            if let Err(e) = cam.close() {
                log::error!("failed to close camera {}: {}", id, e);
                last_error = Some(e);
            }
        }
        match last_error {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// 并行采集所有相机的帧。
    ///
    /// 返回按相机 ID 排序的帧字典。
    /// 单台相机线程 panic 不会崩溃，而是记录错误日志。
    pub fn get_all_frames(&self) -> Result<BTreeMap<String, Frame>> {
        let results: BTreeMap<String, Result<Frame>> = thread::scope(|s| {
            let handles: Vec<_> = self
                .cameras
                .iter()
                .map(|(id, cam)| {
                    let id = id.clone();
                    s.spawn(move || (id, cam.get_frame()))
                })
                .collect();

            handles
                .into_iter()
                .filter_map(|h| match h.join() {
                    Ok(result) => Some(result),
                    Err(e) => {
                        log::error!("camera thread panicked: {:?}", e);
                        None
                    }
                })
                .collect()
        });

        let mut frames = BTreeMap::new();
        for (id, result) in results {
            frames.insert(
                id.clone(),
                result.map_err(|e| {
                    log::error!("failed to get frame from camera {}: {}", id, e);
                    e
                })?,
            );
        }
        Ok(frames)
    }

    /// 并行采集，仅重试失败的相机。
    ///
    /// 第一轮采集所有相机；如果有失败的，
    /// 只重置并重试它们，不中断成功的相机。
    pub fn robust_get_all_frames(&self) -> Result<BTreeMap<String, Frame>> {
        let mut frames = BTreeMap::new();
        let mut failed_ids = Vec::new();

        // 第一轮采集
        for (id, cam) in &self.cameras {
            match cam.get_frame() {
                Ok(frame) => {
                    frames.insert(id.clone(), frame);
                }
                Err(e) => {
                    log::warn!("camera {} failed: {}, will retry", id, e);
                    failed_ids.push(id.clone());
                }
            }
        }

        // 仅重试失败的相机
        for id in &failed_ids {
            if let Some(cam) = self.cameras.get(id) {
                log::info!("resetting failed camera {}...", id);
                if let Err(e) = cam.reset() {
                    log::error!("reset camera {} failed: {}", id, e);
                    continue;
                }
                match cam.get_frame() {
                    Ok(frame) => {
                        frames.insert(id.clone(), frame);
                        log::info!("camera {} recovered", id);
                    }
                    Err(e) => {
                        log::error!("camera {} retry failed: {}", id, e);
                        return Err(e);
                    }
                }
            }
        }

        Ok(frames)
    }

    /// 对所有相机执行健康检查。
    ///
    /// 返回 (healthy_ids, unhealthy_ids)。
    pub fn health_check_all(&self) -> (Vec<String>, Vec<String>) {
        let mut healthy = Vec::new();
        let mut unhealthy = Vec::new();

        for (id, cam) in &self.cameras {
            match cam.health_check() {
                Ok(()) => healthy.push(id.clone()),
                Err(e) => {
                    log::warn!("camera {} health check failed: {}", id, e);
                    unhealthy.push(id.clone());
                }
            }
        }

        (healthy, unhealthy)
    }

    /// 获取相机数量。
    pub fn len(&self) -> usize {
        self.cameras.len()
    }

    /// 是否没有相机。
    pub fn is_empty(&self) -> bool {
        self.cameras.is_empty()
    }

    /// 获取指定 ID 的相机引用。
    pub fn get(&self, id: &str) -> Option<&GigECamera> {
        self.cameras.get(id)
    }

    /// 获取指定 ID 的相机可变引用。
    pub fn get_mut(&mut self, id: &str) -> Option<&mut GigECamera> {
        self.cameras.get_mut(id)
    }

    /// 获取所有相机 ID 列表。
    pub fn ids(&self) -> Vec<String> {
        self.cameras.keys().cloned().collect()
    }

    /// 遍历所有相机。
    pub fn iter(&self) -> impl Iterator<Item = (&String, &GigECamera)> {
        self.cameras.iter()
    }

    /// 可变遍历所有相机。
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&String, &mut GigECamera)> {
        self.cameras.iter_mut()
    }
}

impl Drop for MultiCamera {
    fn drop(&mut self) {
        if let Err(e) = self.close_all() {
            log::error!("error closing multi-camera on drop: {}", e);
        }
    }
}
