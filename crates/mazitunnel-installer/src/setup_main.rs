#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![cfg_attr(not(windows), allow(dead_code, unused_variables))]

use ab_glyph::{Font, FontArc, GlyphId, PxScale, ScaleFont};
use flate2::read::GzDecoder;
use std::env;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

#[cfg(windows)]
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE, HWND, LPARAM, LRESULT, RECT, WPARAM},
    Graphics::Dwm::DwmSetWindowAttribute,
    Graphics::Gdi::{
        BeginPaint, EndPaint, GetDC, ReleaseDC, SetDIBitsToDevice, UpdateWindow, BITMAPINFO,
        BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HDC, PAINTSTRUCT,
    },
    System::Com::CoTaskMemFree,
    System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    },
    System::LibraryLoader::GetModuleHandleW,
    System::Registry::{
        RegCloseKey, RegCreateKeyW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
        HKEY_CURRENT_USER, KEY_READ, REG_DWORD, REG_SZ,
    },
    System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE},
    UI::Shell::{
        SHBrowseForFolderW, SHChangeNotify, SHGetFolderPathW, SHGetPathFromIDListW, BROWSEINFOW,
    },
    UI::WindowsAndMessaging::{
        AdjustWindowRectEx, AllowSetForegroundWindow, CreateWindowExW, DefWindowProcW,
        DispatchMessageW, GetClientRect, GetMessageW, GetSystemMetrics, GetWindowLongPtrW,
        KillTimer, LoadCursorW, LoadIconW, MessageBoxW, PostQuitMessage, RegisterClassExW,
        SendMessageW, SetForegroundWindow, SetProcessDPIAware, SetTimer, SetWindowLongPtrW,
        SetWindowPos, ShowWindow, TranslateMessage, CS_HREDRAW, CS_VREDRAW, GWLP_USERDATA,
        ICON_BIG, ICON_SMALL, IDC_ARROW, IDI_APPLICATION, MB_ICONERROR, MB_OK, MSG, SM_CXSCREEN,
        SM_CYSCREEN, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SW_SHOW, WM_CREATE, WM_DESTROY,
        WM_ERASEBKGND, WM_LBUTTONDOWN, WM_MOUSEMOVE, WM_PAINT, WM_SETICON, WM_TIMER, WNDCLASSEXW,
        WS_CAPTION, WS_MINIMIZEBOX, WS_OVERLAPPED, WS_SYSMENU,
    },
};

const WIN_W: usize = 500;
const WIN_H: usize = 360;

// Gomulu Ikili Veriler ve Varliklar
const PAYLOAD_CLIENT: &[u8] = include_bytes!("../payload_client.bin");
const PAYLOAD_UNINSTALL: &[u8] = include_bytes!("../payload_uninstall.bin");
const APP_ICON_ICO: &[u8] = include_bytes!("../../../assets/app.ico");
const FONT_REGULAR_BYTES: &[u8] = include_bytes!("../../../assets/font_regular.ttf");
const FONT_BOLD_BYTES: &[u8] = include_bytes!("../../../assets/font_bold.ttf");
static SETUP_ICON_56: &[u8] = include_bytes!("../../../assets/setup_icon_56.rgba");

// Renk yardimcilari (0xAARRGGBB)
#[inline(always)]
fn rgba(r: u8, g: u8, b: u8, a: u8) -> u32 {
    ((a as u32) << 24) | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
}

#[inline(always)]
fn color_r(c: u32) -> u8 {
    ((c >> 16) & 0xFF) as u8
}
#[inline(always)]
fn color_g(c: u32) -> u8 {
    ((c >> 8) & 0xFF) as u8
}
#[inline(always)]
fn color_b(c: u32) -> u8 {
    (c & 0xFF) as u8
}
#[inline(always)]
fn color_a(c: u32) -> u8 {
    ((c >> 24) & 0xFF) as u8
}

struct SetupCanvas {
    width: u32,
    height: u32,
    pixels: Vec<u32>,
    font_reg: FontArc,
    font_bold: FontArc,
}

impl SetupCanvas {
    fn new(width: u32, height: u32) -> Self {
        let size = (width * height) as usize;

        let font_reg = fs::read(r"C:\Windows\Fonts\segoeui.ttf")
            .ok()
            .and_then(|bytes| FontArc::try_from_vec(bytes).ok())
            .unwrap_or_else(|| {
                FontArc::try_from_slice(FONT_REGULAR_BYTES)
                    .expect("Geçerli regular font yüklenemedi")
            });

        let font_bold = fs::read(r"C:\Windows\Fonts\segoeuib.ttf")
            .ok()
            .and_then(|bytes| FontArc::try_from_vec(bytes).ok())
            .unwrap_or_else(|| {
                FontArc::try_from_slice(FONT_BOLD_BYTES)
                    .expect("Geçerli bold font yüklenemedi")
            });

        Self {
            width,
            height,
            pixels: vec![0xFF0C0C0E; size],
            font_reg,
            font_bold,
        }
    }

    fn clear(&mut self, color: u32) {
        self.pixels.fill(color);
    }

    fn draw_architectural_backdrop(&mut self, is_dark: bool) {
        let w = self.width as f32;
        let h = self.height as f32;
        let center_x = w * 0.5;
        let center_y = 120.0f32;
        let max_r = (center_x * center_x + (h - center_y) * (h - center_y)).sqrt().max(1.0);

        let max_alpha = if is_dark { 18.0f32 } else { 14.0f32 };
        let dot_rgb = if is_dark { (255u8, 255u8, 255u8) } else { (0u8, 0u8, 0u8) };

        let step = 16usize;
        let w_u = self.width as usize;
        let h_u = self.height as usize;

        for y in (8..h_u).step_by(step) {
            let y_f = y as f32;
            let dy = y_f - center_y;
            let row = y * w_u;

            for x in (8..w_u).step_by(step) {
                let x_f = x as f32;
                let dx = x_f - center_x;
                let dist = (dx * dx + dy * dy).sqrt();
                let falloff = (1.0 - (dist / max_r)).clamp(0.0, 1.0);
                let alpha = (max_alpha * falloff * falloff).round() as u8;
                if alpha > 0 {
                    let idx = row + x;
                    if idx < self.pixels.len() {
                        let bg = self.pixels[idx];
                        let a_u = alpha as u32;
                        let inv_a = 255 - a_u;
                        let r = ((color_r(bg) as u32 * inv_a + dot_rgb.0 as u32 * a_u) / 255) as u8;
                        let g = ((color_g(bg) as u32 * inv_a + dot_rgb.1 as u32 * a_u) / 255) as u8;
                        let b = ((color_b(bg) as u32 * inv_a + dot_rgb.2 as u32 * a_u) / 255) as u8;
                        self.pixels[idx] = rgba(r, g, b, 255);
                    }
                }
            }
        }

        let line_alpha = if is_dark { 14 } else { 10 };
        let line_col = rgba(dot_rgb.0, dot_rgb.1, dot_rgb.2, line_alpha);
        self.draw_line(24.0, 114.0, w - 24.0, 114.0, 1.0, line_col);
    }

    fn blend_pixel(&mut self, x: i32, y: i32, src_color: u32, coverage: f32) {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return;
        }
        let alpha_f = (color_a(src_color) as f32 / 255.0) * coverage;
        if alpha_f <= 0.001 {
            return;
        }

        let idx = (y as u32 * self.width + x as u32) as usize;
        if alpha_f >= 0.999 {
            self.pixels[idx] = src_color | 0xFF000000;
            return;
        }

        let dst = self.pixels[idx];
        let dr = color_r(dst) as f32;
        let dg = color_g(dst) as f32;
        let db = color_b(dst) as f32;

        let sr = color_r(src_color) as f32;
        let sg = color_g(src_color) as f32;
        let sb = color_b(src_color) as f32;

        let inv = 1.0 - alpha_f;
        let out_r = (sr * alpha_f + dr * inv).round() as u8;
        let out_g = (sg * alpha_f + dg * inv).round() as u8;
        let out_b = (sb * alpha_f + db * inv).round() as u8;

