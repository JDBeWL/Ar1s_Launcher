//! 实例截图的扫描与路径校验
//!
//! 截图只读；时间取自 Minecraft 文件名，分辨率只读 PNG 头，不整图解码。

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use chrono::{Local, NaiveDateTime, TimeZone};

use crate::errors::LauncherError;
use crate::models::ScreenshotInfo;
use crate::services::{config, launcher::isolation};
use crate::utils::file_utils;

const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
const CAPTURE_TIME_FORMAT: &str = "%Y-%m-%d_%H.%M.%S";

pub(crate) fn screenshot_dir(instance_name: &str) -> Result<PathBuf, LauncherError> {
    let config = config::load_config()?;
    let game_dir = PathBuf::from(&config.game_dir);
    Ok(isolation::content_root(&config, &game_dir, instance_name).join("screenshots"))
}

pub(crate) fn list_screenshots_in(dir: &Path) -> Result<Vec<ScreenshotInfo>, LauncherError> {
    if !dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut screenshots = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_file() {
            continue;
        }
        let Some(file_name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if !file_name.to_ascii_lowercase().ends_with(".png") {
            continue;
        }

        let path = entry.path();
        let Ok((width, height)) = read_png_size(&path) else {
            continue;
        };
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        let modified_at = meta
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_millis() as i64);
        let Some(captured_at) = parse_capture_time(&file_name).or(modified_at) else {
            continue;
        };

        screenshots.push(ScreenshotInfo {
            file_name,
            path: path.to_string_lossy().into_owned(),
            captured_at,
            size: meta.len(),
            width,
            height,
        });
    }

    screenshots.sort_by(|a, b| {
        b.captured_at
            .cmp(&a.captured_at)
            .then_with(|| b.file_name.to_lowercase().cmp(&a.file_name.to_lowercase()))
    });
    Ok(screenshots)
}

pub(crate) fn resolve_screenshot_in(dir: &Path, file_name: &str) -> Result<PathBuf, LauncherError> {
    if !file_name.to_ascii_lowercase().ends_with(".png") {
        return Err(LauncherError::Custom("截图文件名无效".to_string()));
    }
    file_utils::validate_file_name_or_error(file_name)?;

    let path = dir.join(file_name);
    if !path.is_file() {
        return Err(LauncherError::Custom(format!("截图不存在: {file_name}")));
    }

    let root = fs::canonicalize(dir)?;
    let resolved = fs::canonicalize(&path)?;
    if !resolved.starts_with(&root) {
        return Err(LauncherError::Custom("截图路径无效".to_string()));
    }
    Ok(resolved)
}

fn parse_capture_time(file_name: &str) -> Option<i64> {
    let stem = Path::new(file_name).file_stem()?.to_str()?;
    let naive = NaiveDateTime::parse_from_str(stem, CAPTURE_TIME_FORMAT).ok()?;
    Local
        .from_local_datetime(&naive)
        .single()
        .map(|time| time.timestamp_millis())
}

fn read_png_size(path: &Path) -> Result<(u32, u32), LauncherError> {
    let mut file = fs::File::open(path)?;
    let mut header = [0u8; 24];
    file.read_exact(&mut header)?;

    if header[..8] != PNG_SIGNATURE || &header[12..16] != b"IHDR" {
        return Err(LauncherError::Custom("不是有效的 PNG 文件".to_string()));
    }

    let width = u32::from_be_bytes([header[16], header[17], header[18], header[19]]);
    let height = u32::from_be_bytes([header[20], header[21], header[22], header[23]]);
    if width == 0 || height == 0 {
        return Err(LauncherError::Custom("PNG 尺寸无效".to_string()));
    }
    Ok((width, height))
}

#[cfg(windows)]
const MAX_CLIPBOARD_PIXELS: u64 = 33_554_432;

/// 构造 CF_DIB：40 字节 BITMAPINFOHEADER + 自底向上的 32 位 BGRA
#[cfg(windows)]
fn build_dib(width: u32, height: u32, bgra: &[u8]) -> Vec<u8> {
    const HEADER_SIZE: usize = 40;
    let row_size = width as usize * 4;
    let image_size = row_size * height as usize;
    debug_assert_eq!(bgra.len(), image_size);

    let mut dib = vec![0u8; HEADER_SIZE + image_size];
    dib[0..4].copy_from_slice(&(HEADER_SIZE as u32).to_le_bytes());
    dib[4..8].copy_from_slice(&(width as i32).to_le_bytes());
    dib[8..12].copy_from_slice(&(height as i32).to_le_bytes());
    dib[12..14].copy_from_slice(&1u16.to_le_bytes());
    dib[14..16].copy_from_slice(&32u16.to_le_bytes());
    dib[20..24].copy_from_slice(&(image_size as u32).to_le_bytes());

    for row in 0..height as usize {
        let src = (height as usize - 1 - row) * row_size;
        let dst = HEADER_SIZE + row * row_size;
        dib[dst..dst + row_size].copy_from_slice(&bgra[src..src + row_size]);
    }
    dib
}