        self.pixels[idx] = rgba(out_r, out_g, out_b, 255);
    }

    fn fill_rounded_rect(&mut self, x: f32, y: f32, w: f32, h: f32, radius: f32, color: u32) {
        let r = radius.min(w * 0.5).min(h * 0.5).max(0.0);
        let cx = x + w * 0.5;
        let cy = y + h * 0.5;
        let bx = w * 0.5 - r;
        let by = h * 0.5 - r;

        let min_x = (x.floor() as i32).max(0).min(self.width as i32);
        let min_y = (y.floor() as i32).max(0).min(self.height as i32);
        let max_x = ((x + w).ceil() as i32).max(0).min(self.width as i32);
        let max_y = ((y + h).ceil() as i32).max(0).min(self.height as i32);

        for py in min_y..max_y {
            let py_f = py as f32 + 0.5;
            let dy = (py_f - cy).abs() - by;
            for px in min_x..max_x {
                let px_f = px as f32 + 0.5;
                let dx = (px_f - cx).abs() - bx;
                let dist = if dx > 0.0 && dy > 0.0 {
                    (dx * dx + dy * dy).sqrt() - r
                } else {
                    dx.max(dy) - r
                };
                let cov = (0.5 - dist).clamp(0.0, 1.0);
                if cov > 0.0 {
                    self.blend_pixel(px, py, color, cov);
                }
            }
        }
    }

    fn stroke_rounded_rect(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        radius: f32,
        stroke_width: f32,
        color: u32,
    ) {
        let half_stroke = stroke_width * 0.5;
        let r = radius.min(w * 0.5).min(h * 0.5).max(0.0);
        let cx = x + w * 0.5;
        let cy = y + h * 0.5;
        let bx = w * 0.5 - r;
        let by = h * 0.5 - r;

        let pad = half_stroke + 1.0;
        let min_x = ((x - pad).floor() as i32).max(0).min(self.width as i32);
        let min_y = ((y - pad).floor() as i32).max(0).min(self.height as i32);
        let max_x = ((x + w + pad).ceil() as i32).max(0).min(self.width as i32);
        let max_y = ((y + h + pad).ceil() as i32).max(0).min(self.height as i32);

        for py in min_y..max_y {
            let py_f = py as f32 + 0.5;
            let dy = (py_f - cy).abs() - by;
            for px in min_x..max_x {
                let px_f = px as f32 + 0.5;
                let dx = (px_f - cx).abs() - bx;
                let outer_dist = if dx > 0.0 && dy > 0.0 {
                    (dx * dx + dy * dy).sqrt() - r
                } else {
                    dx.max(dy) - r
                };
                let dist = outer_dist.abs() - half_stroke;
                let cov = (0.5 - dist).clamp(0.0, 1.0);
                if cov > 0.0 {
                    self.blend_pixel(px, py, color, cov);
                }
            }
        }
    }

    fn draw_line(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, stroke_width: f32, color: u32) {
        let half_w = stroke_width * 0.5;
        let pad = half_w + 1.0;
        let min_x = ((x0.min(x1) - pad).floor() as i32).max(0).min(self.width as i32);
        let min_y = ((y0.min(y1) - pad).floor() as i32).max(0).min(self.height as i32);
        let max_x = ((x0.max(x1) + pad).ceil() as i32).max(0).min(self.width as i32);
        let max_y = ((y0.max(y1) + pad).ceil() as i32).max(0).min(self.height as i32);

        let dx = x1 - x0;
        let dy = y1 - y0;
        let len_sq = dx * dx + dy * dy;

        for py in min_y..max_y {
            let py_f = py as f32 + 0.5;
            for px in min_x..max_x {
                let px_f = px as f32 + 0.5;
                let t = if len_sq < 0.0001 {
                    0.0
                } else {
                    let dot = (px_f - x0) * dx + (py_f - y0) * dy;
                    (dot / len_sq).clamp(0.0, 1.0)
                };
                let qx = x0 + t * dx;
                let qy = y0 + t * dy;
                let dist = ((px_f - qx).powi(2) + (py_f - qy).powi(2)).sqrt() - half_w;
                let cov = (0.5 - dist).clamp(0.0, 1.0);
                if cov > 0.0 {
                    self.blend_pixel(px, py, color, cov);
                }
            }
        }
    }

    fn fill_circle(&mut self, cx: f32, cy: f32, radius: f32, color: u32) {
        let min_x = ((cx - radius).floor() as i32).max(0).min(self.width as i32);
        let min_y = ((cy - radius).floor() as i32).max(0).min(self.height as i32);
        let max_x = ((cx + radius).ceil() as i32).max(0).min(self.width as i32);
        let max_y = ((cy + radius).ceil() as i32).max(0).min(self.height as i32);

        for py in min_y..max_y {
            let dy = py as f32 + 0.5 - cy;
            for px in min_x..max_x {
                let dx = px as f32 + 0.5 - cx;
                let dist = (dx * dx + dy * dy).sqrt() - radius;
                let cov = (0.5 - dist).clamp(0.0, 1.0);
                if cov > 0.0 {
                    self.blend_pixel(px, py, color, cov);
                }
            }
        }
    }

    fn stroke_circle(&mut self, cx: f32, cy: f32, radius: f32, stroke_width: f32, color: u32) {
        let half = stroke_width * 0.5;
        let pad = half + 1.0;
        let min_x = ((cx - radius - pad).floor() as i32).max(0).min(self.width as i32);
        let min_y = ((cy - radius - pad).floor() as i32).max(0).min(self.height as i32);
        let max_x = ((cx + radius + pad).ceil() as i32).max(0).min(self.width as i32);
        let max_y = ((cy + radius + pad).ceil() as i32).max(0).min(self.height as i32);

        for py in min_y..max_y {
            let dy = py as f32 + 0.5 - cy;
            for px in min_x..max_x {
                let dx = px as f32 + 0.5 - cx;
                let dist = ((dx * dx + dy * dy).sqrt() - radius).abs() - half;
                let cov = (0.5 - dist).clamp(0.0, 1.0);
                if cov > 0.0 {
                    self.blend_pixel(px, py, color, cov);
                }
            }
        }
    }

    fn draw_rgba_icon(&mut self, x: i32, y: i32, w: u32, h: u32, rgba_data: &[u8]) {
        for iy in 0..h {
            let py = y + iy as i32;
            if py < 0 || py >= self.height as i32 {
                continue;
            }
            for ix in 0..w {
                let px = x + ix as i32;
                if px < 0 || px >= self.width as i32 {
                    continue;
                }
                let offset = ((iy * w + ix) * 4) as usize;
                if offset + 3 >= rgba_data.len() {
                    break;
                }
                let r = rgba_data[offset];
                let g = rgba_data[offset + 1];
                let b = rgba_data[offset + 2];
                let a = rgba_data[offset + 3];
                if a > 0 {
                    self.blend_pixel(px, py, rgba(r, g, b, a), 1.0);
                }
            }
        }
    }

    fn draw_text(&mut self, is_bold: bool, font_size: f32, x: f32, y: f32, text: &str, color: u32) {
        if text.is_empty() {
            return;
        }
        let font = if is_bold { &self.font_bold } else { &self.font_reg };
        let eff_size = font_size.round();
        let scale = PxScale::from(eff_size);
        let scaled = font.as_scaled(scale);

        let mut x_pos = x.round();
        let baseline_y = (y + scaled.ascent()).round();
        let mut prev: Option<GlyphId> = None;

        let alpha_base = color_a(color) as f32 / 255.0;
        let sr = color_r(color) as f32;
        let sg = color_g(color) as f32;
        let sb = color_b(color) as f32;
        let luma = (0.299 * sr + 0.587 * sg + 0.114 * sb).round() as u32;

        for ch in text.chars() {
            let glyph_id = font.glyph_id(ch);
            if let Some(prev_id) = prev {
                x_pos += scaled.kern(prev_id, glyph_id);
            }
            let glyph = glyph_id.with_scale_and_position(scale, ab_glyph::point(x_pos, baseline_y));
            if let Some(outlined) = font.outline_glyph(glyph) {
                let bounds = outlined.px_bounds();
                let width = self.width;
                let height = self.height;
                let pixels = &mut self.pixels;

                outlined.draw(|gx, gy, cov| {
                    if cov <= 0.005 {
                        return;
                    }
                    let cov_adj = if luma > 128 {
                        cov.powf(0.68).min(1.0)
                    } else {
                        cov.powf(0.76).min(1.0)
                    };
                    let alpha_f = alpha_base * cov_adj;
                    if alpha_f <= 0.001 {
                        return;
                    }

                    let px = bounds.min.x as i32 + gx as i32;
                    let py = bounds.min.y as i32 + gy as i32;
                    if px >= 0 && py >= 0 && px < width as i32 && py < height as i32 {
                        let idx = (py as u32 * width + px as u32) as usize;
                        if alpha_f >= 0.999 {
                            pixels[idx] = color | 0xFF000000;
                        } else {
                            let dst = pixels[idx];
                            let dr = color_r(dst) as f32;
                            let dg = color_g(dst) as f32;
                            let db = color_b(dst) as f32;
                            let inv = 1.0 - alpha_f;
                            let out_r = (sr * alpha_f + dr * inv).round() as u8;
                            let out_g = (sg * alpha_f + dg * inv).round() as u8;
                            let out_b = (sb * alpha_f + db * inv).round() as u8;
                            pixels[idx] = rgba(out_r, out_g, out_b, 255);
                        }
                    }
                });
            }
            x_pos += scaled.h_advance(glyph_id);
            prev = Some(glyph_id);
        }
    }

    fn measure_text(&self, is_bold: bool, font_size: f32, text: &str) -> (f32, f32) {
        if text.is_empty() {
            return (0.0, font_size);
        }
        let font = if is_bold { &self.font_bold } else { &self.font_reg };
        let eff_size = font_size.round();
        let scale = PxScale::from(eff_size);
        let scaled = font.as_scaled(scale);

        let mut width = 0.0f32;
        let mut prev: Option<GlyphId> = None;

        for ch in text.chars() {
            let glyph_id = font.glyph_id(ch);
            if let Some(prev_id) = prev {
                width += scaled.kern(prev_id, glyph_id);
            }
            width += scaled.h_advance(glyph_id);
            prev = Some(glyph_id);
        }
        let height = scaled.ascent() - scaled.descent();
        (width, height)
    }

    fn draw_text_centered(&mut self, is_bold: bool, font_size: f32, cx: f32, cy: f32, text: &str, color: u32) {
        let (w, h) = self.measure_text(is_bold, font_size, text);
        self.draw_text(is_bold, font_size, cx - w * 0.5, cy - h * 0.5, text, color);
    }
}

// Kurulum Sihirbazi Durumu
#[derive(PartialEq, Eq, Clone, Copy)]
enum SetupStep {
    Ready,
    Installing,
    Done,
}

struct SharedInstallState {
    progress: f32,
    status_text: String,
    finished: bool,
    error: Option<String>,
}

struct SetupState {
    canvas: SetupCanvas,
    step: SetupStep,
    create_desktop_shortcut: bool,
    create_start_menu_shortcut: bool,
    launch_on_exit: bool,
    display_progress: f32,
    status_text: String,
    install_dir: PathBuf,
    mouse_x: f32,
    mouse_y: f32,
    shared_install: Arc<Mutex<SharedInstallState>>,
}

#[cfg(windows)]
fn get_saved_install_location() -> Option<PathBuf> {
    unsafe {
        let subkey_w: Vec<u16> = "Software\\maziTunnel\0".encode_utf16().collect();
        let val_w: Vec<u16> = "InstallLocation\0".encode_utf16().collect();
        let mut hkey: windows_sys::Win32::System::Registry::HKEY = std::mem::zeroed();
        if RegOpenKeyExW(HKEY_CURRENT_USER, subkey_w.as_ptr(), 0, KEY_READ, &mut hkey) == 0 && !hkey.is_null() {
            let mut val_type: u32 = 0;
            let mut data_size: u32 = 0;
            if RegQueryValueExW(hkey, val_w.as_ptr(), std::ptr::null_mut(), &mut val_type, std::ptr::null_mut(), &mut data_size) == 0
                && val_type == REG_SZ
                && data_size > 0
            {
                let mut buf = vec![0u8; data_size as usize];
                if RegQueryValueExW(hkey, val_w.as_ptr(), std::ptr::null_mut(), &mut val_type, buf.as_mut_ptr(), &mut data_size) == 0 {
                    RegCloseKey(hkey);
                    let u16_len = (data_size as usize) / 2;
                    let u16_slice = std::slice::from_raw_parts(buf.as_ptr() as *const u16, u16_len);
                    let end = u16_slice.iter().position(|&c| c == 0).unwrap_or(u16_slice.len());
                    let s = String::from_utf16_lossy(&u16_slice[..end]).trim().to_string();
                    if !s.is_empty() {
                        return Some(PathBuf::from(s));
                    }
                }
            }
            RegCloseKey(hkey);
        }
    }
    None
}

#[cfg(not(windows))]
fn get_saved_install_location() -> Option<PathBuf> {
    None
}

#[cfg(windows)]
fn browse_for_install_folder(hwnd: HWND, _current: &Path) -> Option<PathBuf> {
    unsafe {
        let title_w: Vec<u16> = "maziTunnel Kurulum Klasörünü Seçin\0".encode_utf16().collect();
        let mut bi: BROWSEINFOW = std::mem::zeroed();
        bi.hwndOwner = hwnd;
        bi.lpszTitle = title_w.as_ptr();
        bi.ulFlags = 0x0001 | 0x0040; // BIF_RETURNONLYFSDIRS | BIF_NEWDIALOGSTYLE

        let pidl = SHBrowseForFolderW(&bi);
        if pidl.is_null() {
            return None;
        }

        let mut path_buf = [0u16; 1024];
        let ok = SHGetPathFromIDListW(pidl, path_buf.as_mut_ptr());
        CoTaskMemFree(pidl as _);

        if ok != 0 {
            let len = path_buf.iter().position(|&c| c == 0).unwrap_or(path_buf.len());
            if len > 0 {
                let chosen = PathBuf::from(String::from_utf16_lossy(&path_buf[..len]));
                let final_path = if chosen
                    .file_name()
                    .map(|n| n.to_string_lossy().eq_ignore_ascii_case("maziTunnel"))
                    .unwrap_or(false)
                {
                    chosen
                } else {
                    chosen.join("maziTunnel")
                };
                return Some(final_path);
            }
        }
    }
    None
}

#[cfg(not(windows))]
fn browse_for_install_folder(_hwnd: (), _current: &Path) -> Option<PathBuf> {
    None
}

impl SetupState {
    fn new() -> Self {
        let install_dir = {
            #[cfg(windows)]
            {
                get_saved_install_location().unwrap_or_else(|| {
                    let local_appdata = env::var("LOCALAPPDATA")
                        .map(PathBuf::from)
                        .unwrap_or_else(|_| PathBuf::from("C:\\Users\\Default\\AppData\\Local"));
                    local_appdata.join("maziTunnel")
                })
            }
            #[cfg(not(windows))]
            {
                let local_appdata = env::var("LOCALAPPDATA")
                    .map(PathBuf::from)
                    .unwrap_or_else(|_| PathBuf::from("C:\\Users\\Default\\AppData\\Local"));
                local_appdata.join("maziTunnel")
            }
        };

        let shared_install = Arc::new(Mutex::new(SharedInstallState {
            progress: 0.0,
            status_text: "Kuruluma hazır.".to_string(),
            finished: false,
            error: None,
        }));

        Self {
            canvas: SetupCanvas::new(WIN_W as u32, WIN_H as u32),
            step: SetupStep::Ready,
            create_desktop_shortcut: true,
            create_start_menu_shortcut: true,
            launch_on_exit: true,
            display_progress: 0.0,
            status_text: "Kuruluma hazır.".to_string(),
            install_dir,
            mouse_x: -1.0,
            mouse_y: -1.0,
            shared_install,
        }
    }