pub fn copy_screenshot(instance_name: &str, file_name: &str) -> Result<(), LauncherError> {
    let dir = screenshot_dir(instance_name)?;
    let path = resolve_screenshot_in(&dir, file_name)?;
    clipboard::copy_png(&path)
}

pub fn open_screenshots_folder(instance_name: &str) -> Result<(), LauncherError> {
    let dir = screenshot_dir(instance_name)?;
    fs::create_dir_all(&dir)?;
    opener::open(&dir).map_err(|error| LauncherError::Custom(format!("无法打开截图目录: {error}")))
}

#[cfg(windows)]
mod clipboard {
    use std::fs::File;
    use std::io::BufReader;
    use std::path::Path;
    use std::ptr;
    use std::thread;
    use std::time::Duration;

    use windows_sys::Win32::Foundation::GlobalFree;
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
    };
    use windows_sys::Win32::System::Memory::{
        GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock,
    };

    use super::{MAX_CLIPBOARD_PIXELS, build_dib};
    use crate::errors::LauncherError;

    const CLIPBOARD_FORMAT_DIB: u32 = 8;
    const OPEN_ATTEMPTS: usize = 5;
    const RETRY_DELAY: Duration = Duration::from_millis(60);

    pub(super) fn copy_png(path: &Path) -> Result<(), LauncherError> {
        let (width, height, pixels) = decode_bgra(path)?;
        let dib = build_dib(width, height, &pixels);
        write_clipboard(&dib)
    }

    fn decode_bgra(path: &Path) -> Result<(u32, u32, Vec<u8>), LauncherError> {
        let file = File::open(path)?;
        let mut decoder = png::Decoder::new(BufReader::new(file));
        decoder.set_transformations(png::Transformations::normalize_to_color8());
        let mut reader = decoder
            .read_info()
            .map_err(|error| LauncherError::Custom(format!("读取截图失败: {error}")))?;

        let (width, height) = reader.info().size();
        if width == 0 || height == 0 {
            return Err(LauncherError::Custom("截图尺寸无效".to_string()));
        }
        if u64::from(width) * u64::from(height) > MAX_CLIPBOARD_PIXELS {
            return Err(LauncherError::Custom("截图尺寸过大，无法复制".to_string()));
        }

        let buffer_size = reader
            .output_buffer_size()
            .ok_or_else(|| LauncherError::Custom("截图尺寸无效".to_string()))?;
        let mut buffer = vec![0u8; buffer_size];
        let info = reader
            .next_frame(&mut buffer)
            .map_err(|error| LauncherError::Custom(format!("解码截图失败: {error}")))?;
        let data = &buffer[..info.buffer_size()];
        let pixel_count = width as usize * height as usize;
        let mut bgra = Vec::with_capacity(pixel_count * 4);

        match info.color_type {
            png::ColorType::Rgba => {
                for pixel in data.as_chunks::<4>().0 {
                    bgra.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
                }
            }
            png::ColorType::Rgb => {
                for pixel in data.as_chunks::<3>().0 {
                    bgra.extend_from_slice(&[pixel[2], pixel[1], pixel[0], 255]);
                }
            }
            png::ColorType::Grayscale => {
                for value in data {
                    bgra.extend_from_slice(&[*value, *value, *value, 255]);
                }
            }
            png::ColorType::GrayscaleAlpha => {
                for pixel in data.as_chunks::<2>().0 {
                    bgra.extend_from_slice(&[pixel[0], pixel[0], pixel[0], pixel[1]]);
                }
            }
            png::ColorType::Indexed => {
                return Err(LauncherError::Custom("不支持的 PNG 颜色格式".to_string()));
            }
        }

        if bgra.len() != pixel_count * 4 {
            return Err(LauncherError::Custom("截图解码失败".to_string()));
        }
        Ok((width, height, bgra))
    }

    fn write_clipboard(dib: &[u8]) -> Result<(), LauncherError> {
        for attempt in 0..OPEN_ATTEMPTS {
            if unsafe { OpenClipboard(ptr::null_mut()) } != 0 {
                break;
            }
            if attempt + 1 == OPEN_ATTEMPTS {
                return Err(last_os_error("无法打开剪贴板"));
            }
            thread::sleep(RETRY_DELAY);
        }

        let result = (|| -> Result<(), LauncherError> {
            if unsafe { EmptyClipboard() } == 0 {
                return Err(last_os_error("无法清空剪贴板"));
            }

            let handle = unsafe { GlobalAlloc(GMEM_MOVEABLE, dib.len()) };
            if handle.is_null() {
                return Err(last_os_error("无法分配剪贴板内存"));
            }
            let locked = unsafe { GlobalLock(handle) };
            if locked.is_null() {
                unsafe { GlobalFree(handle) };
                return Err(last_os_error("无法锁定剪贴板内存"));
            }
            unsafe {
                ptr::copy_nonoverlapping(dib.as_ptr(), locked.cast(), dib.len());
                GlobalUnlock(handle);
            }
            if unsafe { SetClipboardData(CLIPBOARD_FORMAT_DIB, handle) }.is_null() {
                unsafe { GlobalFree(handle) };
                return Err(last_os_error("无法写入剪贴板"));
            }
            Ok(())
        })();

        unsafe { CloseClipboard() };
        result
    }

    fn last_os_error(action: &str) -> LauncherError {
        LauncherError::Custom(format!("{action}: {}", std::io::Error::last_os_error()))
    }
}