    fn render(&mut self) {
        // Koyu Zemin: #0c0c0e
        self.canvas.clear(rgba(12, 12, 14, 255));
        self.canvas.draw_architectural_backdrop(true);

        let mx = self.mouse_x;
        let my = self.mouse_y;

        match self.step {
            SetupStep::Ready => {
                // 1. Ust Baslik Karti (X: 24, Y: 20, W: 452, H: 84)
                let h_card_x = 24.0;
                let h_card_y = 20.0;
                let h_card_w = 452.0;
                let h_card_h = 84.0;

                self.canvas.fill_rounded_rect(h_card_x, h_card_y, h_card_w, h_card_h, 12.0, rgba(20, 20, 23, 255));
                self.canvas.stroke_rounded_rect(h_card_x, h_card_y, h_card_w, h_card_h, 12.0, 1.0, rgba(35, 35, 41, 255));

                self.canvas.draw_rgba_icon(38, 34, 56, 56, SETUP_ICON_56);
                self.canvas.draw_text(true, 22.0, 108.0, 34.0, "maziTunnel Kurulum Sihirbazı", rgba(244, 244, 246, 255));
                self.canvas.draw_text(false, 15.0, 108.0, 64.0, "Bilgisayarınıza maziTunnel v2.0.1 kurulacak.", rgba(161, 161, 170, 255));

                // 2. Kurulum Konumu Karti (X: 24, Y: 122, W: 452, H: 62)
                let loc_x = 24.0;
                let loc_y = 122.0;
                let loc_w = 452.0;
                let loc_h = 62.0;

                self.canvas.fill_rounded_rect(loc_x, loc_y, loc_w, loc_h, 10.0, rgba(20, 20, 23, 255));
                self.canvas.stroke_rounded_rect(loc_x, loc_y, loc_w, loc_h, 10.0, 1.0, rgba(35, 35, 41, 255));

                self.canvas.draw_text(true, 13.5, loc_x + 16.0, loc_y + 11.0, "KURULUM KONUMU", rgba(142, 142, 153, 255));

                let path_raw = self.install_dir.to_string_lossy();
                let mut path_display = path_raw.to_string();
                let max_path_w = 325.0;
                if self.canvas.measure_text(false, 14.5, &path_display).0 > max_path_w {
                    while path_display.len() > 10
                        && self.canvas.measure_text(false, 14.5, &format!("{}...", path_display)).0 > max_path_w
                    {
                        path_display.pop();
                    }
                    path_display = format!("{}...", path_display);
                }
                self.canvas.draw_text(false, 14.5, loc_x + 16.0, loc_y + 33.0, &path_display, rgba(212, 212, 216, 255));

                // Degistir... Butonu (X: 368, Y: 135, W: 96, H: 36, Radius: 8)
                let btn_x = 368.0;
                let btn_y = 135.0;
                let btn_w = 96.0;
                let btn_h = 36.0;
                let btn_hover = mx >= 368.0 && mx <= 464.0 && my >= 135.0 && my <= 171.0;

                let btn_bg = if btn_hover { rgba(42, 42, 50, 255) } else { rgba(30, 30, 36, 255) };
                let btn_border = if btn_hover { rgba(75, 75, 88, 255) } else { rgba(52, 52, 62, 255) };
                let btn_text_c = if btn_hover { rgba(255, 255, 255, 255) } else { rgba(244, 244, 246, 255) };

                self.canvas.fill_rounded_rect(btn_x, btn_y, btn_w, btn_h, 8.0, btn_bg);
                self.canvas.stroke_rounded_rect(btn_x, btn_y, btn_w, btn_h, 8.0, 1.0, btn_border);
                self.canvas.draw_text_centered(true, 13.5, btn_x + btn_w * 0.5, btn_y + btn_h * 0.5, "Değiştir", btn_text_c);

                // 3. Etkilesimli Onay Kutulari (Checkboxes) - Monokrom B&W/Zinc
                // Checkbox 1: Masaustu kisayolu
                let chk1_x = 28.0;
                let chk1_y = 202.0;
                let chk1_hover = mx >= 24.0 && mx <= 320.0 && my >= 198.0 && my <= 228.0;

                if self.create_desktop_shortcut {
                    self.canvas.fill_rounded_rect(chk1_x, chk1_y, 22.0, 22.0, 5.0, rgba(244, 244, 246, 255));
                    self.canvas.draw_line(chk1_x + 5.5, chk1_y + 11.5, chk1_x + 9.5, chk1_y + 16.0, 2.2, rgba(12, 12, 14, 255));
                    self.canvas.draw_line(chk1_x + 9.5, chk1_y + 16.0, chk1_x + 16.5, chk1_y + 6.5, 2.2, rgba(12, 12, 14, 255));
                } else {
                    self.canvas.fill_rounded_rect(chk1_x, chk1_y, 22.0, 22.0, 5.0, rgba(24, 24, 28, 255));
                    let border_c = if chk1_hover { rgba(90, 90, 105, 255) } else { rgba(55, 55, 65, 255) };
                    self.canvas.stroke_rounded_rect(chk1_x, chk1_y, 22.0, 22.0, 5.0, 1.2, border_c);
                }
                let chk1_text_c = if chk1_hover { rgba(255, 255, 255, 255) } else { rgba(244, 244, 246, 255) };
                self.canvas.draw_text(false, 15.0, chk1_x + 30.0, chk1_y + 2.0, "Masaüstü kısayolu oluştur", chk1_text_c);

                // Checkbox 2: Baslat menusu kisayolu
                let chk2_x = 28.0;
                let chk2_y = 236.0;
                let chk2_hover = mx >= 24.0 && mx <= 340.0 && my >= 232.0 && my <= 262.0;

                if self.create_start_menu_shortcut {
                    self.canvas.fill_rounded_rect(chk2_x, chk2_y, 22.0, 22.0, 5.0, rgba(244, 244, 246, 255));
                    self.canvas.draw_line(chk2_x + 5.5, chk2_y + 11.5, chk2_x + 9.5, chk2_y + 16.0, 2.2, rgba(12, 12, 14, 255));
                    self.canvas.draw_line(chk2_x + 9.5, chk2_y + 16.0, chk2_x + 16.5, chk2_y + 6.5, 2.2, rgba(12, 12, 14, 255));
                } else {
                    self.canvas.fill_rounded_rect(chk2_x, chk2_y, 22.0, 22.0, 5.0, rgba(24, 24, 28, 255));
                    let border_c = if chk2_hover { rgba(90, 90, 105, 255) } else { rgba(55, 55, 65, 255) };
                    self.canvas.stroke_rounded_rect(chk2_x, chk2_y, 22.0, 22.0, 5.0, 1.2, border_c);
                }
                let chk2_text_c = if chk2_hover { rgba(255, 255, 255, 255) } else { rgba(244, 244, 246, 255) };
                self.canvas.draw_text(false, 15.0, chk2_x + 30.0, chk2_y + 2.0, "Başlat menüsü kısayolu oluştur", chk2_text_c);

                // 4. Alt Bar (Y: 294)
                let cancel_x = 24.0;
                let cancel_y = 294.0;
                let cancel_w = 130.0;
                let cancel_h = 44.0;
                let cancel_hover = mx >= cancel_x && mx <= cancel_x + cancel_w && my >= cancel_y && my <= cancel_y + cancel_h;

                let cancel_bg = if cancel_hover { rgba(38, 38, 46, 255) } else { rgba(26, 26, 32, 255) };
                let cancel_border = if cancel_hover { rgba(65, 65, 78, 255) } else { rgba(45, 45, 55, 255) };
                self.canvas.fill_rounded_rect(cancel_x, cancel_y, cancel_w, cancel_h, 10.0, cancel_bg);
                self.canvas.stroke_rounded_rect(cancel_x, cancel_y, cancel_w, cancel_h, 10.0, 1.0, cancel_border);
                self.canvas.draw_text_centered(true, 15.5, cancel_x + cancel_w * 0.5, cancel_y + cancel_h * 0.5, "İptal", rgba(212, 212, 216, 255));

                let start_x = 170.0;
                let start_y = 294.0;
                let start_w = 306.0;
                let start_h = 44.0;
                let start_hover = mx >= start_x && mx <= start_x + start_w && my >= start_y && my <= start_y + start_h;

                let start_bg = if start_hover { rgba(255, 255, 255, 255) } else { rgba(244, 244, 246, 255) };
                self.canvas.fill_rounded_rect(start_x, start_y, start_w, start_h, 10.0, start_bg);
                self.canvas.draw_text_centered(true, 15.5, start_x + start_w * 0.5, start_y + start_h * 0.5, "Kurulumu Başlat", rgba(12, 12, 14, 255));
            }
            SetupStep::Installing => {
                // 1. Ust Baslik Karti
                let h_card_x = 24.0;
                let h_card_y = 20.0;
                let h_card_w = 452.0;
                let h_card_h = 84.0;

                self.canvas.fill_rounded_rect(h_card_x, h_card_y, h_card_w, h_card_h, 12.0, rgba(20, 20, 23, 255));
                self.canvas.stroke_rounded_rect(h_card_x, h_card_y, h_card_w, h_card_h, 12.0, 1.0, rgba(35, 35, 41, 255));

                self.canvas.draw_rgba_icon(38, 34, 56, 56, SETUP_ICON_56);
                self.canvas.draw_text(true, 22.0, 108.0, 34.0, "maziTunnel Kuruluyor...", rgba(244, 244, 246, 255));
                self.canvas.draw_text(false, 15.0, 108.0, 64.0, "Lütfen dosyalar sisteminize kopyalanırken bekleyin.", rgba(161, 161, 170, 255));

                // 2. Orta Ilerleme Karti (X: 24, Y: 122, W: 452, H: 154)
                let p_card_x = 24.0;
                let p_card_y = 122.0;
                let p_card_w = 452.0;
                let p_card_h = 154.0;

                self.canvas.fill_rounded_rect(p_card_x, p_card_y, p_card_w, p_card_h, 12.0, rgba(20, 20, 23, 255));
                self.canvas.stroke_rounded_rect(p_card_x, p_card_y, p_card_w, p_card_h, 12.0, 1.0, rgba(35, 35, 41, 255));

                // Durum ve Yuzde Metni
                self.canvas.draw_text(false, 15.0, p_card_x + 20.0, p_card_y + 26.0, &self.status_text, rgba(244, 244, 246, 255));

                let p = self.display_progress.clamp(0.0, 1.0);
                let percent_str = format!("{:.0}%", p * 100.0);
                let (pct_w, _) = self.canvas.measure_text(true, 15.0, &percent_str);
                self.canvas.draw_text(true, 15.0, p_card_x + p_card_w - 20.0 - pct_w, p_card_y + 26.0, &percent_str, rgba(161, 161, 170, 255));

                // Ilerleme Cubugu - Monokrom Beyaz/Zinc Dolgu
                let pbar_x = p_card_x + 20.0;
                let pbar_y = p_card_y + 62.0;
                let pbar_w = p_card_w - 40.0;
                let pbar_h = 14.0;

                self.canvas.fill_rounded_rect(pbar_x, pbar_y, pbar_w, pbar_h, 7.0, rgba(28, 28, 34, 255));
                if p > 0.005 {
                    let fill_w = (pbar_w * p).max(14.0).min(pbar_w);
                    self.canvas.fill_rounded_rect(pbar_x, pbar_y, fill_w, pbar_h, 7.0, rgba(244, 244, 246, 255));
                }

                // Alt Aciklama
                self.canvas.draw_text(false, 13.5, p_card_x + 20.0, p_card_y + 98.0, "Zero-Config P2P Tünel Sistemi kurulumu devam ediyor.", rgba(115, 115, 125, 255));

                // 3. Alt Bar (Devre Disi Buton)
                let wait_btn_x = 24.0;
                let wait_btn_y = 294.0;
                let wait_btn_w = 452.0;
                let wait_btn_h = 44.0;

                self.canvas.fill_rounded_rect(wait_btn_x, wait_btn_y, wait_btn_w, wait_btn_h, 10.0, rgba(26, 26, 32, 255));
                self.canvas.stroke_rounded_rect(wait_btn_x, wait_btn_y, wait_btn_w, wait_btn_h, 10.0, 1.0, rgba(40, 40, 48, 255));
                self.canvas.draw_text_centered(true, 15.5, wait_btn_x + wait_btn_w * 0.5, wait_btn_y + wait_btn_h * 0.5, "Lütfen Bekleyin...", rgba(115, 115, 125, 255));
            }
            SetupStep::Done => {
                // 1. Ust Baslik Karti - Monokrom Beyaz Baslik
                let h_card_x = 24.0;
                let h_card_y = 20.0;
                let h_card_w = 452.0;
                let h_card_h = 84.0;

                self.canvas.fill_rounded_rect(h_card_x, h_card_y, h_card_w, h_card_h, 12.0, rgba(20, 20, 23, 255));
                self.canvas.stroke_rounded_rect(h_card_x, h_card_y, h_card_w, h_card_h, 12.0, 1.0, rgba(35, 35, 41, 255));

                self.canvas.draw_rgba_icon(38, 34, 56, 56, SETUP_ICON_56);
                self.canvas.draw_text(true, 20.0, 108.0, 34.0, "maziTunnel Başarıyla Kuruldu!", rgba(244, 244, 246, 255));
                self.canvas.draw_text(false, 15.0, 108.0, 64.0, "Sürüm 2.0.1 kullanıma hazır.", rgba(161, 161, 170, 255));

                // 2. Ozet Karti (X: 24, Y: 122, W: 452, H: 148)
                let d_card_x = 24.0;
                let d_card_y = 122.0;
                let d_card_w = 452.0;
                let d_card_h = 148.0;

                self.canvas.fill_rounded_rect(d_card_x, d_card_y, d_card_w, d_card_h, 12.0, rgba(20, 20, 23, 255));
                self.canvas.stroke_rounded_rect(d_card_x, d_card_y, d_card_w, d_card_h, 12.0, 1.0, rgba(35, 35, 41, 255));

                // Basari Rozeti - Monokrom Zinc/Beyaz
                self.canvas.fill_circle(58.0, 156.0, 16.0, rgba(32, 32, 38, 255));
                self.canvas.stroke_circle(58.0, 156.0, 16.0, 1.5, rgba(244, 244, 246, 255));
                self.canvas.draw_line(51.0, 156.0, 56.0, 161.0, 2.0, rgba(244, 244, 246, 255));
                self.canvas.draw_line(56.0, 161.0, 66.0, 151.0, 2.0, rgba(244, 244, 246, 255));

                self.canvas.draw_text(true, 15.0, 88.0, 144.0, "Tüm uygulama dosyaları yerleştirildi.", rgba(244, 244, 246, 255));
                self.canvas.draw_text(false, 14.0, 88.0, 168.0, "Kayıt defteri ve başlat menüsü yapılandırıldı.", rgba(161, 161, 170, 255));

                // Ayirici cizgi
                self.canvas.draw_line(d_card_x + 16.0, 198.0, d_card_x + d_card_w - 16.0, 198.0, 1.0, rgba(35, 35, 41, 255));

                // Checkbox: Kurulum kapandiginda baslat - Monokrom
                let chk3_x = 38.0;
                let chk3_y = 215.0;
                let chk3_hover = mx >= 34.0 && mx <= 460.0 && my >= 210.0 && my <= 244.0;

                if self.launch_on_exit {
                    self.canvas.fill_rounded_rect(chk3_x, chk3_y, 22.0, 22.0, 5.0, rgba(244, 244, 246, 255));
                    self.canvas.draw_line(chk3_x + 5.5, chk3_y + 11.5, chk3_x + 9.5, chk3_y + 16.0, 2.2, rgba(12, 12, 14, 255));
                    self.canvas.draw_line(chk3_x + 9.5, chk3_y + 16.0, chk3_x + 16.5, chk3_y + 6.5, 2.2, rgba(12, 12, 14, 255));
                } else {
                    self.canvas.fill_rounded_rect(chk3_x, chk3_y, 22.0, 22.0, 5.0, rgba(24, 24, 28, 255));
                    let border_c = if chk3_hover { rgba(90, 90, 105, 255) } else { rgba(55, 55, 65, 255) };
                    self.canvas.stroke_rounded_rect(chk3_x, chk3_y, 22.0, 22.0, 5.0, 1.2, border_c);
                }
                let chk3_text_c = if chk3_hover { rgba(255, 255, 255, 255) } else { rgba(244, 244, 246, 255) };
                self.canvas.draw_text(false, 15.0, chk3_x + 30.0, chk3_y + 2.0, "Kurulum sihirbazı kapandığında maziTunnel'i başlat", chk3_text_c);

                // 3. Alt Buton ("Bitir") - Monokrom Beyaz Buton
                let finish_x = 24.0;
                let finish_y = 294.0;
                let finish_w = 452.0;
                let finish_h = 44.0;
                let finish_hover = mx >= finish_x && mx <= finish_x + finish_w && my >= finish_y && my <= finish_y + finish_h;

                let finish_bg = if finish_hover { rgba(255, 255, 255, 255) } else { rgba(244, 244, 246, 255) };
                self.canvas.fill_rounded_rect(finish_x, finish_y, finish_w, finish_h, 10.0, finish_bg);
                self.canvas.draw_text_centered(true, 15.5, finish_x + finish_w * 0.5, finish_y + finish_h * 0.5, "Bitir", rgba(12, 12, 14, 255));
            }
        }
    }
}

// Guvenli Islem Sonlandirici (Asla Kendi PID'sini Kapatmaz!)
fn kill_other_mazitunnel_processes() {
    let my_pid = std::process::id();
    #[cfg(windows)]
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot != -1 as isize as HANDLE {
            let mut entry: PROCESSENTRY32W = std::mem::zeroed();
            entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;

            if Process32FirstW(snapshot, &mut entry) != 0 {
                loop {
                    let exe_name = String::from_utf16_lossy(
                        &entry.szExeFile[..entry
                            .szExeFile
                            .iter()
                            .position(|&c| c == 0)
                            .unwrap_or(entry.szExeFile.len())],
                    );

                    if entry.th32ProcessID != my_pid && exe_name.eq_ignore_ascii_case("maziTunnel.exe") {
                        let proc_handle = OpenProcess(PROCESS_TERMINATE, 0, entry.th32ProcessID);
                        if !proc_handle.is_null() {
                            TerminateProcess(proc_handle, 1);
                            CloseHandle(proc_handle);
                        }
                    }

                    if Process32NextW(snapshot, &mut entry) == 0 {
                        break;
                    }
                }
            }
            CloseHandle(snapshot);
        }
    }
}

// Semantic version karsilastirici: remote > current ise true doner
fn is_newer_version(current: &str, remote: &str) -> bool {
    let parse_ver = |v: &str| -> (u32, u32, u32) {
        let clean = v.trim_start_matches('v').trim();
        let parts: Vec<&str> = clean.split('.').collect();
        let major = parts.get(0).and_then(|s| s.parse().ok()).unwrap_or(0);
        let minor = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
        let patch = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
        (major, minor, patch)
    };
    parse_ver(remote) > parse_ver(current)
}