#[cfg(not(windows))]
mod clipboard {
    use std::path::Path;

    use crate::errors::LauncherError;

    pub(super) fn copy_png(_path: &Path) -> Result<(), LauncherError> {
        Err(LauncherError::Custom("当前平台不支持复制截图".to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Local, TimeZone};
    use std::fs::{self, File};
    use std::io::BufWriter;
    use tempfile::TempDir;

    fn write_png(path: &Path, width: u32, height: u32) {
        let file = File::create(path).unwrap();
        let mut encoder = png::Encoder::new(BufWriter::new(file), width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer
            .write_image_data(&vec![0u8; width as usize * height as usize * 4])
            .unwrap();
    }

    #[test]
    fn parses_capture_time_from_minecraft_name() {
        let expected = Local
            .with_ymd_and_hms(2026, 10, 25, 15, 30, 45)
            .single()
            .unwrap()
            .timestamp_millis();
        assert_eq!(
            parse_capture_time("2026-10-25_15.30.45.png"),
            Some(expected)
        );
        assert_eq!(parse_capture_time("not-a-time.png"), None);
        assert_eq!(parse_capture_time("2026-13-40_99.99.99.png"), None);
    }

    #[test]
    fn reads_png_dimensions_and_rejects_garbage() {
        let dir = TempDir::new().unwrap();
        let png = dir.path().join("shot.png");
        write_png(&png, 3, 2);
        assert_eq!(read_png_size(&png).unwrap(), (3, 2));

        let broken = dir.path().join("broken.png");
        fs::write(&broken, b"not a png").unwrap();
        assert!(read_png_size(&broken).is_err());
    }

    #[test]
    fn lists_only_top_level_pngs_newest_first() {
        let dir = TempDir::new().unwrap();
        let shots = dir.path().join("screenshots");
        fs::create_dir_all(&shots).unwrap();
        write_png(&shots.join("2026-10-25_10.00.00.png"), 2, 2);
        write_png(&shots.join("2026-10-25_12.00.00.PNG"), 3, 2);
        write_png(&shots.join("2099-01-01_00.00.00.png"), 1, 1);
        fs::write(shots.join("notes.txt"), b"ignore").unwrap();
        fs::write(shots.join("broken.png"), b"not a png").unwrap();
        fs::create_dir_all(shots.join("nested")).unwrap();
        write_png(&shots.join("nested/2026-10-25_13.00.00.png"), 1, 1);

        let list = list_screenshots_in(&shots).unwrap();
        assert_eq!(list.len(), 3);
        assert_eq!(list[0].file_name, "2099-01-01_00.00.00.png");
        assert_eq!(list[1].file_name, "2026-10-25_12.00.00.PNG");
        assert_eq!(list[1].width, 3);
        assert_eq!(list[1].height, 2);
        assert!(list[0].captured_at > list[1].captured_at);
        assert!(list.iter().all(|shot| shot.path.ends_with(&shot.file_name)));
    }

    #[test]
    fn rejects_screenshot_names_outside_the_folder() {
        let dir = TempDir::new().unwrap();
        write_png(&dir.path().join("2026-10-25_10.00.00.png"), 1, 1);

        assert!(resolve_screenshot_in(dir.path(), "2026-10-25_10.00.00.png").is_ok());
        for name in ["../escape.png", "nested/a.png", "evil.txt", "C:evil.png"] {
            assert!(
                resolve_screenshot_in(dir.path(), name).is_err(),
                "{name} 应被拒绝"
            );
        }
    }
}

#[cfg(all(test, windows))]
mod clipboard_tests {
    use super::build_dib;

    #[test]
    fn builds_bottom_up_bgra_dib() {
        // 两行 BGRA：内存顺序是上排、下排，DIB 要翻成下排在前
        let top = [1u8, 2, 3, 4, 5, 6, 7, 8];
        let bottom = [9u8, 10, 11, 12, 13, 14, 15, 16];
        let mut pixels = top.to_vec();
        pixels.extend_from_slice(&bottom);

        let dib = build_dib(2, 2, &pixels);
        assert_eq!(&dib[0..4], &40u32.to_le_bytes());
        assert_eq!(&dib[4..8], &2i32.to_le_bytes());
        assert_eq!(&dib[8..12], &2i32.to_le_bytes());
        assert_eq!(&dib[12..14], &1u16.to_le_bytes());
        assert_eq!(&dib[14..16], &32u16.to_le_bytes());
        assert_eq!(&dib[16..20], &0u32.to_le_bytes());
        assert_eq!(&dib[20..24], &16u32.to_le_bytes());
        assert_eq!(dib.len(), 40 + 16);
        assert_eq!(&dib[40..48], &bottom);
        assert_eq!(&dib[48..56], &top);
    }
}