// R2 uzerinden en guncel istemci binary'sini indiren cevrimici yukleyici
fn fetch_latest_client_from_r2() -> Option<Vec<u8>> {
    let mut cmd = Command::new("curl.exe");
    cmd.args([
        "-sL",
        "--connect-timeout",
        "3",
        "--max-time",
        "6",
        "https://updates.mazidev.xyz/version.json",
    ]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }
    let output = cmd.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);

    let extract_json_str = |json: &str, key: &str| -> Option<String> {
        let pattern = format!("\"{}\"", key);
        let pos = json.find(&pattern)?;
        let after_key = &json[pos + pattern.len()..];
        let colon = after_key.find(':')?;
        let after_colon = after_key[colon + 1..].trim_start();
        if !after_colon.starts_with('"') {
            return None;
        }
        let after_quote = &after_colon[1..];
        let end_quote = after_quote.find('"')?;
        Some(after_quote[..end_quote].to_string())
    };

    let remote_ver = extract_json_str(&text, "version")?;
    if !is_newer_version(env!("CARGO_PKG_VERSION"), &remote_ver) {
        return None;
    }

    let target_url = extract_json_str(&text, "clientUrl")
        .or_else(|| extract_json_str(&text, "client_url"))?;

    if !target_url.starts_with("https://") {
        return None;
    }

    let temp_file = env::temp_dir().join(format!("mazitunneldl{}.exe", std::process::id()));
    let temp_str = temp_file.to_string_lossy().to_string();

    let mut dl_cmd = Command::new("curl.exe");
    dl_cmd.args([
        "-sL",
        "--connect-timeout",
        "4",
        "--max-time",
        "25",
        "-o",
        &temp_str,
        &target_url,
    ]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        dl_cmd.creation_flags(0x08000000);
    }
    let dl_status = dl_cmd.status().ok()?;
    if !dl_status.success() {
        let _ = fs::remove_file(&temp_file);
        return None;
    }

    let bytes = fs::read(&temp_file).ok();
    let _ = fs::remove_file(&temp_file);

    if let Some(b) = bytes {
        if b.len() > 500_000 && b.starts_with(b"MZ") {
            return Some(b);
        }
    }
    None
}

// GZIP sikistirmali veya ham ikili veriyi cozen yardimci
fn decode_payload(raw: &[u8]) -> Vec<u8> {
    if raw.len() >= 2 && raw[0] == 0x1f && raw[1] == 0x8b {
        let mut decoder = GzDecoder::new(raw);
        let mut out = Vec::new();
        if decoder.read_to_end(&mut out).is_ok() && !out.is_empty() {
            return out;
        }
    }
    raw.to_vec()
}

#[cfg(windows)]
fn get_special_folder(csidl: i32) -> Option<PathBuf> {
    unsafe {
        let mut buf = [0u16; 260];
        let hr = SHGetFolderPathW(
            0 as _,
            csidl,
            0 as _,
            0,
            buf.as_mut_ptr(),
        );
        if hr == 0 {
            let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            if len > 0 {
                return Some(PathBuf::from(String::from_utf16_lossy(&buf[..len])));
            }
        }
    }
    None
}

#[cfg(not(windows))]
fn get_special_folder(_csidl: i32) -> Option<PathBuf> {
    None
}

// Windows PowerShell tabanli guvenilir kisayol olusturucu
fn create_windows_shortcut(target: &Path, shortcut: &Path, workdir: &Path, icon: &Path) {
    if let Some(parent) = shortcut.parent() {
        let _ = fs::create_dir_all(parent);
    }

    let script = "$ws = New-Object -ComObject WScript.Shell; \
                  $s = $ws.CreateShortcut($env:MT_LNK); \
                  $s.TargetPath = $env:MT_TARGET; \
                  $s.WorkingDirectory = $env:MT_WORKDIR; \
                  $s.IconLocation = \"$($env:MT_ICON),0\"; \
                  $s.Description = 'maziTunnel P2P Ağ Tüneli'; \
                  $s.Save();";

    let mut cmd = Command::new("powershell.exe");
    cmd.args(["-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-Command", script]);
    cmd.env("MT_LNK", shortcut);
    cmd.env("MT_TARGET", target);
    cmd.env("MT_ICON", icon);
    cmd.env("MT_WORKDIR", workdir);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }
    let _ = cmd.output();
}

// Windows Uninstall Registry Kaydi
fn register_uninstall_entry(install_dir: &Path, uninstall_exe: &Path, client_exe: &Path) {
    #[cfg(windows)]
    unsafe {
        let subkey = "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\maziTunnel\0"
            .encode_utf16()
            .collect::<Vec<u16>>();

        let mut hkey: windows_sys::Win32::System::Registry::HKEY = std::mem::zeroed();
        let res = RegCreateKeyW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            &mut hkey,
        );

        if res == 0 && !hkey.is_null() {
            let set_str = |key_name: &str, val_str: &str| {
                let kn = format!("{}\0", key_name).encode_utf16().collect::<Vec<u16>>();
                let vn = format!("{}\0", val_str).encode_utf16().collect::<Vec<u16>>();
                RegSetValueExW(
                    hkey,
                    kn.as_ptr(),
                    0,
                    REG_SZ,
                    vn.as_ptr() as _,
                    (vn.len() * 2) as u32,
                );
            };

            let set_dword = |key_name: &str, val: u32| {
                let kn = format!("{}\0", key_name).encode_utf16().collect::<Vec<u16>>();
                RegSetValueExW(
                    hkey,
                    kn.as_ptr(),
                    0,
                    REG_DWORD,
                    &val as *const _ as _,
                    std::mem::size_of::<u32>() as u32,
                );
            };

            set_str("DisplayName", "maziTunnel");
            set_str("DisplayVersion", "2.0.1");
            set_str("Publisher", "mazi0182");
            set_str("DisplayIcon", &format!("{},0", client_exe.to_string_lossy()));
            set_str("InstallLocation", &install_dir.to_string_lossy());
            set_str("UninstallString", &uninstall_exe.to_string_lossy());
            set_dword("NoModify", 1);
            set_dword("NoRepair", 1);

            RegCloseKey(hkey);
        }

        // HKCU\Software\maziTunnel altina da InstallLocation kaydi ekle
        let subkey_app = "Software\\maziTunnel\0"
            .encode_utf16()
            .collect::<Vec<u16>>();
        let mut hkey_app: windows_sys::Win32::System::Registry::HKEY = std::mem::zeroed();
        let res_app = RegCreateKeyW(
            HKEY_CURRENT_USER,
            subkey_app.as_ptr(),
            &mut hkey_app,
        );
        if res_app == 0 && !hkey_app.is_null() {
            let kn = "InstallLocation\0".encode_utf16().collect::<Vec<u16>>();
            let vn = format!("{}\0", install_dir.to_string_lossy())
                .encode_utf16()
                .collect::<Vec<u16>>();
            RegSetValueExW(
                hkey_app,
                kn.as_ptr(),
                0,
                REG_SZ,
                vn.as_ptr() as _,
                (vn.len() * 2) as u32,
            );
            RegCloseKey(hkey_app);
        }
    }
}

#[cfg(windows)]
unsafe fn blit_setup_canvas(hdc: HDC, pixels: &[u32]) {
    if hdc.is_null() {
        return;
    }
    let mut bmi: BITMAPINFO = std::mem::zeroed();
    bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    bmi.bmiHeader.biWidth = WIN_W as i32;
    bmi.bmiHeader.biHeight = -(WIN_H as i32);
    bmi.bmiHeader.biPlanes = 1;
    bmi.bmiHeader.biBitCount = 32;
    bmi.bmiHeader.biCompression = BI_RGB;

    SetDIBitsToDevice(
        hdc,
        0,
        0,
        WIN_W as u32,
        WIN_H as u32,
        0,
        0,
        0,
        WIN_H as u32,
        pixels.as_ptr() as _,
        &bmi,
        DIB_RGB_COLORS,
    );
}

// Windows UI Pencere Yordami
#[cfg(windows)]
unsafe extern "system" fn setup_window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut SetupState;

    match msg {
        WM_CREATE => {
            SetTimer(hwnd, 1, 16, None); // 60 FPS
            0
        }
        WM_TIMER => {
            if !state_ptr.is_null() {
                let state = &mut *state_ptr;

                let mut err_to_show: Option<String> = None;

                if state.step == SetupStep::Installing {
                    if let Ok(shared) = state.shared_install.lock() {
                        let target = shared.progress;
                        let diff = target - state.display_progress;
                        if diff > 0.001 {
                            state.display_progress += diff * 0.15;
                        } else {
                            state.display_progress = target;
                        }
                        state.status_text = shared.status_text.clone();

                        if let Some(err) = &shared.error {
                            err_to_show = Some(err.clone());
                            state.step = SetupStep::Done;
                        } else if shared.finished && state.display_progress >= 0.99 {
                            state.display_progress = 1.0;
                            state.step = SetupStep::Done;
                        }
                    }
                }

                state.render();

                let hdc = GetDC(hwnd);
                blit_setup_canvas(hdc, &state.canvas.pixels);
                ReleaseDC(hwnd, hdc);

                if let Some(err_msg) = err_to_show {
                    let title_w: Vec<u16> = "maziTunnel Kurulum Hatası\0".encode_utf16().collect();
                    let msg_w: Vec<u16> = format!("Kurulum sırasında bir hata oluştu:\n\n{}\0", err_msg)
                        .encode_utf16()
                        .collect();
                    MessageBoxW(hwnd, msg_w.as_ptr(), title_w.as_ptr(), MB_OK | MB_ICONERROR);
                }
            }
            0
        }
        WM_PAINT => {
            let mut ps: PAINTSTRUCT = std::mem::zeroed();
            let hdc = BeginPaint(hwnd, &mut ps);
            if !state_ptr.is_null() {
                let state = &mut *state_ptr;
                state.render();
                blit_setup_canvas(hdc, &state.canvas.pixels);
            }
            EndPaint(hwnd, &ps);
            0
        }
        WM_ERASEBKGND => 1,
        WM_MOUSEMOVE => {
            if !state_ptr.is_null() {
                let state = &mut *state_ptr;
                let mut client_rect: RECT = std::mem::zeroed();
                GetClientRect(hwnd, &mut client_rect);
                let client_w = (client_rect.right - client_rect.left).max(1) as f32;
                let client_h = (client_rect.bottom - client_rect.top).max(1) as f32;

                let raw_x = (lparam & 0xFFFF) as i16 as f32;
                let raw_y = ((lparam >> 16) & 0xFFFF) as i16 as f32;

                state.mouse_x = raw_x * (WIN_W as f32 / client_w);
                state.mouse_y = raw_y * (WIN_H as f32 / client_h);
            }
            0
        }
        WM_LBUTTONDOWN => {
            if !state_ptr.is_null() {
                let state = &mut *state_ptr;
                let mut client_rect: RECT = std::mem::zeroed();
                GetClientRect(hwnd, &mut client_rect);
                let client_w = (client_rect.right - client_rect.left).max(1) as f32;
                let client_h = (client_rect.bottom - client_rect.top).max(1) as f32;

                let raw_x = (lparam & 0xFFFF) as i16 as f32;
                let raw_y = ((lparam >> 16) & 0xFFFF) as i16 as f32;

                let mx = raw_x * (WIN_W as f32 / client_w);
                let my = raw_y * (WIN_H as f32 / client_h);

                match state.step {
                    SetupStep::Ready => {
                        // Kurulum Konumu Karti veya Degistir Butonu Tiklamasi (Buton: 368..464, 135..171; Kart: 24..476, 122..184)
                        if (mx >= 368.0 && mx <= 464.0 && my >= 135.0 && my <= 171.0)
                            || (mx >= 24.0 && mx <= 476.0 && my >= 122.0 && my <= 184.0)
                        {
                            #[cfg(windows)]
                            if let Some(new_path) = browse_for_install_folder(hwnd, &state.install_dir) {
                                state.install_dir = new_path;
                            }
                            return 0;
                        }

                        // Checkbox 1: Masaustu kisayolu (X: 24..320, Y: 198..228)
                        if mx >= 24.0 && mx <= 320.0 && my >= 198.0 && my <= 228.0 {
                            state.create_desktop_shortcut = !state.create_desktop_shortcut;
                            return 0;
                        }

                        // Checkbox 2: Baslat menusu kisayolu (X: 24..340, Y: 232..262)
                        if mx >= 24.0 && mx <= 340.0 && my >= 232.0 && my <= 262.0 {
                            state.create_start_menu_shortcut = !state.create_start_menu_shortcut;
                            return 0;
                        }

                        // Iptal Butonu (X: 24..154, Y: 294..338)
                        if mx >= 24.0 && mx <= 154.0 && my >= 294.0 && my <= 338.0 {
                            PostQuitMessage(0);
                            return 0;
                        }

                        // Kurulumu Baslat Butonu (X: 170..476, Y: 294..338)
                        if mx >= 170.0 && mx <= 476.0 && my >= 294.0 && my <= 338.0 {
                            state.step = SetupStep::Installing;
                            state.status_text = "Kuruluma başlanıyor...".to_string();
                            state.display_progress = 0.05;

                            let shared_clone = Arc::clone(&state.shared_install);
                            let install_dir = state.install_dir.clone();
                            let create_desktop = state.create_desktop_shortcut;
                            let create_start = state.create_start_menu_shortcut;

                            std::thread::spawn(move || {
                                // 1. Acik maziTunnel islemleri kapatiliyor
                                {
                                    let mut s = shared_clone.lock().unwrap();
                                    s.progress = 0.15;
                                    s.status_text = "Açık maziTunnel işlemleri kapatılıyor...".to_string();
                                }
                                kill_other_mazitunnel_processes();
                                std::thread::sleep(std::time::Duration::from_millis(150));

                                {
                                    let mut s = shared_clone.lock().unwrap();
                                    s.status_text = "En güncel sürüm kontrol ediliyor...".to_string();
                                    s.progress = 0.20;
                                }
                                let client_bytes = fetch_latest_client_from_r2()
                                    .unwrap_or_else(|| decode_payload(PAYLOAD_CLIENT));

                                // 2. Kurulum dizini hazirlaniyor
                                {
                                    let mut s = shared_clone.lock().unwrap();
                                    s.progress = 0.35;
                                    s.status_text = "Kurulum dizini hazırlanıyor...".to_string();
                                }
                                if let Err(e) = fs::create_dir_all(&install_dir) {
                                    let mut s = shared_clone.lock().unwrap();
                                    s.error = Some(format!("Kurulum dizini oluşturulamadı: {}", e));
                                    s.finished = true;
                                    return;
                                }
                                std::thread::sleep(std::time::Duration::from_millis(100));

                                // 3. Uygulama dosyalari cikartiliyor
                                {
                                    let mut s = shared_clone.lock().unwrap();
                                    s.progress = 0.65;
                                    s.status_text = "Uygulama dosyaları çıkartılıyor...".to_string();
                                }
                                let exe_client = install_dir.join("maziTunnel.exe");
                                let exe_uninstall = install_dir.join("Uninstall.exe");
                                let icon_path = install_dir.join("app.ico");

                                let uninstall_bytes = decode_payload(PAYLOAD_UNINSTALL);

                                if let Err(e) = fs::write(&exe_client, &client_bytes) {
                                    let mut s = shared_clone.lock().unwrap();
                                    s.error = Some(format!("maziTunnel.exe yazılamadı: {}", e));
                                    s.finished = true;
                                    return;
                                }
                                if let Err(e) = fs::write(&exe_uninstall, &uninstall_bytes) {
                                    let mut s = shared_clone.lock().unwrap();
                                    s.error = Some(format!("Uninstall.exe yazılamadı: {}", e));
                                    s.finished = true;
                                    return;
                                }
                                if let Err(e) = fs::write(&icon_path, APP_ICON_ICO) {
                                    let mut s = shared_clone.lock().unwrap();
                                    s.error = Some(format!("app.ico yazılamadı: {}", e));
                                    s.finished = true;
                                    return;
                                }
                                std::thread::sleep(std::time::Duration::from_millis(150));

                                // 4. Kisayollar ve Windows Kayit Defteri yapilandiriliyor
                                {
                                    let mut s = shared_clone.lock().unwrap();
                                    s.progress = 0.88;
                                    s.status_text = "Kısayollar ve Windows Kayıt Defteri yapılandırılıyor...".to_string();
                                }

                                if let Some(programs_dir) = get_special_folder(0x0002).or_else(|| {
                                    env::var("APPDATA").ok().map(|appdata| {
                                        PathBuf::from(appdata)
                                            .join("Microsoft")
                                            .join("Windows")
                                            .join("Start Menu")
                                            .join("Programs")
                                    })
                                }) {
                                    let legacy_dir = programs_dir.join("maziTunnel");
                                    let _ = fs::remove_dir_all(&legacy_dir);

                                    if create_start {
                                        let _ = fs::create_dir_all(&programs_dir);
                                        let shortcut_path = programs_dir.join("maziTunnel.lnk");
                                        create_windows_shortcut(&exe_client, &shortcut_path, &install_dir, &icon_path);
                                    }
                                }

                                if create_desktop {
                                    if let Some(desktop_dir) = get_special_folder(0x0010).or_else(|| {
                                        env::var("USERPROFILE").ok().map(|userprofile| PathBuf::from(userprofile).join("Desktop"))
                                    }) {
                                        let _ = fs::create_dir_all(&desktop_dir);
                                        let desktop_shortcut = desktop_dir.join("maziTunnel.lnk");
                                        create_windows_shortcut(&exe_client, &desktop_shortcut, &install_dir, &icon_path);
                                    }
                                }

                                #[cfg(windows)]
                                unsafe {
                                    SHChangeNotify(
                                        0x08000000,
                                        0x0000,
                                        std::ptr::null(),
                                        std::ptr::null(),
                                    );
                                }

                                register_uninstall_entry(&install_dir, &exe_uninstall, &exe_client);
                                std::thread::sleep(std::time::Duration::from_millis(100));

                                // 5. Tamamlandi
                                {
                                    let mut s = shared_clone.lock().unwrap();
                                    s.progress = 1.0;
                                    s.status_text = "Kurulum başarıyla tamamlandı!".to_string();
                                    s.finished = true;
                                }
                            });
                        }
                    }
                    SetupStep::Installing => {}
                    SetupStep::Done => {
                        // Checkbox: Kurulum kapandiginda baslat (X: 34..460, Y: 210..244)
                        if mx >= 34.0 && mx <= 460.0 && my >= 210.0 && my <= 244.0 {
                            state.launch_on_exit = !state.launch_on_exit;
                            return 0;
                        }

                        // Bitir Butonu (X: 24..476, Y: 294..338)
                        if mx >= 24.0 && mx <= 476.0 && my >= 294.0 && my <= 338.0 {
                            if state.launch_on_exit {
                                let exe_client = state.install_dir.join("maziTunnel.exe");
                                let mut cmd = Command::new(&exe_client);
                                #[cfg(windows)]
                                unsafe {
                                    AllowSetForegroundWindow(0xFFFFFFFF);
                                }
                                let _ = cmd.spawn();
                            }
                            PostQuitMessage(0);
                            return 0;
                        }
                    }
                }
            }
            0
        }
        WM_DESTROY => {
            KillTimer(hwnd, 1);
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let is_silent = args.iter().any(|a| a == "--silent" || a == "/S" || a == "-s");
    let is_updated = args.iter().any(|a| a == "--updated");

    if is_silent {
        kill_other_mazitunnel_processes();
        let install_dir = {
            #[cfg(windows)]
            {
                get_saved_install_location().unwrap_or_else(|| {
                    let local_appdata = env::var("LOCALAPPDATA")
                        .map(PathBuf::from)
                        .unwrap_or_else(|_| PathBuf::from("C:\\Users\\Default\\AppData\\Local"));
                    local_appdata.join("maziTunnel")
                })
            }
            #[cfg(not(windows))]
            {
                let local_appdata = env::var("LOCALAPPDATA")
                    .map(PathBuf::from)
                    .unwrap_or_else(|_| PathBuf::from("C:\\Users\\Default\\AppData\\Local"));
                local_appdata.join("maziTunnel")
            }
        };
        let _ = fs::create_dir_all(&install_dir);

        let exe_client = install_dir.join("maziTunnel.exe");
        let exe_uninstall = install_dir.join("Uninstall.exe");
        let icon_path = install_dir.join("app.ico");

        let client_bytes = if is_updated {
            decode_payload(PAYLOAD_CLIENT)
        } else {
            fetch_latest_client_from_r2().unwrap_or_else(|| decode_payload(PAYLOAD_CLIENT))
        };
        let uninstall_bytes = decode_payload(PAYLOAD_UNINSTALL);

        for _ in 0..20 {
            if fs::write(&exe_client, &client_bytes).is_ok() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(150));
        }
        let _ = fs::write(&exe_uninstall, &uninstall_bytes);
        let _ = fs::write(&icon_path, APP_ICON_ICO);

        if let Some(programs_dir) = get_special_folder(0x0002).or_else(|| {
            env::var("APPDATA").ok().map(|appdata| {
                PathBuf::from(appdata)
                    .join("Microsoft")
                    .join("Windows")
                    .join("Start Menu")
                    .join("Programs")
            })
        }) {
            let legacy_dir = programs_dir.join("maziTunnel");
            let _ = fs::remove_dir_all(&legacy_dir);
            let _ = fs::create_dir_all(&programs_dir);
            let shortcut_path = programs_dir.join("maziTunnel.lnk");
            create_windows_shortcut(&exe_client, &shortcut_path, &install_dir, &icon_path);
        }

        if let Some(desktop_dir) = get_special_folder(0x0010).or_else(|| {
            env::var("USERPROFILE").ok().map(|userprofile| PathBuf::from(userprofile).join("Desktop"))
        }) {
            let _ = fs::create_dir_all(&desktop_dir);
            let desktop_shortcut = desktop_dir.join("maziTunnel.lnk");
            create_windows_shortcut(&exe_client, &desktop_shortcut, &install_dir, &icon_path);
        }

        #[cfg(windows)]
        unsafe {
            SHChangeNotify(
                0x08000000,
                0x0000,
                std::ptr::null(),
                std::ptr::null(),
            );
        }
        register_uninstall_entry(&install_dir, &exe_uninstall, &exe_client);

        let mut cmd = Command::new(&exe_client);
        if is_updated {
            cmd.arg("--updated");
        }
        #[cfg(windows)]
        unsafe {
            AllowSetForegroundWindow(0xFFFFFFFF);
        }
        let _ = cmd.spawn();
        return;
    }

    #[cfg(windows)]
    unsafe {
        SetProcessDPIAware();

        let h_instance = GetModuleHandleW(std::ptr::null());
        let class_name = "maziTunnelSetupClass\0".encode_utf16().collect::<Vec<u16>>();
        let window_title = "maziTunnel Kurulum Sihirbazı\0".encode_utf16().collect::<Vec<u16>>();

        let mut icon = LoadIconW(h_instance, 1 as *const u16);
        if icon.is_null() {
            icon = LoadIconW(0 as _, IDI_APPLICATION);
        }
        let cursor = LoadCursorW(0 as _, IDC_ARROW);

        let wnd_class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(setup_window_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: h_instance,
            hIcon: icon,
            hCursor: cursor,
            hbrBackground: 0 as _,
            lpszMenuName: std::ptr::null(),
            lpszClassName: class_name.as_ptr(),
            hIconSm: icon,
        };

        RegisterClassExW(&wnd_class);

        let mut rect = RECT {
            left: 0,
            top: 0,
            right: WIN_W as i32,
            bottom: WIN_H as i32,
        };
        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;
        AdjustWindowRectEx(&mut rect, style, 0, 0);

        let outer_w = rect.right - rect.left;
        let outer_h = rect.bottom - rect.top;

        let screen_w = GetSystemMetrics(SM_CXSCREEN);
        let screen_h = GetSystemMetrics(SM_CYSCREEN);
        let win_x = ((screen_w - outer_w) / 2).max(0);
        let win_y = ((screen_h - outer_h) / 2).max(0);

        let hwnd = CreateWindowExW(
            0,
            class_name.as_ptr(),
            window_title.as_ptr(),
            style,
            win_x,
            win_y,
            outer_w,
            outer_h,
            0 as _,
            0 as _,
            h_instance,
            std::ptr::null(),
        );

        if hwnd.is_null() {
            return;
        }

        SendMessageW(hwnd, WM_SETICON, ICON_BIG as usize, icon as isize);
        SendMessageW(hwnd, WM_SETICON, ICON_SMALL as usize, icon as isize);

        // Koyu DWM baslik cubugu
        let dark_val: i32 = 1;
        DwmSetWindowAttribute(hwnd, 20, &dark_val as *const _ as _, 4);
        let caption_color: u32 = 0x000E0C0C;
        DwmSetWindowAttribute(hwnd, 35, &caption_color as *const _ as _, 4);
        // Windows 11 DWM yuvarlak pencere kosesi (DWMWA_WINDOW_CORNER_PREFERENCE = 33, DWMWCP_ROUND = 2)
        let round_corner: i32 = 2;
        DwmSetWindowAttribute(hwnd, 33, &round_corner as *const _ as _, 4);

        let setup_state = Box::new(SetupState::new());
        let state_ptr = Box::into_raw(setup_state);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, state_ptr as isize);

        ShowWindow(hwnd, SW_SHOW);
        UpdateWindow(hwnd);

        SetWindowPos(hwnd, -1isize as HWND, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW);
        SetWindowPos(hwnd, -2isize as HWND, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW);
        SetForegroundWindow(hwnd);

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, 0 as _, 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        let _ = Box::from_raw(state_ptr);
    }

    #[cfg(not(windows))]
    {
        println!("maziTunnel Kurulum Sihirbazı yalnızca Windows platformunda çalışır.");
    }
}
