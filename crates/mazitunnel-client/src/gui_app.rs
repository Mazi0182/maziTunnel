use crate::canvas::{
    color_a, color_b, color_g, color_r, ease_out_back, ease_out_cubic, lerp_color, rgba, Canvas,
    FontWeight,
};
use crate::client_core::{ClientConfig, MaziTunnelClientCore, SavedServer, TunnelState};
use mazitunnel_core::crypto::normalize_server_key;
use mazitunnel_core::protocol::Role;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

#[cfg(windows)]
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
    Graphics::Dwm::DwmSetWindowAttribute,
    Graphics::Gdi::{
        BeginPaint, EndPaint, GetDC, ReleaseDC, SetDIBitsToDevice,
        BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HDC, PAINTSTRUCT,
    },
    System::DataExchange::{
        CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData,
    },
    System::LibraryLoader::GetModuleHandleW,
    System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE},
    UI::Input::KeyboardAndMouse::{GetAsyncKeyState, ReleaseCapture, SetCapture},
    UI::Shell::{
        Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
    },
    UI::WindowsAndMessaging::{
        AdjustWindowRectEx, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
        GetClientRect, GetCursorPos, GetForegroundWindow, GetMessageW, GetSystemMetrics,
        GetWindowLongPtrW, IsWindowVisible, KillTimer, LoadCursorW, LoadIconW, PeekMessageW,
        PostMessageW, PostQuitMessage, RegisterClassExW, SendMessageW, SetForegroundWindow,
        SetLayeredWindowAttributes, SetProcessDPIAware, SetTimer, SetWindowLongPtrW, SetWindowPos,
        ShowWindow, TranslateMessage, CS_HREDRAW, CS_VREDRAW, GWLP_USERDATA, HICON, ICON_BIG,
        ICON_SMALL, IDC_ARROW, IDI_APPLICATION, LWA_ALPHA, MSG, PM_REMOVE, SM_CXSCREEN,
        SM_CYSCREEN, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW,
        SW_HIDE, SW_RESTORE, SW_SHOW, WM_ACTIVATE, WM_CHAR, WM_CLOSE, WM_CREATE, WM_DESTROY,
        WM_ERASEBKGND, WM_KEYDOWN, WM_KILLFOCUS, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE,
        WM_PAINT, WM_QUIT, WM_RBUTTONUP, WM_SETICON, WM_TIMER, WM_USER, WNDCLASSEXW,
        WS_CAPTION, WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_MINIMIZEBOX,
        WS_OVERLAPPED, WS_POPUP, WS_SYSMENU,
    },
};

pub const WINDOW_WIDTH: u32 = 390;
pub const WINDOW_HEIGHT: u32 = 490;
pub const TRAY_POPUP_WIDTH: u32 = 256;
pub const TRAY_POPUP_HEIGHT: u32 = 260;

static APP_ICON_28: &[u8] = include_bytes!("../../../assets/app_icon_28.rgba");
#[allow(dead_code)]
static APP_ICON_48: &[u8] = include_bytes!("../../../assets/app_icon_48.rgba");

static CHANGELOG_NOTES: &[&str] = &[
    "• Bağlantı anahtarı yapıştırma ve otomatik tireleme (XXXX-XXXX-XXXX) eklendi.",
    "• CGNAT STUN periyodik yenileme ve dinamik uç nokta geçişi sağlandı.",
    "• Proaktif 120ms şifreli UDP delme ile doğrudan bağlantı kararlılığı artırıldı.",
    "• Tüm arayüz ve log bildirimlerinde Türkçe karakterler standartlaştırıldı.",
];

#[cfg(not(windows))]
#[allow(dead_code)]
const WM_USER: u32 = 0x0400;

#[cfg(not(windows))]
pub type HWND = isize;

#[allow(dead_code)]
const WM_TRAY_ICON: u32 = WM_USER + 100;
#[allow(dead_code)]
const WM_TRAY_COMMAND: u32 = WM_USER + 105;

// Sunucu Anahtari Sansurleme Fonksiyonu
// Ilk 4 karakter acik, sonrakiler sansurlu (XXXX-••••-••••)
pub fn mask_server_key(raw: &str) -> String {
    let mut count = 0;
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        if ch.is_alphanumeric() {
            count += 1;
            if count <= 4 {
                out.push(ch.to_ascii_uppercase());
            } else {
                out.push('•');
            }
        } else {
            out.push(ch);
        }
    }
    out
}

#[inline]
pub fn directional_ease_out_cubic(p: f32, moving_to_one: bool) -> f32 {
    let t = p.clamp(0.0, 1.0);
    if moving_to_one {
        1.0 - (1.0 - t).powi(3)
    } else {
        t.powi(3)
    }
}

#[derive(Clone, Copy)]
pub struct ThemePalette {
    pub is_dark: bool,
    pub window_bg: u32,
    pub card_bg: u32,
    pub card_border: u32,
    pub text_primary: u32,
    pub text_secondary: u32,
    pub text_muted: u32,
    pub dock_bg: u32,
    pub dock_border: u32,
    pub pill_bg: u32,
    pub pill_border: u32,
    pub input_bg: u32,
    pub input_border: u32,
    pub input_border_focus: u32,
    pub role_badge_bg: u32,
    pub role_badge_fg: u32,
    pub role_badge_border: u32,
    pub accent: u32,
    pub accent_text: u32,
    pub bg_hover: u32,
    pub status_online: u32,
    pub status_connecting: u32,
    pub status_error: u32,
}

impl ThemePalette {
    pub fn dark() -> Self {
        Self {
            is_dark: true,
            window_bg: rgba(12, 12, 14, 255),
            card_bg: rgba(20, 20, 23, 255),
            card_border: rgba(35, 35, 41, 255),
            text_primary: rgba(244, 244, 246, 255),
            text_secondary: rgba(142, 142, 153, 255),
            text_muted: rgba(104, 104, 115, 255),
            dock_bg: rgba(24, 24, 28, 255),
            dock_border: rgba(39, 39, 47, 255),
            pill_bg: rgba(28, 28, 34, 255),
            pill_border: rgba(46, 46, 56, 255),
            input_bg: rgba(28, 28, 34, 255),
            input_border: rgba(44, 44, 53, 255),
            input_border_focus: rgba(244, 244, 246, 255),
            role_badge_bg: rgba(35, 35, 41, 255),
            role_badge_fg: rgba(244, 244, 246, 255),
            role_badge_border: rgba(52, 52, 61, 255),
            accent: rgba(244, 244, 246, 255),
            accent_text: rgba(20, 20, 23, 255),
            bg_hover: rgba(35, 35, 41, 255),
            status_online: rgba(244, 244, 246, 255),
            status_connecting: rgba(161, 161, 170, 255),
            status_error: rgba(239, 68, 68, 255),
        }
    }

    pub fn light() -> Self {
        Self {
            is_dark: false,
            window_bg: rgba(239, 239, 242, 255),
            card_bg: rgba(255, 255, 255, 255),
            card_border: rgba(226, 226, 231, 255),
            text_primary: rgba(20, 20, 23, 255),
            text_secondary: rgba(104, 104, 115, 255),
            text_muted: rgba(130, 130, 140, 255),
            dock_bg: rgba(255, 255, 255, 255),
            dock_border: rgba(226, 226, 231, 255),
            pill_bg: rgba(244, 244, 246, 255),
            pill_border: rgba(212, 212, 216, 255),
            input_bg: rgba(244, 244, 246, 255),
            input_border: rgba(212, 212, 216, 255),
            input_border_focus: rgba(20, 20, 23, 255),
            role_badge_bg: rgba(20, 20, 23, 255),
            role_badge_fg: rgba(255, 255, 255, 255),
            role_badge_border: rgba(39, 39, 42, 255),
            accent: rgba(20, 20, 23, 255),
            accent_text: rgba(255, 255, 255, 255),
            bg_hover: rgba(228, 228, 233, 255),
            status_online: rgba(20, 20, 23, 255),
            status_connecting: rgba(113, 113, 122, 255),
            status_error: rgba(220, 38, 38, 255),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(default)]
pub struct ServerConfig {
    pub name: String,
    pub code: String,
    pub admin_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub theme: String,
    pub active_server_idx: usize,
    #[serde(default, alias = "saved_servers")]
    pub servers: Vec<ServerConfig>,
    pub last_seen_version: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            theme: "dark".to_string(),
            active_server_idx: 0,
            servers: Vec::new(),
            last_seen_version: String::new(),
        }
    }
}

pub fn get_config_path() -> PathBuf {
    let dir = mazitunnel_core::paths::get_client_data_dir();
    let _ = std::fs::create_dir_all(&dir);
    dir.join("config.json")
}

pub fn load_config() -> AppConfig {
    let client_cfg = ClientConfig::load();
    let servers: Vec<ServerConfig> = client_cfg
        .saved_servers
        .into_iter()
        .filter(|s| {
            let norm = normalize_server_key(&s.code);
            !norm.is_empty() && norm != "A1B2-C3D4-E5F6"
        })
        .map(|s| ServerConfig {
            name: s.name,
            code: normalize_server_key(&s.code),
            admin_key: s.admin_key,
        })
        .collect();

    let mut active_idx = client_cfg.active_server_idx;
    if active_idx >= servers.len() {
        active_idx = 0;
    }

    AppConfig {
        theme: if client_cfg.theme.is_empty() {
            "dark".to_string()
        } else {
            client_cfg.theme
        },
        active_server_idx: active_idx,
        servers,
        last_seen_version: client_cfg.last_seen_version,
    }
}

pub fn save_config(cfg: &AppConfig) {
    let mut client_cfg = ClientConfig::load();
    client_cfg.theme = cfg.theme.clone();
    client_cfg.last_seen_version = cfg.last_seen_version.clone();
    client_cfg.active_server_idx = cfg.active_server_idx;
    client_cfg.saved_servers = cfg
        .servers
        .iter()
        .map(|s| SavedServer {
            name: s.name.clone(),
            code: normalize_server_key(&s.code),
            admin_key: s.admin_key.clone(),
        })
        .collect();

    if let Some(active) = cfg.servers.get(cfg.active_server_idx) {
        client_cfg.server_name = active.name.clone();
        client_cfg.server_code = normalize_server_key(&active.code);
        client_cfg.admin_key = active.admin_key.clone();
    } else if let Some(first) = cfg.servers.first() {
        client_cfg.server_name = first.name.clone();
        client_cfg.server_code = normalize_server_key(&first.code);
        client_cfg.admin_key = first.admin_key.clone();
    } else {
        client_cfg.server_code.clear();
        client_cfg.admin_key.clear();
    }

    let _ = client_cfg.save();

    let old_p = get_config_path();
    if let Some(parent) = old_p.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(bytes) = serde_json::to_vec_pretty(cfg) {
        let _ = std::fs::write(&old_p, &bytes);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivePage {
    Main,
    Config,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveInput {
    None,
    ServerName,
    ServerCode,
    AdminKey,
}

pub struct GuiAppState {
    pub core: Arc<MaziTunnelClientCore>,
    pub canvas: Canvas,
    pub config: AppConfig,
    pub theme: ThemePalette,
    pub page: ActivePage,
    pub state: ConnectionState,
    pub status_subtitle: String,
    pub ping_ms: Option<u32>,
    pub assigned_address: String,
    pub is_admin: bool,
    pub copy_feedback_timer: f32,
    pub copy_anim_t: f32,

    // Guncelleme Modali Durumlari (v2.0.0)
    pub show_update_modal: bool,
    pub update_modal_closing: bool,
    pub update_modal_t: f32,
    pub update_btn_hover_t: f32,

    // Sistem Tepsisi Popup Penceresi
    #[cfg(windows)]
    pub tray_popup_hwnd: HWND,

    // 180Hz Gecis ve Efekt Durumlari
    pub theme_morph_t: f32,
    pub theme_crossfade_alpha: f32,
    pub theme_crossfade_buf: Vec<u32>,

    pub page_transition_t: f32,
    pub page_slide_dir: f32,
    pub page_from_buf: Vec<u32>,
    pub page_to_buf: Vec<u32>,

    // TunnelSwitch ve Diger Arayuz Animasyonlari
    pub switch_pos: f32,
    pub switch_anim_from: f32,
    pub switch_anim_to: f32,
    pub switch_anim_progress: f32,
    pub connecting_started_at: Option<Instant>,
    pub switch_hover: f32,
    pub switch_press: f32,
    pub spinner_angle: f32,
    pub pulse_t: f32,
    pub anim_time: f32,
    pub last_frame: Instant,

    // Adres Hapi
    pub pill_anim_t: f32,
    pub copy_btn_hover_t: f32,

    // Alt Kart Butonu
    pub dock_btn_hover_t: f32,
    pub theme_btn_hover_t: f32,

    // Ayarlar Sayfasi Durumu
    pub dropdown_open: bool,
    pub dropdown_anim_t: f32,
    pub dropdown_selected_idx: usize, // 0 = + Yeni Sunucu Ekle, 1..=servers.len()
    pub dropdown_hover_idx: Option<usize>,

    pub admin_key_expanded: bool,
    pub admin_expand_t: f32,

    pub active_input: ActiveInput,
    pub cursor_blink: f32,
    pub edit_name: String,
    pub edit_code: String,
    pub edit_admin_key: String,
    pub show_server_code: bool,
    pub show_admin_key: bool,
    pub input_select_all: bool,
    pub input_cursor: usize,
    pub input_sel_anchor: usize,
    pub input_is_selecting: bool,
    pub code_eye_t: f32,
    pub admin_eye_t: f32,
    pub code_eye_hover_t: f32,
    pub admin_eye_hover_t: f32,

    pub config_back_hover_t: f32,
    pub config_delete_hover_t: f32,
    pub config_cancel_hover_t: f32,
    pub config_save_hover_t: f32,

    // Mouse Durumu
    pub mouse_x: f32,
    pub mouse_y: f32,
    pub mouse_pressed: bool,
    pub pressed_control: Option<String>,

    #[cfg(windows)]
    pub main_hwnd: HWND,
    #[cfg(windows)]
    pub tray_subpopup_hwnd: HWND,
    pub notified_connected: bool,
    pub was_connected_for_notify: bool,
    pub last_notify_time: Option<Instant>,

    pub switch_drag_start_x: f32,
    pub switch_drag_start_pos: f32,
    pub switch_is_dragging: bool,

    // Akici Metin Gecisi (SmoothLabel Paritesi)
    pub display_title: String,
    pub display_subtitle: String,
    pub title_fade_alpha: f32,
    pub title_fading_out: bool,
    pub sub_fade_alpha: f32,
    pub sub_fading_out: bool,
    pub dirty: bool,
    pub last_redraw: Instant,
}

#[inline]
pub fn scale_alpha(col: u32, factor: f32) -> u32 {
    let a = (color_a(col) as f32 * factor.clamp(0.0, 1.0)).round() as u8;
    rgba(color_r(col), color_g(col), color_b(col), a)
}

fn render_secret_field(
    canvas: &mut Canvas,
    text: &str,
    is_code: bool,
    eye_t: f32,
    base_x: f32,
    base_y: f32,
    font_size: f32,
    color: u32,
) {
    let t = eye_t.clamp(0.0, 1.0);
    if text.is_empty() {
        return;
    }
    if t <= 0.01 {
        let display = if is_code {
            mask_server_key(text)
        } else {
            text.chars().map(|_| '•').collect::<String>()
        };
        canvas.draw_text(is_code, font_size, base_x, base_y, &display, color);
        return;
    }
    if t >= 0.99 {
        canvas.draw_text(is_code, font_size, base_x, base_y, text, color);
        return;
    }

    let chars: Vec<char> = text.chars().collect();
    let total_len = chars.len().max(1);
    let mut cur_x = base_x;

    for (i, &ch) in chars.iter().enumerate() {
        let char_prog = i as f32 / total_len as f32;
        let is_revealed = char_prog < t;
        let dist = (t - char_prog).abs();
        let wave = (1.0 - (dist * 4.0).min(1.0)).max(0.0);
        let y_offset = -1.6 * wave;
        let alpha_factor = (0.75 + 0.25 * wave).clamp(0.0, 1.0);

        let ch_to_draw = if is_code && ch == '-' {
            '-'
        } else if is_revealed {
            ch
        } else {
            '•'
        };

        let ch_str = ch_to_draw.to_string();
        let (cw, _) = canvas.measure_text(is_code, font_size, &ch_str);
        canvas.draw_text(
            is_code,
            font_size,
            cur_x,
            base_y + y_offset,
            &ch_str,
            scale_alpha(color, alpha_factor),
        );
        cur_x += cw;
    }
}

impl GuiAppState {
    pub fn new(core: Arc<MaziTunnelClientCore>, is_updated: bool) -> Self {
        let mut config = load_config();
        let mut core_cfg = core.config.lock().unwrap();

        // Cekirdek ve app yapilandirmasini kayipsiz esitle
        if config.servers.is_empty() && !core_cfg.saved_servers.is_empty() {
            config.servers = core_cfg
                .saved_servers
                .iter()
                .filter(|s| {
                    let norm = normalize_server_key(&s.code);
                    !norm.is_empty() && norm != "A1B2-C3D4-E5F6"
                })
                .map(|s| ServerConfig {
                    name: s.name.clone(),
                    code: normalize_server_key(&s.code),
                    admin_key: s.admin_key.clone(),
                })
                .collect();
            if !core_cfg.server_code.is_empty() {
                let norm = normalize_server_key(&core_cfg.server_code);
                if let Some(pos) = config.servers.iter().position(|s| s.code.eq_ignore_ascii_case(&norm)) {
                    config.active_server_idx = pos;
                }
            }
        } else if !config.servers.is_empty() && core_cfg.saved_servers.is_empty() {
            core_cfg.saved_servers = config
                .servers
                .iter()
                .map(|s| SavedServer {
                    name: s.name.clone(),
                    code: normalize_server_key(&s.code),
                    admin_key: s.admin_key.clone(),
                })
                .collect();
            if let Some(srv) = config.servers.get(config.active_server_idx) {
                core_cfg.server_name = srv.name.clone();
                core_cfg.server_code = normalize_server_key(&srv.code);
                core_cfg.admin_key = srv.admin_key.clone();
            }
            let _ = core_cfg.save();
        }

        for s in &mut config.servers {
            s.code = normalize_server_key(&s.code);
        }
        config.servers.retain(|s| !s.code.is_empty() && s.code != "A1B2-C3D4-E5F6");
        if config.active_server_idx >= config.servers.len() {
            config.active_server_idx = 0;
        }

        if !core_cfg.theme.is_empty() {
            config.theme = core_cfg.theme.clone();
        }
        if !core_cfg.last_seen_version.is_empty() {
            config.last_seen_version = core_cfg.last_seen_version.clone();
        }
        drop(core_cfg);

        let is_dark = config.theme != "light";
        let theme = if is_dark {
            ThemePalette::dark()
        } else {
            ThemePalette::light()
        };

        let mut edit_name = String::new();
        let mut edit_code = String::new();
        let mut edit_admin_key = String::new();

        if let Some(srv) = config.servers.get(config.active_server_idx) {
            edit_name = srv.name.clone();
            edit_code = srv.code.clone();
            edit_admin_key = srv.admin_key.clone();
        }

        let show_modal = is_updated;
        if config.last_seen_version != "2.0.1" {
            config.last_seen_version = "2.0.1".to_string();
            let mut c = core.config.lock().unwrap();
            c.last_seen_version = "2.0.1".to_string();
            let _ = c.save();
            drop(c);
            save_config(&config);
        }
        let initial_title = "Bağlı Değil".to_string();
        let initial_subtitle = "Bağlanmak için anahtara dokunun.".to_string();

        Self {
            core,
            canvas: Canvas::new(WINDOW_WIDTH, WINDOW_HEIGHT),
            config,
            theme,
            page: ActivePage::Main,
            state: ConnectionState::Disconnected,
            status_subtitle: initial_subtitle.clone(),
            ping_ms: None,
            assigned_address: "baglanti.local".to_string(),
            is_admin: false,
            copy_feedback_timer: 0.0,
            copy_anim_t: 0.0,

            show_update_modal: show_modal,
            update_modal_closing: false,
            update_modal_t: if show_modal { 1.0 } else { 0.0 },
            update_btn_hover_t: 0.0,

            #[cfg(windows)]
            tray_popup_hwnd: 0 as HWND,
            #[cfg(windows)]
            main_hwnd: 0 as HWND,
            #[cfg(windows)]
            tray_subpopup_hwnd: 0 as HWND,
            notified_connected: false,
            was_connected_for_notify: false,
            last_notify_time: None,

            switch_drag_start_x: 0.0,
            switch_drag_start_pos: 0.0,
            switch_is_dragging: false,

            theme_morph_t: if is_dark { 0.0 } else { 1.0 },
            theme_crossfade_alpha: 0.0,
            theme_crossfade_buf: Vec::new(),

            page_transition_t: 1.0,
            page_slide_dir: 1.0,
            page_from_buf: Vec::new(),
            page_to_buf: Vec::new(),

            switch_pos: 0.0,
            switch_anim_from: 0.0,
            switch_anim_to: 0.0,
            switch_anim_progress: 1.0,
            connecting_started_at: None,
            switch_hover: 0.0,
            switch_press: 0.0,
            spinner_angle: 0.0,
            pulse_t: 0.0,
            anim_time: 0.0,
            last_frame: Instant::now(),

            pill_anim_t: 0.0,
            copy_btn_hover_t: 0.0,

            dock_btn_hover_t: 0.0,
            theme_btn_hover_t: 0.0,

            dropdown_open: false,
            dropdown_anim_t: 0.0,
            dropdown_selected_idx: 0,
            dropdown_hover_idx: None,

            admin_key_expanded: false,
            admin_expand_t: 0.0,

            active_input: ActiveInput::None,
            cursor_blink: 0.0,
            edit_name,
            edit_code,
            edit_admin_key,
            show_server_code: false,
            show_admin_key: false,
            input_select_all: false,
            input_cursor: 0,
            input_sel_anchor: 0,
            input_is_selecting: false,
            code_eye_t: 0.0,
            admin_eye_t: 0.0,
            code_eye_hover_t: 0.0,
            admin_eye_hover_t: 0.0,

            config_back_hover_t: 0.0,
            config_delete_hover_t: 0.0,
            config_cancel_hover_t: 0.0,
            config_save_hover_t: 0.0,

            mouse_x: 0.0,
            mouse_y: 0.0,
            mouse_pressed: false,
            pressed_control: None,

            display_title: initial_title,
            display_subtitle: initial_subtitle,
            title_fade_alpha: 1.0,
            title_fading_out: false,
            sub_fade_alpha: 1.0,
            sub_fading_out: false,
            dirty: true,
            last_redraw: Instant::now(),
        }
    }

    #[inline]
    pub fn current_switch_y(&self) -> f32 {
        let pill_ease = directional_ease_out_cubic(self.pill_anim_t, self.state == ConnectionState::Connected);
        110.0 - (pill_ease * 26.0)
    }

    #[inline]
    pub fn config_form_y(&self) -> (f32, f32, f32) {
        let admin_ease = directional_ease_out_cubic(self.admin_expand_t, self.admin_key_expanded);
        let extra_h = 52.0 * admin_ease;
        let form_h = 282.0 + extra_h;
        let form_y = ((WINDOW_HEIGHT as f32 - form_h) * 0.5 + 14.0).round();
        (form_y, form_h, extra_h)
    }

    pub fn get_displayed_input_string(&self, input: ActiveInput) -> String {
        match input {
            ActiveInput::ServerCode => {
                if self.show_server_code {
                    self.edit_code.clone()
                } else {
                    mask_server_key(&self.edit_code)
                }
            }
            ActiveInput::AdminKey => {
                if self.show_admin_key {
                    self.edit_admin_key.clone()
                } else {
                    self.edit_admin_key.chars().map(|_| '•').collect()
                }
            }
            ActiveInput::ServerName => self.edit_name.clone(),
            ActiveInput::None => String::new(),
        }
    }

    pub fn char_index_from_rel_x(&self, is_code: bool, font_size: f32, display_str: &str, rel_x: f32) -> usize {
        let rel_x = rel_x.max(0.0);
        let chars: Vec<char> = display_str.chars().collect();
        let total = chars.len();
        if total == 0 {
            return 0;
        }

        let mut best_idx = 0;
        let mut best_diff = rel_x;

        let mut prefix = String::with_capacity(display_str.len());
        for (i, ch) in chars.iter().enumerate() {
            prefix.push(*ch);
            let (w, _) = self.canvas.measure_text(is_code, font_size, &prefix);
            let diff = (w - rel_x).abs();
            if diff < best_diff {
                best_diff = diff;
                best_idx = i + 1;
            }
        }

        best_idx
    }

    fn get_active_char_count(&self) -> usize {
        match self.active_input {
            ActiveInput::ServerName => self.edit_name.chars().count(),
            ActiveInput::ServerCode => self.edit_code.chars().count(),
            ActiveInput::AdminKey => self.edit_admin_key.chars().count(),
            ActiveInput::None => 0,
        }
    }

    #[allow(dead_code)]
    fn get_active_input_slice(&self, start: usize, end: usize) -> Option<String> {
        if start >= end {
            return None;
        }
        match self.active_input {
            ActiveInput::ServerName => {
                let chars: Vec<char> = self.edit_name.chars().collect();
                if start < chars.len() {
                    let end = end.min(chars.len());
                    Some(chars[start..end].iter().collect())
                } else {
                    None
                }
            }
            ActiveInput::ServerCode => {
                let chars: Vec<char> = self.edit_code.chars().collect();
                if start < chars.len() {
                    let end = end.min(chars.len());
                    Some(chars[start..end].iter().collect())
                } else {
                    None
                }
            }
            ActiveInput::AdminKey => {
                let chars: Vec<char> = self.edit_admin_key.chars().collect();
                if start < chars.len() {
                    let end = end.min(chars.len());
                    Some(chars[start..end].iter().collect())
                } else {
                    None
                }
            }
            ActiveInput::None => None,
        }
    }

    fn delete_active_char_range(&mut self, start: usize, end: usize) {
        if start >= end {
            return;
        }
        match self.active_input {
            ActiveInput::ServerName => {
                let chars: Vec<char> = self.edit_name.chars().collect();
                if start < chars.len() {
                    let end = end.min(chars.len());
                    self.edit_name = chars[..start].iter().chain(&chars[end..]).collect();
                }
            }
            ActiveInput::ServerCode => {
                let chars: Vec<char> = self.edit_code.chars().collect();
                if start < chars.len() {
                    let end = end.min(chars.len());
                    self.edit_code = chars[..start].iter().chain(&chars[end..]).collect();
                }
            }
            ActiveInput::AdminKey => {
                let chars: Vec<char> = self.edit_admin_key.chars().collect();
                if start < chars.len() {
                    let end = end.min(chars.len());
                    self.edit_admin_key = chars[..start].iter().chain(&chars[end..]).collect();
                }
            }
            ActiveInput::None => {}
        }
    }

    fn insert_active_char(&mut self, at: usize, ch: char) {
        match self.active_input {
            ActiveInput::ServerName => {
                if self.edit_name.len() < 32 {
                    let mut chars: Vec<char> = self.edit_name.chars().collect();
                    let at = at.min(chars.len());
                    chars.insert(at, ch);
                    self.edit_name = chars.into_iter().collect();
                }
            }
            ActiveInput::ServerCode => {
                if self.edit_code.len() < 40 {
                    let mut chars: Vec<char> = self.edit_code.chars().collect();
                    let at = at.min(chars.len());
                    chars.insert(at, ch.to_ascii_uppercase());
                    self.edit_code = chars.into_iter().collect();
                }
            }
            ActiveInput::AdminKey => {
                if self.edit_admin_key.len() < 64 {
                    let mut chars: Vec<char> = self.edit_admin_key.chars().collect();
                    let at = at.min(chars.len());
                    chars.insert(at, ch);
                    self.edit_admin_key = chars.into_iter().collect();
                }
            }
            ActiveInput::None => {}
        }
    }

    fn reformat_server_code(&mut self) {
        if self.active_input != ActiveInput::ServerCode {
            return;
        }
        let chars: Vec<char> = self.edit_code.chars().collect();
        let cur = self.input_cursor.min(chars.len());
        let anc = self.input_sel_anchor.min(chars.len());

        let alnum_before_cur = chars[..cur].iter().filter(|c| c.is_ascii_alphanumeric()).count();
        let alnum_before_anc = chars[..anc].iter().filter(|c| c.is_ascii_alphanumeric()).count();

        let clean: Vec<char> = chars
            .into_iter()
            .filter(|c| c.is_ascii_alphanumeric())
            .map(|c| c.to_ascii_uppercase())
            .take(12)
            .collect();

        let mut formatted = String::with_capacity(14);
        let mut new_cur = 0usize;
        let mut new_anc = 0usize;
        let mut seen_alnum = 0usize;

        for (i, ch) in clean.iter().enumerate() {
            if i > 0 && i % 4 == 0 {
                formatted.push('-');
            }
            if seen_alnum == alnum_before_cur {
                new_cur = formatted.chars().count();
            }
            if seen_alnum == alnum_before_anc {
                new_anc = formatted.chars().count();
            }
            formatted.push(*ch);
            seen_alnum += 1;
        }
        if alnum_before_cur >= seen_alnum {
            new_cur = formatted.chars().count();
        }
        if alnum_before_anc >= seen_alnum {
            new_anc = formatted.chars().count();
        }

        self.edit_code = formatted;
        self.input_cursor = new_cur;
        self.input_sel_anchor = new_anc;
    }

    pub fn is_animating(&self) -> bool {
        self.state == ConnectionState::Connecting
            || self.page_transition_t < 1.0
            || (self.show_update_modal && (self.update_modal_closing || self.update_modal_t < 1.0))
            || self.theme_crossfade_alpha > 0.001
            || self.switch_anim_progress < 1.0
            || self.switch_is_dragging
            || (self.pill_anim_t > 0.001 && self.pill_anim_t < 0.999)
            || (self.dropdown_anim_t > 0.001 && self.dropdown_anim_t < 0.999)
            || (self.admin_expand_t > 0.001 && self.admin_expand_t < 0.999)
            || (self.code_eye_t > 0.001 && self.code_eye_t < 0.999)
            || (self.admin_eye_t > 0.001 && self.admin_eye_t < 0.999)
            || self.code_eye_hover_t > 0.005
            || self.admin_eye_hover_t > 0.005
            || self.title_fade_alpha < 0.999
            || self.sub_fade_alpha < 0.999
            || self.copy_feedback_timer > 0.0
            || (self.copy_anim_t > 0.001 && self.copy_anim_t < 0.999)
            || self.input_is_selecting
            || self.active_input != ActiveInput::None
            || self.mouse_pressed
            || self.switch_hover > 0.005
            || self.switch_press > 0.005
            || self.theme_btn_hover_t > 0.005
            || self.dock_btn_hover_t > 0.005
            || self.copy_btn_hover_t > 0.005
            || self.config_back_hover_t > 0.005
            || self.config_delete_hover_t > 0.005
            || self.config_cancel_hover_t > 0.005
            || self.config_save_hover_t > 0.005
            || self.update_btn_hover_t > 0.005
    }

    pub fn toggle_theme(&mut self) {
        // Eski kareyi crossfade icin kaydet
        self.theme_crossfade_buf = self.canvas.pixels.clone();
        self.theme_crossfade_alpha = 1.0;

        if self.theme.is_dark {
            self.theme = ThemePalette::light();
            self.config.theme = "light".to_string();
        } else {
            self.theme = ThemePalette::dark();
            self.config.theme = "dark".to_string();
        }

        self.core.config.lock().unwrap().theme = self.config.theme.clone();
        let _ = self.core.config.lock().unwrap().save();
        save_config(&self.config);
    }

    pub fn switch_to_page(&mut self, target_page: ActivePage) {
        if self.page == target_page && self.page_transition_t >= 1.0 {
            return;
        }

        self.dock_btn_hover_t = 0.0;
        self.config_back_hover_t = 0.0;
        self.config_cancel_hover_t = 0.0;
        self.config_save_hover_t = 0.0;

        // 1. Cikis yapan sayfayi isle
        self.canvas.clear(self.theme.window_bg);
        match self.page {
            ActivePage::Main => self.render_page_main(),
            ActivePage::Config => self.render_page_config(),
        }
        self.page_from_buf = self.canvas.pixels.clone();

        // 2. Hedef sayfayi isle
        self.page = target_page;
        self.canvas.clear(self.theme.window_bg);
        match self.page {
            ActivePage::Main => self.render_page_main(),
            ActivePage::Config => self.render_page_config(),
        }
        self.page_to_buf = self.canvas.pixels.clone();

        // 3. Gecisi baslat
        self.page_slide_dir = if target_page == ActivePage::Config { 1.0 } else { -1.0 };
        self.page_transition_t = 0.0;
    }

    pub fn open_config_page(&mut self, select_idx: usize) {
        self.dropdown_open = false;
        self.dropdown_anim_t = 0.0;
        self.dropdown_selected_idx = select_idx;
        self.show_server_code = false;
        self.show_admin_key = false;
        self.input_select_all = false;
        self.input_cursor = self.edit_code.chars().count();
        self.input_sel_anchor = self.input_cursor;
        self.input_is_selecting = false;
        self.code_eye_t = 0.0;
        self.admin_eye_t = 0.0;
        self.code_eye_hover_t = 0.0;
        self.admin_eye_hover_t = 0.0;

        if select_idx == 0 || self.config.servers.is_empty() {
            self.dropdown_selected_idx = 0;
            self.edit_name.clear();
            self.edit_code.clear();
            self.edit_admin_key.clear();
            self.admin_key_expanded = false;
            self.admin_expand_t = 0.0;
            self.active_input = ActiveInput::ServerCode;
        } else {
            let srv_idx = select_idx - 1;
            if let Some(srv) = self.config.servers.get(srv_idx) {
                self.edit_name = srv.name.clone();
                self.edit_code = srv.code.clone();
                self.edit_admin_key = srv.admin_key.clone();
                self.admin_key_expanded = false;
                self.admin_expand_t = 0.0;
            }
            self.active_input = ActiveInput::None;
        }

        self.switch_to_page(ActivePage::Config);
    }

    pub fn toggle_connection(&mut self) {
        match self.state {
            ConnectionState::Disconnected => {
                if self.config.servers.is_empty() {
                    self.open_config_page(0);
                    return;
                }

                if let Some(srv) = self.config.servers.get(self.config.active_server_idx) {
                    if srv.code.trim().is_empty() {
                        self.open_config_page(self.config.active_server_idx + 1);
                        self.active_input = ActiveInput::ServerCode;
                        return;
                    }
                    self.core.local_resolver.set_server_name(&srv.name);
                    if let Err(e) = self.core.connect(&srv.code, &srv.admin_key) {
                        self.status_subtitle = format!("Hata: {}", e);
                    } else {
                        self.connecting_started_at = Some(Instant::now());
                        self.state = ConnectionState::Connecting;
                        self.status_subtitle = "Ağ hazırlanıyor, eşleşme isteği gönderiliyor...".to_string();
                        self.switch_anim_from = self.switch_pos;
                        self.switch_anim_to = 1.0;
                        self.switch_anim_progress = 0.0;
                    }
                } else {
                    self.open_config_page(0);
                }
            }
            ConnectionState::Connecting | ConnectionState::Connected => {
                self.core.disconnect();
                self.connecting_started_at = None;
                self.state = ConnectionState::Disconnected;
                self.ping_ms = None;
                self.handle_disconnection_notify();
                self.status_subtitle = "Bağlantı kesildi.".to_string();
                self.switch_anim_from = self.switch_pos;
                self.switch_anim_to = 0.0;
                self.switch_anim_progress = 0.0;
            }
        }
    }

    fn handle_disconnection_notify(&mut self) {
        if self.was_connected_for_notify {
            self.was_connected_for_notify = false;
            #[cfg(windows)]
            let in_background = unsafe {
                IsWindowVisible(self.main_hwnd) == 0 || GetForegroundWindow() != self.main_hwnd
            };
            #[cfg(not(windows))]
            let in_background = false;

            let srv_label = if let Some(s) = self.config.servers.get(self.config.active_server_idx) {
                s.name.clone()
            } else {
                "maziTunnel".to_string()
            };
            let _ = &srv_label;

            if in_background && self.last_notify_time.map_or(true, |t| t.elapsed().as_secs() >= 5) {
                self.last_notify_time = Some(Instant::now());
                #[cfg(windows)]
                unsafe {
                    send_tray_notification(
                        self.main_hwnd,
                        "maziTunnel — Bağlantı Kesildi",
                        &format!("{} sunucusu ile bağlantı sona erdi.", srv_label),
                    );
                }
            }
        }
    }

    pub fn update_animations(&mut self) {
        let now = Instant::now();
        let dt = (now - self.last_frame).as_secs_f32().min(0.05);
        self.last_frame = now;

        self.anim_time += dt;
        self.pulse_t = (self.anim_time * 2.5).sin() * 0.5 + 0.5;
        self.cursor_blink += dt * 2.5;
        self.spinner_angle = (self.spinner_angle + dt * 6.5) % (std::f32::consts::PI * 2.0);

        if self.copy_feedback_timer > 0.0 {
            self.copy_feedback_timer = (self.copy_feedback_timer - dt).max(0.0);
        }

        let target_copy_anim = if self.copy_feedback_timer > 0.0 { 1.0 } else { 0.0 };
        let copy_step = dt / 0.18;
        if target_copy_anim > self.copy_anim_t {
            self.copy_anim_t = (self.copy_anim_t + copy_step).min(1.0);
        } else if target_copy_anim < self.copy_anim_t {
            self.copy_anim_t = (self.copy_anim_t - copy_step).max(0.0);
        }

        // Tema crossfade ve morf gecisi (320ms)
        if self.theme_crossfade_alpha > 0.001 {
            self.theme_crossfade_alpha = (self.theme_crossfade_alpha - dt / 0.22).max(0.0);
        }
        let target_morph = if self.theme.is_dark { 0.0 } else { 1.0 };
        let morph_step = dt / 0.32;
        if target_morph > self.theme_morph_t {
            self.theme_morph_t = (self.theme_morph_t + morph_step).min(1.0);
        } else if target_morph < self.theme_morph_t {
            self.theme_morph_t = (self.theme_morph_t - morph_step).max(0.0);
        }

        // Sayfa kaydirma gecis animasyonu (220ms)
        if self.page_transition_t < 1.0 {
            self.page_transition_t = (self.page_transition_t + dt / 0.22).min(1.0);
        }

        // Goz animasyonlari (160ms)
        let target_code_eye = if self.show_server_code { 1.0 } else { 0.0 };
        let eye_toggle_step = dt / 0.16;
        if target_code_eye > self.code_eye_t {
            self.code_eye_t = (self.code_eye_t + eye_toggle_step).min(1.0);
        } else if target_code_eye < self.code_eye_t {
            self.code_eye_t = (self.code_eye_t - eye_toggle_step).max(0.0);
        }

        let target_admin_eye = if self.show_admin_key { 1.0 } else { 0.0 };
        if target_admin_eye > self.admin_eye_t {
            self.admin_eye_t = (self.admin_eye_t + eye_toggle_step).min(1.0);
        } else if target_admin_eye < self.admin_eye_t {
            self.admin_eye_t = (self.admin_eye_t - eye_toggle_step).max(0.0);
        }

        // Guncelleme modali animasyonu (260ms)
        if self.show_update_modal {
            if self.update_modal_closing {
                self.update_modal_t = (self.update_modal_t - dt / 0.26).max(0.0);
                if self.update_modal_t <= 0.001 {
                    self.show_update_modal = false;
                    self.update_modal_closing = false;
                }
            } else if self.update_modal_t < 1.0 {
                self.update_modal_t = (self.update_modal_t + dt / 0.26).min(1.0);
            }
        }

        // Canli cekirdek durumu senkronizasyonu
        match self.core.shared_state.get_state() {
            TunnelState::Disconnected => {
                self.connecting_started_at = None;
                self.state = ConnectionState::Disconnected;
                self.ping_ms = None;
                self.handle_disconnection_notify();
                if let Some(err) = self.core.shared_state.get_last_error() {
                    self.status_subtitle = err;
                } else {
                    self.status_subtitle = "Bağlanmak için anahtara dokunun.".to_string();
                }
            }
            TunnelState::Signaling => {
                self.state = ConnectionState::Connecting;
                let is_early = self.connecting_started_at.map_or(false, |s| s.elapsed().as_millis() < 180);
                if is_early {
                    self.status_subtitle = "Ağ hazırlanıyor, eşleşme isteği gönderiliyor...".to_string();
                } else {
                    self.status_subtitle = "Eşleşme isteği gönderildi, sunucudan yanıt bekleniyor...".to_string();
                }
            }
            TunnelState::Punching(n) => {
                self.state = ConnectionState::Connecting;
                self.status_subtitle = format!("Eşleşme sağlandı, doğrudan P2P bağlantı kuruluyor (#{})...", n);
            }
            TunnelState::Authenticating => {
                self.state = ConnectionState::Connecting;
                self.status_subtitle = "Sunucu ile güvenli tünel ve kimlik doğrulanıyor...".to_string();
            }
            TunnelState::Reconnecting(n) => {
                self.state = ConnectionState::Connecting;
                self.status_subtitle = format!("Yeniden bağlanılıyor ({}/5)...", n);
            }
            TunnelState::Connected {
                role,
                rtt_ms,
                address,
                server_name,
            } => {
                let elapsed_ms = self.connecting_started_at.map_or(9999, |s| s.elapsed().as_millis());
                if elapsed_ms < 850 {
                    self.state = ConnectionState::Connecting;
                    if elapsed_ms > 350 {
                        self.status_subtitle = "Kimlik doğrulandı, güvenli tünel hazırlanıyor...".to_string();
                    }
                } else {
                    self.connecting_started_at = None;
                    self.state = ConnectionState::Connected;
                    self.is_admin = role == Role::Admin;
                    self.assigned_address = address;
                    let live_ping = self.core.shared_state.ping_ms();
                    let ms = if live_ping > 0.0 {
                        live_ping.round() as u32
                    } else {
                        rtt_ms.round() as u32
                    };
                    self.ping_ms = Some(ms.max(1));
                    if self.is_admin {
                        self.status_subtitle = "Yönetici bağlantısı aktif. Adresi kopyalayıp bağlanabilirsiniz.".to_string();
                    } else {
                        self.status_subtitle = "Bağlantı aktif. Adresi kopyalayıp oyuna bağlanabilirsiniz.".to_string();
                    }

                    if !server_name.trim().is_empty() {
                        if let Some(srv) = self.config.servers.get_mut(self.config.active_server_idx) {
                            if srv.name != server_name {
                                srv.name = server_name.clone();
                                self.sync_and_save_servers();
                            }
                        }
                    }

                    #[cfg(windows)]
                    let in_background = unsafe {
                        IsWindowVisible(self.main_hwnd) == 0 || GetForegroundWindow() != self.main_hwnd
                    };
                    #[cfg(not(windows))]
                    let in_background = false;

                    if !self.was_connected_for_notify {
                        self.was_connected_for_notify = true;
                        let srv_label = if !server_name.trim().is_empty() {
                            server_name.clone()
                        } else if let Some(s) = self.config.servers.get(self.config.active_server_idx) {
                            s.name.clone()
                        } else {
                            "maziTunnel".to_string()
                        };
                        let _ = &srv_label;
                        if in_background && self.last_notify_time.map_or(true, |t| t.elapsed().as_secs() >= 8) {
                            self.last_notify_time = Some(Instant::now());
                            #[cfg(windows)]
                            unsafe {
                                send_tray_notification(
                                    self.main_hwnd,
                                    "maziTunnel — Bağlantı Aktif",
                                    &format!("{} sunucusuna bağlandınız.\nAdres: {}", srv_label, self.assigned_address),
                                );
                            }
                        }
                    }
                }
            }
            TunnelState::Error(err) => {
                self.connecting_started_at = None;
                self.state = ConnectionState::Disconnected;
                self.ping_ms = None;
                self.handle_disconnection_notify();
                self.status_subtitle = err;
            }
        }

        // Akici baslik/aciklama gecisi (SmoothLabel Paritesi)
        let target_title = match self.state {
            ConnectionState::Connected => "Bağlı",
            ConnectionState::Connecting => "Bağlanıyor...",
            ConnectionState::Disconnected => {
                if self.status_subtitle.starts_with("Hata:") {
                    "Bağlantı Hatası"
                } else {
                    "Bağlı Değil"
                }
            }
        };

        if self.display_title != target_title {
            self.display_title = target_title.to_string();
            self.title_fade_alpha = 0.35;
        }
        if self.title_fade_alpha < 1.0 {
            self.title_fade_alpha = (self.title_fade_alpha + dt / 0.12).min(1.0);
        }

        if self.display_subtitle != self.status_subtitle {
            self.display_subtitle = self.status_subtitle.clone();
            self.sub_fade_alpha = 0.35;
        }
        if self.sub_fade_alpha < 1.0 {
            self.sub_fade_alpha = (self.sub_fade_alpha + dt / 0.12).min(1.0);
        }

        // TunnelSwitch pozisyon animasyonu (380ms OutBack acilma, OutCubic kapanma)
        if !self.switch_is_dragging {
            let desired_pos = match self.state {
                ConnectionState::Connected | ConnectionState::Connecting => 1.0f32,
                ConnectionState::Disconnected => 0.0f32,
            };
            if (desired_pos - self.switch_anim_to).abs() > 0.001 {
                self.switch_anim_from = self.switch_pos;
                self.switch_anim_to = desired_pos;
                self.switch_anim_progress = 0.0;
            }
            if self.switch_anim_progress < 1.0 {
                let duration = 0.38;
                self.switch_anim_progress = (self.switch_anim_progress + dt / duration).min(1.0);
                let prog = self.switch_anim_progress;
                if self.switch_anim_to > self.switch_anim_from {
                    let curve = ease_out_back(prog, 0.22);
                    self.switch_pos = self.switch_anim_from + (self.switch_anim_to - self.switch_anim_from) * curve;
                } else {
                    let curve = 1.0 - (1.0 - prog).powi(3);
                    self.switch_pos = self.switch_anim_from + (self.switch_anim_to - self.switch_anim_from) * curve;
                }
            }
        }

        // TunnelSwitch hover ve press animasyonlari (ustel yumusatma)
        let sw_y = self.current_switch_y();
        let switch_hover_target = if self.page == ActivePage::Main
            && !self.show_update_modal
            && self.mouse_x >= 125.0
            && self.mouse_x <= 265.0
            && self.mouse_y >= sw_y
            && self.mouse_y <= (sw_y + 68.0)
        {
            1.0f32
        } else {
            0.0f32
        };
        let decay_hover = 1.0 - (-dt * 14.0).exp();
        self.switch_hover += (switch_hover_target - self.switch_hover) * decay_hover;

        let switch_press_target = if self.mouse_pressed && self.pressed_control.as_deref() == Some("switch") {
            1.0f32
        } else {
            0.0f32
        };
        let decay_press = 1.0 - (-dt * 18.0).exp();
        self.switch_press += (switch_press_target - self.switch_press) * decay_press;

        // Adres hapi gosterim animasyonu (180ms yonlu gecis)
        let target_pill = if self.state == ConnectionState::Connected { 1.0f32 } else { 0.0f32 };
        let pill_step = dt / 0.18;
        if target_pill > self.pill_anim_t {
            self.pill_anim_t = (self.pill_anim_t + pill_step).min(1.0);
        } else if target_pill < self.pill_anim_t {
            self.pill_anim_t = (self.pill_anim_t - pill_step).max(0.0);
        }

        // Dropdown yukseklik animasyonu (240ms)
        let target_drop = if self.dropdown_open { 1.0f32 } else { 0.0f32 };
        let drop_step = dt / 0.24;
        if target_drop > self.dropdown_anim_t {
            self.dropdown_anim_t = (self.dropdown_anim_t + drop_step).min(1.0);
        } else if target_drop < self.dropdown_anim_t {
            self.dropdown_anim_t = (self.dropdown_anim_t - drop_step).max(0.0);
        }

        // Yonetici anahtari acilma animasyonu (250ms)
        let target_adm = if self.admin_key_expanded { 1.0f32 } else { 0.0f32 };
        let adm_step = dt / 0.25;
        if target_adm > self.admin_expand_t {
            self.admin_expand_t = (self.admin_expand_t + adm_step).min(1.0);
        } else if target_adm < self.admin_expand_t {
            self.admin_expand_t = (self.admin_expand_t - adm_step).max(0.0);
        }

        // Buton hover interpolasyonlari
        self.interpolate_hover_states(dt);
    }

    fn interpolate_hover_states(&mut self, dt: f32) {
        let mx = self.mouse_x;
        let my = self.mouse_y;
        let speed = (dt * 16.0).min(1.0);

        let in_modal = self.show_update_modal && self.update_modal_t > 0.01;

        let target_update_btn = if in_modal && mx >= 40.0 && mx <= 350.0 && my >= 372.0 && my <= 414.0 { 1.0 } else { 0.0 };
        self.update_btn_hover_t += (target_update_btn - self.update_btn_hover_t) * speed;

        if in_modal {
            self.theme_btn_hover_t += (0.0 - self.theme_btn_hover_t) * speed;
            self.dock_btn_hover_t += (0.0 - self.dock_btn_hover_t) * speed;
            self.copy_btn_hover_t += (0.0 - self.copy_btn_hover_t) * speed;
            self.code_eye_hover_t = (self.code_eye_hover_t - dt / 0.12).max(0.0);
            self.admin_eye_hover_t = (self.admin_eye_hover_t - dt / 0.12).max(0.0);
            return;
        }

        // Ust tema degistirme butonu (X: 338..370, Y: 13..43)
        let target_theme_btn = if mx >= 338.0 && mx <= 370.0 && my >= 13.0 && my <= 43.0 { 1.0 } else { 0.0 };
        self.theme_btn_hover_t += (target_theme_btn - self.theme_btn_hover_t) * speed;

        if self.page == ActivePage::Main {
            // Degistir butonu (X: 266..356, Y: 381..415)
            let target_dock_btn = if mx >= 266.0 && mx <= 356.0 && my >= 381.0 && my <= 415.0 { 1.0 } else { 0.0 };
            self.dock_btn_hover_t += (target_dock_btn - self.dock_btn_hover_t) * speed;

            // Kopyala butonu (pill acikken X: 254..348, Y: pill_y + 5..37)
            let pill_ease = directional_ease_out_cubic(self.pill_anim_t, self.state == ConnectionState::Connected);
            let pill_y = 272.0 + (1.0 - pill_ease) * 10.0;
            let target_copy_btn = if self.pill_anim_t > 0.5 && mx >= 254.0 && mx <= 348.0 && my >= (pill_y + 5.0) && my <= (pill_y + 37.0) {
                1.0
            } else {
                0.0
            };
            self.copy_btn_hover_t += (target_copy_btn - self.copy_btn_hover_t) * speed;
            self.code_eye_hover_t = (self.code_eye_hover_t - dt / 0.12).max(0.0);
            self.admin_eye_hover_t = (self.admin_eye_hover_t - dt / 0.12).max(0.0);
        } else {
            // Config sayfasindaki butonlar
            let (form_y, _form_h, extra_h) = self.config_form_y();
            let admin_ease = directional_ease_out_cubic(self.admin_expand_t, self.admin_key_expanded);
            let code_inp_y = form_y + 116.0;
            let admin_inp_y = form_y + 196.0 - (1.0 - admin_ease) * 6.0;
            let btn_y = form_y + 216.0 + extra_h;

            let target_back = if mx >= 20.0 && mx <= 88.0 && my >= 9.0 && my <= 43.0 { 1.0 } else { 0.0 };
            self.config_back_hover_t += (target_back - self.config_back_hover_t) * speed;

            let target_del = if self.dropdown_selected_idx > 0 && mx >= 318.0 && mx <= 370.0 && my >= 9.0 && my <= 43.0 { 1.0 } else { 0.0 };
            self.config_delete_hover_t += (target_del - self.config_delete_hover_t) * speed;

            let target_cancel = if mx >= 38.0 && mx <= 188.0 && my >= btn_y && my <= (btn_y + 42.0) { 1.0 } else { 0.0 };
            self.config_cancel_hover_t += (target_cancel - self.config_cancel_hover_t) * speed;

            let target_save = if mx >= 202.0 && mx <= 352.0 && my >= btn_y && my <= (btn_y + 42.0) { 1.0 } else { 0.0 };
            self.config_save_hover_t += (target_save - self.config_save_hover_t) * speed;

            // Baglanti Kodu Goz Butonu (X: 310..352, Y: code_inp_y..code_inp_y + 40)
            let eye_hover_step = dt / 0.12;
            let target_code_eye = if mx >= 310.0 && mx <= 352.0 && my >= code_inp_y && my <= (code_inp_y + 40.0) { 1.0 } else { 0.0 };
            if target_code_eye > self.code_eye_hover_t {
                self.code_eye_hover_t = (self.code_eye_hover_t + eye_hover_step).min(1.0);
            } else if target_code_eye < self.code_eye_hover_t {
                self.code_eye_hover_t = (self.code_eye_hover_t - eye_hover_step).max(0.0);
            }

            // Yonetici Anahtari Goz Butonu (X: 310..352, Y: admin_inp_y..admin_inp_y + 38)
            let target_admin_eye = if self.admin_key_expanded && mx >= 310.0 && mx <= 352.0 && my >= admin_inp_y && my <= (admin_inp_y + 38.0) { 1.0 } else { 0.0 };
            if target_admin_eye > self.admin_eye_hover_t {
                self.admin_eye_hover_t = (self.admin_eye_hover_t + eye_hover_step).min(1.0);
            } else if target_admin_eye < self.admin_eye_hover_t {
                self.admin_eye_hover_t = (self.admin_eye_hover_t - eye_hover_step).max(0.0);
            }
        }
    }

    pub fn render(&mut self) {
        self.canvas.clear(self.theme.window_bg);

        if self.page_transition_t < 1.0 && !self.page_from_buf.is_empty() && !self.page_to_buf.is_empty() {
            self.canvas.composite_page_slide(&self.page_from_buf, &self.page_to_buf, self.page_transition_t, self.page_slide_dir);
        } else {
            match self.page {
                ActivePage::Main => self.render_page_main(),
                ActivePage::Config => self.render_page_config(),
            }
        }

        // Tema gecis crossfade katmani
        if self.theme_crossfade_alpha > 0.001 && !self.theme_crossfade_buf.is_empty() {
            self.canvas.blend_from_buffer(&self.theme_crossfade_buf, self.theme_crossfade_alpha);
        }

        // Guncelleme Modali (v2.0.0)
        if self.show_update_modal && self.update_modal_t > 0.001 {
            self.render_update_modal();
        }
    }

    // --- SAYFA 1: ANA EKRAN ---
    fn render_page_main(&mut self) {
        let pal = self.theme;
        self.canvas.draw_architectural_backdrop(self.theme.is_dark);

        // 1. Ust Baslik Cubugu (Y: 14..46)
        self.canvas.draw_text_weight(FontWeight::Bold, 17.5, 20.0, 16.0, "maziTunnel", pal.text_primary);

        // Bagliyken Rol Rozeti (YÖNETİCİ / OYUNCU)
        if self.state == ConnectionState::Connected {
            let role_str = if self.is_admin { "YÖNETİCİ" } else { "OYUNCU" };
            let (rw, _) = self.canvas.measure_text_semibold(11.0, role_str);
            let pill_w = rw + 14.0;
            let (tw, _) = self.canvas.measure_text_weight(FontWeight::Bold, 17.5, "maziTunnel");
            let rx = 20.0 + tw + 10.0;
            let ry = 17.0;

            let (role_bg, role_border, role_fg) = if self.is_admin {
                if pal.is_dark {
                    (rgba(62, 42, 12, 255), rgba(180, 120, 24, 255), rgba(251, 191, 36, 255))
                } else {
                    (rgba(254, 243, 199, 255), rgba(217, 119, 6, 255), rgba(146, 64, 14, 255))
                }
            } else {
                if pal.is_dark {
                    (rgba(24, 42, 62, 255), rgba(48, 92, 142, 255), rgba(125, 211, 252, 255))
                } else {
                    (rgba(224, 242, 254, 255), rgba(56, 189, 248, 255), rgba(3, 105, 161, 255))
                }
            };

            self.canvas.fill_rounded_rect(rx, ry, pill_w, 22.0, 5.0, role_bg);
            self.canvas.stroke_rounded_rect(rx, ry, pill_w, 22.0, 5.0, 1.0, role_border);
            self.canvas.draw_text_centered_semibold(11.0, rx + pill_w * 0.5, ry + 11.0, role_str, role_fg);
        }

        // PingBadge (Yalnizca bagliyken ve ms degeri varken cizilir)
        if self.state == ConnectionState::Connected {
            if let Some(ms) = self.ping_ms {
                let ping_str = format!("{} ms", ms);
                let (ptw, _) = self.canvas.measure_text_semibold(11.0, &ping_str);
                let ping_w = (ptw + 14.0).max(44.0);
                let ping_h = 22.0f32;
                let ping_x = 330.0 - ping_w;
                let ping_y = 17.0f32;

                self.canvas.fill_rounded_rect(ping_x, ping_y, ping_w, ping_h, 5.5, pal.accent);
                self.canvas.draw_text_centered_semibold(11.0, ping_x + ping_w * 0.5, ping_y + ping_h * 0.5, &ping_str, pal.accent_text);
            }
        }

        // ThemeToggleButton (X: 338.0, Y: 13.0, W: 32.0, H: 30.0)
        let theme_btn_bg = lerp_color(pal.card_bg, pal.bg_hover, self.theme_btn_hover_t);
        self.canvas.fill_rounded_rect(338.0, 13.0, 32.0, 30.0, 7.0, theme_btn_bg);
        self.canvas.stroke_rounded_rect(338.0, 13.0, 32.0, 30.0, 7.0, 1.0, pal.card_border);
        self.canvas.draw_sun_moon_morph(354.0, 28.0, self.theme_morph_t, pal.text_primary, theme_btn_bg);

        // 2. Hero Card (X: 20.0, Y: 58.0, W: 350.0, H: 292.0, radius 20.0)
        let hero_x = 20.0f32;
        let hero_y = 58.0f32;
        let hero_w = 350.0f32;
        let hero_h = 292.0f32;

        self.canvas.fill_rounded_rect(hero_x, hero_y, hero_w, hero_h, 20.0, pal.card_bg);
        self.canvas.stroke_rounded_rect(hero_x, hero_y, hero_w, hero_h, 20.0, 1.0, pal.card_border);

        // TunnelSwitch (X: 125.0, W: 140.0, H: 68.0, radius 34.0, dikey ortalanmis)
        let sw_x = 125.0f32;
        let sw_y = self.current_switch_y();
        let sw_w = 140.0f32;
        let sw_h = 68.0f32;

        // Baglanirken genisleyen disa dalga halkasi (900ms nefes dongusu)
        if self.state == ConnectionState::Connecting {
            let pulse_phase = (self.anim_time * 1.1).fract();
            let pulse_expand = 8.0 * pulse_phase;
            let pulse_alpha = ((1.0 - pulse_phase) * 105.0).round() as u8;
            let pulse_col = if pal.is_dark {
                rgba(244, 244, 246, pulse_alpha)
            } else {
                rgba(20, 20, 23, pulse_alpha)
            };
            self.canvas.stroke_rounded_rect(
                sw_x - pulse_expand,
                sw_y - pulse_expand,
                sw_w + pulse_expand * 2.0,
                sw_h + pulse_expand * 2.0,
                34.0 + pulse_expand,
                1.8,
                pulse_col,
            );
        }

        // Hover Halesi
        if self.switch_hover > 0.01 {
            let halo_alpha = (26.0 * self.switch_hover) as u8;
            let halo_col = if pal.is_dark {
                rgba(244, 244, 246, halo_alpha)
            } else {
                rgba(20, 20, 23, halo_alpha)
            };
            self.canvas.fill_rounded_rect(sw_x - 4.0, sw_y - 4.0, sw_w + 8.0, sw_h + 8.0, 38.0, halo_col);
        }

        // Polarite degisim renkleri
        let clamped_pos = self.switch_pos.clamp(0.0, 1.0);
        let (track_bg, track_border, thumb_bg, icon_col) = if pal.is_dark {
            (
                lerp_color(rgba(32, 32, 38, 255), rgba(244, 244, 246, 255), clamped_pos),
                lerp_color(rgba(46, 46, 54, 255), rgba(244, 244, 246, 255), clamped_pos),
                lerp_color(rgba(244, 244, 246, 255), rgba(20, 20, 23, 255), clamped_pos),
                lerp_color(rgba(20, 20, 23, 255), rgba(244, 244, 246, 255), clamped_pos),
            )
        } else {
            (
                lerp_color(rgba(228, 228, 233, 255), rgba(20, 20, 23, 255), clamped_pos),
                lerp_color(rgba(208, 208, 216, 255), rgba(20, 20, 23, 255), clamped_pos),
                lerp_color(rgba(20, 20, 23, 255), rgba(255, 255, 255, 255), clamped_pos),
                lerp_color(rgba(255, 255, 255, 255), rgba(20, 20, 23, 255), clamped_pos),
            )
        };

        self.canvas.fill_rounded_rect(sw_x, sw_y, sw_w, sw_h, 34.0, track_bg);
        self.canvas.stroke_rounded_rect(sw_x, sw_y, sw_w, sw_h, 34.0, 1.5, track_border);

        // İc durum isaretcileri
        if clamped_pos > 0.05 {
            let line_a = (140.0 * clamped_pos) as u8;
            let line_col = if pal.is_dark { rgba(20, 20, 23, line_a) } else { rgba(255, 255, 255, line_a) };
            self.canvas.draw_line(sw_x + 24.0, sw_y + 24.0, sw_x + 24.0, sw_y + sw_h - 24.0, 2.2, line_col);
        }
        if (1.0 - clamped_pos) > 0.05 {
            let circ_a = (130.0 * (1.0 - clamped_pos)) as u8;
            let circ_col = if pal.is_dark { rgba(142, 142, 153, circ_a) } else { rgba(82, 82, 91, circ_a) };
            self.canvas.stroke_circle(sw_x + sw_w - 24.0, sw_y + sw_h * 0.5, 6.0, 2.0, circ_col);
        }

        // Basparmak (Thumb) geometrisi (Tactile stretch ve yay sekmesi ile)
        let margin = 6.0f32;
        let base_thumb_d = 56.0f32;
        let stretch = self.switch_press * 7.0;
        let thumb_w = base_thumb_d + stretch;
        let travel = sw_w - 12.0 - thumb_w;
        let raw_thumb_x = sw_x + margin + travel * self.switch_pos;
        let min_thumb_x = sw_x + margin - 1.5;
        let max_thumb_x = sw_x + sw_w - margin - thumb_w + 1.5;
        let thumb_x = raw_thumb_x.clamp(min_thumb_x, max_thumb_x);
        let thumb_y = sw_y + margin;

        let shadow_col = rgba(0, 0, 0, if pal.is_dark { 55 } else { 35 });
        self.canvas.fill_rounded_rect(thumb_x, thumb_y + 2.0, thumb_w, 56.0, 28.0, shadow_col);
        self.canvas.fill_rounded_rect(thumb_x, thumb_y, thumb_w, 56.0, 28.0, thumb_bg);

        let tcx = thumb_x + thumb_w * 0.5;
        let tcy = thumb_y + 28.0;

        if self.state == ConnectionState::Connecting {
            let ring_col = rgba(
                color_r(icon_col),
                color_g(icon_col),
                color_b(icon_col),
                45,
            );
            self.canvas.stroke_circle(tcx, tcy, 11.5, 1.8, ring_col);
            let spin_rad = self.anim_time * 7.85;
            self.canvas.stroke_arc(
                tcx,
                tcy,
                11.5,
                spin_rad,
                250.0 * (std::f32::consts::PI / 180.0),
                2.4,
                icon_col,
            );
        } else {
            self.canvas.draw_power_icon(tcx, tcy, 10.5, icon_col, 2.3);
        }

        // Hero Card Baslik ve Alt Metin (Ferah 20px bosluk ve akici metin solma)
        let title_y = sw_y + 102.0;
        let title_alpha_factor = self.title_fade_alpha * self.title_fade_alpha * (3.0 - 2.0 * self.title_fade_alpha);
        let title_alpha = (color_a(pal.text_primary) as f32 * title_alpha_factor).round() as u8;
        let title_col = rgba(
            color_r(pal.text_primary),
            color_g(pal.text_primary),
            color_b(pal.text_primary),
            title_alpha,
        );
        self.canvas.draw_text_centered(true, 22.0, 195.0, title_y, &self.display_title, title_col);

        // Status Subtitle (Word-wrapped to max 310px)
        let max_sub_w = 310.0f32;
        let mut sub_lines = Vec::new();
        let words: Vec<&str> = self.display_subtitle.split_whitespace().collect();
        let mut cur_line = String::new();
        for w in words {
            let cand = if cur_line.is_empty() { w.to_string() } else { format!("{} {}", cur_line, w) };
            if self.canvas.measure_text(false, 13.5, &cand).0 <= max_sub_w || cur_line.is_empty() {
                cur_line = cand;
            } else {
                sub_lines.push(cur_line);
                cur_line = w.to_string();
            }
        }
        if !cur_line.is_empty() {
            sub_lines.push(cur_line);
        }

        let start_sub_y = if sub_lines.len() <= 1 { title_y + 36.0 } else { title_y + 34.0 };
        let sub_alpha_factor = self.sub_fade_alpha * self.sub_fade_alpha * (3.0 - 2.0 * self.sub_fade_alpha);
        let sub_alpha = (color_a(pal.text_secondary) as f32 * sub_alpha_factor).round() as u8;
        let sub_col = rgba(
            color_r(pal.text_secondary),
            color_g(pal.text_secondary),
            color_b(pal.text_secondary),
            sub_alpha,
        );
        for (i, line) in sub_lines.iter().enumerate() {
            self.canvas.draw_text_centered(false, 13.5, 195.0, start_sub_y + (i as f32) * 20.0, line, sub_col);
        }

        // Animated AddressPill (X: 36.0, Y: 272.0, W: 318.0, H: 42.0, radius 12.0)
        if self.pill_anim_t > 0.01 {
            let pill_ease = directional_ease_out_cubic(self.pill_anim_t, self.state == ConnectionState::Connected);
            let pill_x = 36.0f32;
            let pill_y = 272.0 + (1.0 - pill_ease) * 10.0;
            let pill_w = 318.0f32;
            let pill_h = 42.0f32;

            let scale_alpha = |col: u32, factor: f32| -> u32 {
                let a = (color_a(col) as f32 * factor.clamp(0.0, 1.0)).round() as u8;
                rgba(color_r(col), color_g(col), color_b(col), a)
            };

            self.canvas.fill_rounded_rect(pill_x, pill_y, pill_w, pill_h, 12.0, scale_alpha(pal.pill_bg, pill_ease));
            self.canvas.stroke_rounded_rect(pill_x, pill_y, pill_w, pill_h, 12.0, 1.0, scale_alpha(pal.pill_border, pill_ease));

            // Sol monokrom durum noktasi (yesil degil!)
            let dot_col = if pal.is_dark { rgba(244, 244, 246, 255) } else { rgba(20, 20, 23, 255) };
            self.canvas.fill_circle(pill_x + 16.0, pill_y + 21.0, 4.0, scale_alpha(dot_col, pill_ease));

            // Adres metni
            self.canvas.draw_text_weight(FontWeight::Semibold, 14.5, pill_x + 28.0, pill_y + 12.0, &self.assigned_address, scale_alpha(pal.text_primary, pill_ease));

            // Kopyala Butonu (X: 256.0, Y: pill_y + 6.0, W: 90.0, H: 30.0)
            let copy_btn_x = pill_x + pill_w - 98.0;
            let copy_btn_y = pill_y + 6.0;
            let copy_btn_w = 90.0;
            let copy_btn_h = 30.0;

            let ce = ease_out_cubic(self.copy_anim_t);

            let normal_bg = lerp_color(pal.bg_hover, pal.accent, self.copy_btn_hover_t);
            let normal_fg = lerp_color(pal.text_primary, pal.accent_text, self.copy_btn_hover_t);
            let normal_border = lerp_color(pal.card_border, pal.accent, self.copy_btn_hover_t);

            let inv_bg = if pal.is_dark { rgba(244, 244, 246, 255) } else { rgba(20, 20, 23, 255) };
            let inv_fg = if pal.is_dark { rgba(20, 20, 23, 255) } else { rgba(244, 244, 246, 255) };
            let inv_border = inv_bg;

            let copy_bg = lerp_color(normal_bg, inv_bg, ce);
            let copy_fg = lerp_color(normal_fg, inv_fg, ce);
            let copy_border = lerp_color(normal_border, inv_border, ce);

            self.canvas.fill_rounded_rect(copy_btn_x, copy_btn_y, copy_btn_w, copy_btn_h, 8.0, scale_alpha(copy_bg, pill_ease));
            self.canvas.stroke_rounded_rect(copy_btn_x, copy_btn_y, copy_btn_w, copy_btn_h, 8.0, 1.0, scale_alpha(copy_border, pill_ease));

            if ce < 0.99 {
                let cy = copy_btn_y + copy_btn_h * 0.5 - 7.0 * ce;
                self.canvas.draw_text_centered_semibold(13.5, copy_btn_x + copy_btn_w * 0.5, cy, "Kopyala", scale_alpha(copy_fg, (1.0 - ce) * pill_ease));
            }
            if ce > 0.01 {
                let cy = copy_btn_y + copy_btn_h * 0.5 + 7.0 * (1.0 - ce);
                self.canvas.draw_text_centered_semibold(13.5, copy_btn_x + copy_btn_w * 0.5, cy, "Kopyalandı", scale_alpha(copy_fg, ce * pill_ease));
            }
        }

        // 3. Alt ServerDockCard (X: 20.0, Y: 364.0, W: 350.0, H: 68.0, radius 16.0)
        let dock_x = 20.0f32;
        let dock_y = 364.0f32;
        let dock_w = 350.0f32;
        let dock_h = 68.0f32;

        self.canvas.fill_rounded_rect(dock_x, dock_y, dock_w, dock_h, 16.0, pal.dock_bg);
        self.canvas.stroke_rounded_rect(dock_x, dock_y, dock_w, dock_h, 16.0, 1.0, pal.dock_border);

        // Dual-blade monokrom ServerNodeIcon (Sol tarafta cx = 52.0, cy = 398.0)
        let conn_t = if self.state == ConnectionState::Connected { 1.0 } else { 0.0 };
        self.canvas.draw_server_rack_node(52.0, 398.0, conn_t, pal.is_dark);

        let srv = self.config.servers.get(self.config.active_server_idx);
        let srv_name_text = match srv {
            Some(s) if !s.name.trim().is_empty() => s.name.as_str(),
            _ => "Sunucu Seçilmedi",
        };

        self.canvas.draw_text_weight(FontWeight::Semibold, 14.5, 82.0, dock_y + 14.0, srv_name_text, pal.text_primary);

        let sub_code_text = if self.state == ConnectionState::Connected {
            self.assigned_address.clone()
        } else {
            match srv {
                Some(s) if !s.code.trim().is_empty() => mask_server_key(&s.code),
                _ => "Yapılandırmak için dokunun".to_string(),
            }
        };
        self.canvas.draw_text(false, 12.5, 82.0, dock_y + 37.0, &sub_code_text, pal.text_secondary);

        // Sag Degistir Butonu (X: 266.0, Y: 381.0, W: 90.0, H: 34.0, radius 10.0)
        let btn_edit_x = 266.0f32;
        let btn_edit_y = 381.0f32;
        let btn_edit_w = 90.0f32;
        let btn_edit_h = 34.0f32;

        let edit_bg = lerp_color(pal.bg_hover, pal.accent, self.dock_btn_hover_t);
        let edit_fg = lerp_color(pal.text_primary, pal.accent_text, self.dock_btn_hover_t);

        self.canvas.fill_rounded_rect(btn_edit_x, btn_edit_y, btn_edit_w, btn_edit_h, 10.0, edit_bg);
        self.canvas.draw_text_centered_semibold(13.5, btn_edit_x + btn_edit_w * 0.5, btn_edit_y + btn_edit_h * 0.5, "Değiştir", edit_fg);

        // 4. Footer (Sag alt kose)
        let (vw, _) = self.canvas.measure_text_semibold(11.5, "v2.0.1");
        self.canvas.draw_text_semibold(11.5, 368.0 - vw, 464.0, "v2.0.1", pal.text_muted);
    }

    // --- SAYFA 2: SUNUCU AYARLARI ---
    fn render_page_config(&mut self) {
        let pal = self.theme;
        self.canvas.draw_architectural_backdrop(self.theme.is_dark);

        // 1. Ust Navigasyon Cubugu (Y: 10.0)
        let back_btn_x = 20.0f32;
        let back_btn_y = 10.0f32;
        let back_btn_w = 68.0f32;
        let back_btn_h = 30.0f32;

        let back_bg = lerp_color(pal.card_bg, pal.bg_hover, self.config_back_hover_t);
        self.canvas.fill_rounded_rect(back_btn_x, back_btn_y, back_btn_w, back_btn_h, 8.0, back_bg);
        self.canvas.stroke_rounded_rect(back_btn_x, back_btn_y, back_btn_w, back_btn_h, 8.0, 1.0, pal.card_border);
        self.canvas.draw_text_centered(true, 14.5, back_btn_x + back_btn_w * 0.5, back_btn_y + back_btn_h * 0.5, "< Geri", pal.text_primary);

        self.canvas.draw_text_centered(true, 17.5, 195.0, 25.0, "Sunucu Ayarları", pal.text_primary);

        // Sil Butonu (Yalnizca secili kayitli sunucu varsa gosterilir)
        if self.dropdown_selected_idx > 0 {
            let del_btn_x = 318.0f32;
            let del_btn_y = 10.0f32;
            let del_btn_w = 52.0f32;
            let del_btn_h = 30.0f32;

            let del_alpha = (30.0 + self.config_delete_hover_t * 50.0) as u8;
            self.canvas.fill_rounded_rect(del_btn_x, del_btn_y, del_btn_w, del_btn_h, 8.0, rgba(239, 68, 68, del_alpha));
            self.canvas.stroke_rounded_rect(del_btn_x, del_btn_y, del_btn_w, del_btn_h, 8.0, 1.0, rgba(239, 68, 68, 100));
            self.canvas.draw_text_centered(true, 14.5, del_btn_x + del_btn_w * 0.5, del_btn_y + del_btn_h * 0.5, "Sil", rgba(248, 113, 113, 255));
        }

        // 2. Ana Form Karti
        let (form_y, form_h, extra_h) = self.config_form_y();
        let admin_ease = directional_ease_out_cubic(self.admin_expand_t, self.admin_key_expanded);
        let form_x = 20.0f32;
        let form_w = 350.0f32;

        self.canvas.fill_rounded_rect(form_x, form_y, form_w, form_h, 18.0, pal.card_bg);
        self.canvas.stroke_rounded_rect(form_x, form_y, form_w, form_h, 18.0, 1.0, pal.card_border);

        // KAYITLI SUNUCULAR Etiketi (form_y + 20.0)
        self.canvas.draw_text(true, 13.0, 38.0, form_y + 20.0, "KAYITLI SUNUCULAR", pal.text_muted);

        // Downward Dropdown Secim Kutusu (combo_y = form_y + 40.0)
        let combo_x = 38.0f32;
        let combo_y = form_y + 40.0;
        let combo_w = 314.0f32;
        let combo_h = 40.0f32;

        let combo_border = if self.dropdown_open { pal.input_border_focus } else { pal.input_border };
        self.canvas.fill_rounded_rect(combo_x, combo_y, combo_w, combo_h, 8.0, pal.input_bg);
        self.canvas.stroke_rounded_rect(combo_x, combo_y, combo_w, combo_h, 8.0, 1.0, combo_border);

        let selected_text = if self.dropdown_selected_idx == 0 {
            "+ Yeni Sunucu Ekle".to_string()
        } else if let Some(s) = self.config.servers.get(self.dropdown_selected_idx - 1) {
            let masked = mask_server_key(&s.code);
            if masked.is_empty() {
                s.name.clone()
            } else {
                format!("{}  •  {}", s.name, masked)
            }
        } else {
            "+ Yeni Sunucu Ekle".to_string()
        };

        self.canvas.draw_text(true, 15.5, combo_x + 14.0, combo_y + 11.0, &selected_text, pal.text_primary);
        let drop_ease = directional_ease_out_cubic(self.dropdown_anim_t, self.dropdown_open);
        self.canvas.draw_chevron(combo_x + combo_w - 20.0, combo_y + combo_h * 0.5, 5.5, drop_ease, pal.text_secondary, 1.8);

        // BAĞLANTI ANAHTARI Etiketi (form_y + 96.0)
        self.canvas.draw_text(true, 13.0, 38.0, form_y + 96.0, "BAĞLANTI ANAHTARI", pal.text_muted);

        // Baglanti Kodu Giris Kutusu (code_inp_y = form_y + 116.0)
        let code_inp_x = 38.0f32;
        let code_inp_y = form_y + 116.0;
        let code_inp_w = 314.0f32;
        let code_inp_h = 40.0f32;

        let code_border = if self.active_input == ActiveInput::ServerCode {
            pal.input_border_focus
        } else {
            pal.input_border
        };
        self.canvas.fill_rounded_rect(code_inp_x, code_inp_y, code_inp_w, code_inp_h, 8.0, pal.input_bg);
        self.canvas.stroke_rounded_rect(code_inp_x, code_inp_y, code_inp_w, code_inp_h, 8.0, 1.0, code_border);

        let is_editing_code = self.active_input == ActiveInput::ServerCode;

        if self.edit_code.is_empty() {
            self.canvas.draw_text(true, 16.0, code_inp_x + 14.0, code_inp_y + 11.0, "Örn: A1B2-C3D4-E5F6", pal.text_muted);
        } else {
            let display_code = self.get_displayed_input_string(ActiveInput::ServerCode);
            let total_chars = display_code.chars().count();
            let (sel_min, sel_max) = if self.input_select_all {
                (0, total_chars)
            } else {
                let c = self.input_cursor.min(total_chars);
                let a = self.input_sel_anchor.min(total_chars);
                (c.min(a), c.max(a))
            };

            if is_editing_code && sel_min < sel_max {
                let prefix_min: String = display_code.chars().take(sel_min).collect();
                let prefix_max: String = display_code.chars().take(sel_max).collect();
                let (x0, _) = self.canvas.measure_text(true, 16.0, &prefix_min);
                let (x1, _) = self.canvas.measure_text(true, 16.0, &prefix_max);
                self.canvas.fill_rounded_rect(
                    code_inp_x + 14.0 + x0,
                    code_inp_y + 9.0,
                    (x1 - x0).max(2.0),
                    22.0,
                    4.0,
                    rgba(59, 130, 246, 95),
                );
            }

            render_secret_field(
                &mut self.canvas,
                &self.edit_code,
                true,
                self.code_eye_t,
                code_inp_x + 14.0,
                code_inp_y + 11.0,
                16.0,
                pal.text_primary,
            );

            if is_editing_code && sel_min == sel_max && (self.cursor_blink.sin() > 0.0) {
                let cur = self.input_cursor.min(total_chars);
                let prefix: String = display_code.chars().take(cur).collect();
                let (tw, _) = self.canvas.measure_text(true, 16.0, &prefix);
                self.canvas.draw_line(
                    code_inp_x + 14.0 + tw,
                    code_inp_y + 9.0,
                    code_inp_x + 14.0 + tw,
                    code_inp_y + 31.0,
                    1.8,
                    pal.text_primary,
                );
            }
        }

        // Sag Goz (Goster / Gizle) Butonu (code_inp_y + 4.0)
        let btn_eye_x = 312.0f32;
        let btn_eye_y = code_inp_y + 4.0;
        let btn_eye_w = 34.0f32;
        let btn_eye_h = 32.0f32;
        let eye_col = lerp_color(pal.text_muted, pal.text_primary, self.code_eye_t.max(self.code_eye_hover_t * 0.7));
        self.canvas.draw_icon_eye_animated(
            btn_eye_x + btn_eye_w * 0.5,
            btn_eye_y + btn_eye_h * 0.5,
            self.code_eye_t,
            self.code_eye_hover_t,
            eye_col,
            pal.input_bg,
        );

        // Yönetici Anahtarı Acilir Satiri (form_y + 170.0)
        let admin_lbl_y = form_y + 170.0;
        let admin_lbl = "Yönetici Anahtarı";
        let toggle_col = if self.admin_key_expanded || self.admin_expand_t > 0.1 {
            pal.text_primary
        } else {
            pal.text_secondary
        };
        self.canvas.draw_text(true, 13.5, 38.0, admin_lbl_y, admin_lbl, toggle_col);
        let (lbl_w, _) = self.canvas.measure_text(true, 13.5, admin_lbl);
        self.canvas.draw_chevron(38.0 + lbl_w + 12.0, admin_lbl_y + 8.0, 4.5, admin_ease, toggle_col, 1.6);

        // Yonetici Anahtari Giris Kutusu (Animasyonlu Genisleme ve Y-Slide)
        if self.admin_expand_t > 0.01 {
            let admin_inp_x = 38.0f32;
            let admin_inp_y = form_y + 196.0 - (1.0 - admin_ease) * 6.0;
            let admin_inp_w = 314.0f32;
            let admin_inp_h = 38.0f32;

            let adm_border = if self.active_input == ActiveInput::AdminKey {
                pal.input_border_focus
            } else {
                pal.input_border
            };
            self.canvas.fill_rounded_rect(admin_inp_x, admin_inp_y, admin_inp_w, admin_inp_h, 8.0, scale_alpha(pal.input_bg, admin_ease));
            self.canvas.stroke_rounded_rect(admin_inp_x, admin_inp_y, admin_inp_w, admin_inp_h, 8.0, 1.0, scale_alpha(adm_border, admin_ease));

            let is_editing_adm = self.active_input == ActiveInput::AdminKey;
            if self.edit_admin_key.is_empty() {
                self.canvas.draw_text(
                    false,
                    14.5,
                    admin_inp_x + 14.0,
                    admin_inp_y + 10.0,
                    "Yönetici anahtarı (isteğe bağlı)",
                    scale_alpha(pal.text_muted, admin_ease),
                );
            } else {
                let display_adm = self.get_displayed_input_string(ActiveInput::AdminKey);
                let total_chars = display_adm.chars().count();
                let (sel_min, sel_max) = if self.input_select_all {
                    (0, total_chars)
                } else {
                    let c = self.input_cursor.min(total_chars);
                    let a = self.input_sel_anchor.min(total_chars);
                    (c.min(a), c.max(a))
                };

                if is_editing_adm && sel_min < sel_max {
                    let prefix_min: String = display_adm.chars().take(sel_min).collect();
                    let prefix_max: String = display_adm.chars().take(sel_max).collect();
                    let (x0, _) = self.canvas.measure_text(false, 14.5, &prefix_min);
                    let (x1, _) = self.canvas.measure_text(false, 14.5, &prefix_max);
                    self.canvas.fill_rounded_rect(
                        admin_inp_x + 14.0 + x0,
                        admin_inp_y + 8.0,
                        (x1 - x0).max(2.0),
                        22.0,
                        4.0,
                        scale_alpha(rgba(59, 130, 246, 95), admin_ease),
                    );
                }

                render_secret_field(
                    &mut self.canvas,
                    &self.edit_admin_key,
                    false,
                    self.admin_eye_t,
                    admin_inp_x + 14.0,
                    admin_inp_y + 10.0,
                    14.5,
                    scale_alpha(pal.text_primary, admin_ease),
                );

                if is_editing_adm && sel_min == sel_max && (self.cursor_blink.sin() > 0.0) {
                    let cur = self.input_cursor.min(total_chars);
                    let prefix: String = display_adm.chars().take(cur).collect();
                    let (tw, _) = self.canvas.measure_text(false, 14.5, &prefix);
                    self.canvas.draw_line(
                        admin_inp_x + 14.0 + tw,
                        admin_inp_y + 8.0,
                        admin_inp_x + 14.0 + tw,
                        admin_inp_y + 30.0,
                        1.8,
                        scale_alpha(pal.text_primary, admin_ease),
                    );
                }
            }

            // Yonetici Sag Goz Butonu
            let btn_adm_eye_x = 312.0f32;
            let btn_adm_eye_y = admin_inp_y + 4.0;
            let btn_adm_eye_w = 34.0f32;
            let btn_adm_eye_h = 30.0f32;
            let adm_eye_col = lerp_color(pal.text_muted, pal.text_primary, self.admin_eye_t.max(self.admin_eye_hover_t * 0.7));
            self.canvas.draw_icon_eye_animated(
                btn_adm_eye_x + btn_adm_eye_w * 0.5,
                btn_adm_eye_y + btn_adm_eye_h * 0.5,
                self.admin_eye_t,
                self.admin_eye_hover_t,
                scale_alpha(adm_eye_col, admin_ease),
                scale_alpha(pal.input_bg, admin_ease),
            );
        }

        // Alt Aksiyon Butonlari: Vazgec ve Kaydet (form_y + 216.0 + extra_h, H: 42.0)
        let btn_y = form_y + 216.0 + extra_h;
        let btn_cancel_x = 38.0f32;
        let btn_cancel_y = btn_y;
        let btn_cancel_w = 150.0f32;
        let btn_cancel_h = 42.0f32;

        let cancel_bg = lerp_color(pal.card_bg, pal.bg_hover, self.config_cancel_hover_t);
        self.canvas.fill_rounded_rect(btn_cancel_x, btn_cancel_y, btn_cancel_w, btn_cancel_h, 10.0, cancel_bg);
        self.canvas.stroke_rounded_rect(btn_cancel_x, btn_cancel_y, btn_cancel_w, btn_cancel_h, 10.0, 1.0, pal.card_border);
        self.canvas.draw_text_centered_semibold(14.5, btn_cancel_x + btn_cancel_w * 0.5, btn_cancel_y + btn_cancel_h * 0.5, "Vazgeç", pal.text_primary);

        let btn_save_x = 202.0f32;
        let btn_save_y = btn_y;
        let btn_save_w = 150.0f32;
        let btn_save_h = 42.0f32;

        let save_bg = lerp_color(pal.accent, pal.bg_hover, self.config_save_hover_t);
        let save_fg = lerp_color(pal.accent_text, pal.text_primary, self.config_save_hover_t);
        self.canvas.fill_rounded_rect(btn_save_x, btn_save_y, btn_save_w, btn_save_h, 10.0, save_bg);
        self.canvas.draw_text_centered_semibold(14.5, btn_save_x + btn_save_w * 0.5, btn_save_y + btn_save_h * 0.5, "Kaydet", save_fg);

        // 3. Downward Dropdown Acilir Menusu (Tum formun uzerine cizilir!)
        if self.dropdown_anim_t > 0.01 {
            let drop_ease = directional_ease_out_cubic(self.dropdown_anim_t, self.dropdown_open);
            let y_slide = (1.0 - drop_ease) * -6.0;
            let drop_x = combo_x;
            let drop_y = combo_y + 44.0 + y_slide;
            let drop_w = combo_w;
            let item_h = 36.0f32;

            let total_items = 1 + self.config.servers.len();
            let total_h = (total_items as f32 * item_h).min(200.0);
            let drop_h = total_h;

            // Menü golgesi ve arkaplani
            self.canvas.fill_rounded_rect(drop_x, drop_y + 3.0, drop_w, drop_h, 10.0, scale_alpha(rgba(0, 0, 0, 50), drop_ease));
            self.canvas.fill_rounded_rect(drop_x, drop_y, drop_w, drop_h, 10.0, scale_alpha(pal.card_bg, drop_ease));
            self.canvas.stroke_rounded_rect(drop_x, drop_y, drop_w, drop_h, 10.0, 1.0, scale_alpha(pal.card_border, drop_ease));

            for i in 0..total_items {
                let iy = drop_y + (i as f32 * item_h);
                if (iy + item_h) > (drop_y + drop_h + 2.0) {
                    break;
                }

                let is_hover = self.dropdown_hover_idx == Some(i);
                let is_selected = self.dropdown_selected_idx == i;

                if is_hover || is_selected {
                    let h_col = if is_selected { pal.dock_bg } else { pal.bg_hover };
                    self.canvas.fill_rounded_rect(drop_x + 3.0, iy + 2.0, drop_w - 6.0, item_h - 4.0, 7.0, scale_alpha(h_col, drop_ease));
                }

                let (label, is_bold) = if i == 0 {
                    ("+ Yeni Sunucu Ekle".to_string(), true)
                } else {
                    let s = &self.config.servers[i - 1];
                    let m = mask_server_key(&s.code);
                    (format!("{}  •  {}", s.name, m), false)
                };

                let col = if is_selected { pal.text_primary } else { pal.text_secondary };
                self.canvas.draw_text(is_bold, 14.5, drop_x + 14.0, iy + 9.0, &label, scale_alpha(col, drop_ease));
            }
        }

        // 4. Footer (Sag alt kose)
        let (vw, _) = self.canvas.measure_text_semibold(11.5, "v2.0.1");
        self.canvas.draw_text_semibold(11.5, 368.0 - vw, 464.0, "v2.0.1", pal.text_muted);
    }

    // --- GUNCELLEME MODALI (v2.0.0) ---
    fn render_update_modal(&mut self) {
        let pal = self.theme;
        let ease_t = directional_ease_out_cubic(self.update_modal_t, !self.update_modal_closing);

        // Arka plan karartmasi
        self.canvas.fill_rect(
            0.0,
            0.0,
            WINDOW_WIDTH as f32,
            WINDOW_HEIGHT as f32,
            rgba(0, 0, 0, (185.0 * ease_t) as u8),
        );

        let card_w = 346.0f32;
        let card_h = 374.0f32;
        let card_x = 22.0f32;
        let card_y = 58.0 + (1.0 - ease_t) * 16.0;

        let modal_bg = if pal.is_dark {
            rgba(20, 20, 23, (255.0 * ease_t) as u8)
        } else {
            rgba(255, 255, 255, (255.0 * ease_t) as u8)
        };
        let modal_border = if pal.is_dark {
            rgba(45, 45, 55, (255.0 * ease_t) as u8)
        } else {
            rgba(215, 215, 222, (255.0 * ease_t) as u8)
        };

        self.canvas.fill_rounded_rect(card_x, card_y, card_w, card_h, 18.0, modal_bg);
        self.canvas.stroke_rounded_rect(card_x, card_y, card_w, card_h, 18.0, 1.0, modal_border);

        // Ikon ve Basliklar
        self.canvas.draw_rgba_icon(40, (card_y + 20.0) as i32, 28, 28, APP_ICON_28, ease_t);
        self.canvas.draw_text(true, 19.5, 76.0, card_y + 18.0, "Güncelleme Tamamlandı", pal.text_primary);
        self.canvas.draw_text(true, 14.5, 76.0, card_y + 42.0, "v2.0.1 Sürüm Notları", pal.text_secondary);

        // Ayirici cizgi
        self.canvas.draw_line(card_x + 18.0, card_y + 70.0, card_x + card_w - 18.0, card_y + 70.0, 1.0, pal.card_border);

        // Surum maddeleri
        let mut cur_y = card_y + 84.0;
        for note in CHANGELOG_NOTES {
            cur_y = self.canvas.draw_text_wrapped(note, card_x + 18.0, cur_y, 310.0, 20.0, 14.5, pal.text_secondary, false);
            cur_y += 8.0;
        }

        // Alt Buton (Harika, Baslayalim)
        let btn_x = 40.0f32;
        let btn_y = card_y + card_h - 56.0;
        let btn_w = 310.0f32;
        let btn_h = 42.0f32;

        let btn_bg = lerp_color(pal.accent, pal.bg_hover, self.update_btn_hover_t);
        let btn_fg = lerp_color(pal.accent_text, pal.text_primary, self.update_btn_hover_t);

        self.canvas.fill_rounded_rect(btn_x, btn_y, btn_w, btn_h, 10.0, btn_bg);
        self.canvas.draw_text_centered(true, 15.5, btn_x + btn_w * 0.5, btn_y + btn_h * 0.5, "Harika, Başlayalım", btn_fg);
    }

    // --- FARE OLAYLARI ---
    pub fn on_mouse_move(&mut self, x: f32, y: f32) {
        self.mouse_x = x;
        self.mouse_y = y;

        if self.mouse_pressed && self.pressed_control.as_deref() == Some("switch") {
            let dx = x - self.switch_drag_start_x;
            if dx.abs() > 8.0 {
                self.switch_is_dragging = true;
            }
            if self.switch_is_dragging {
                self.switch_pos = (self.switch_drag_start_pos + dx / 72.0).clamp(0.0, 1.0);
            }
        }

        if self.mouse_pressed && self.input_is_selecting && self.active_input != ActiveInput::None {
            let (is_code, font_size, inp_x) = match self.active_input {
                ActiveInput::ServerCode => (true, 16.0, 38.0f32),
                ActiveInput::AdminKey => (false, 14.5, 38.0f32),
                _ => (false, 14.5, 38.0f32),
            };
            let disp = self.get_displayed_input_string(self.active_input);
            let idx = self.char_index_from_rel_x(is_code, font_size, &disp, x - (inp_x + 14.0));
            self.input_cursor = idx;
            self.input_select_all = false;
        }

        if self.page == ActivePage::Config && self.dropdown_open {
            let (form_y, _, _) = self.config_form_y();
            let combo_x = 38.0f32;
            let combo_y = form_y + 40.0;
            let drop_y = combo_y + 44.0;
            let drop_w = 314.0f32;
            let item_h = 36.0f32;
            let total_items = 1 + self.config.servers.len();

            let mut hovered = None;
            for i in 0..total_items {
                let iy = drop_y + (i as f32 * item_h);
                if x >= combo_x && x <= combo_x + drop_w && y >= iy && y <= (iy + item_h) {
                    hovered = Some(i);
                    break;
                }
            }
            self.dropdown_hover_idx = hovered;
        } else {
            self.dropdown_hover_idx = None;
        }
    }

    pub fn on_mouse_down(&mut self, x: f32, y: f32, #[allow(unused_variables)] hwnd: isize) {
        self.mouse_x = x;
        self.mouse_y = y;
        self.mouse_pressed = true;

        if self.show_update_modal {
            let card_x = 22.0f32;
            let card_w = 346.0f32;
            let card_y = 58.0f32;
            let card_h = 374.0f32;
            let btn_x = 40.0f32;
            let btn_y = card_y + card_h - 56.0;
            let btn_w = 310.0f32;
            let btn_h = 42.0f32;

            let on_btn = x >= btn_x && x <= btn_x + btn_w && y >= btn_y && y <= btn_y + btn_h;
            let outside_card = x < card_x || x > card_x + card_w || y < card_y || y > card_y + card_h;

            if on_btn || outside_card {
                self.close_update_modal();
            }
            return;
        }

        match self.page {
            ActivePage::Main => {
                // TunnelSwitch tiklamasi
                let sw_y = self.current_switch_y();
                if x >= 125.0 && x <= 265.0 && y >= sw_y && y <= (sw_y + 68.0) {
                    self.pressed_control = Some("switch".to_string());
                    self.switch_drag_start_x = x;
                    self.switch_drag_start_pos = self.switch_pos;
                    self.switch_is_dragging = false;
                    return;
                }

                // Tema Butonu (X: 338..370, Y: 13..43)
                if x >= 338.0 && x <= 370.0 && y >= 13.0 && y <= 43.0 {
                    self.toggle_theme();
                    #[cfg(windows)]
                    unsafe {
                        apply_dwm_theme(hwnd as _, self.theme.is_dark);
                        if !self.tray_popup_hwnd.is_null() {
                            apply_dwm_theme(self.tray_popup_hwnd, self.theme.is_dark);
                        }
                        if !self.tray_subpopup_hwnd.is_null() {
                            apply_dwm_theme(self.tray_subpopup_hwnd, self.theme.is_dark);
                        }
                    }
                    return;
                }

                // Kopyala Butonu (pill acikken X: 254..348)
                if self.pill_anim_t > 0.5 {
                    let pill_ease = directional_ease_out_cubic(self.pill_anim_t, self.state == ConnectionState::Connected);
                    let pill_y = 272.0 + (1.0 - pill_ease) * 10.0;
                    if x >= 254.0 && x <= 348.0 && y >= (pill_y + 5.0) && y <= (pill_y + 37.0) {
                        self.copy_feedback_timer = 2.0;
                        #[cfg(windows)]
                        copy_to_clipboard(hwnd as _, &self.assigned_address);
                        return;
                    }
                }

                // Degistir Butonu (Alt Dock - X: 266..356, Y: 381..415)
                if x >= 266.0 && x <= 356.0 && y >= 381.0 && y <= 415.0 {
                    let idx = if self.config.servers.is_empty() { 0 } else { self.config.active_server_idx + 1 };
                    self.open_config_page(idx);
                    return;
                }
            }
            ActivePage::Config => {
                let (form_y, _form_h, extra_h) = self.config_form_y();
                let admin_ease = directional_ease_out_cubic(self.admin_expand_t, self.admin_key_expanded);
                let combo_x = 38.0f32;
                let combo_y = form_y + 40.0;
                let code_inp_y = form_y + 116.0;
                let admin_lbl_y = form_y + 170.0;
                let admin_inp_y = form_y + 196.0 - (1.0 - admin_ease) * 6.0;
                let btn_y = form_y + 216.0 + extra_h;

                // Dropdown acikken tiklama kontrolu
                if self.dropdown_open {
                    let drop_y = combo_y + 44.0;
                    let drop_w = 314.0f32;
                    let item_h = 36.0f32;
                    let total_items = 1 + self.config.servers.len();

                    for i in 0..total_items {
                        let iy = drop_y + (i as f32 * item_h);
                        if x >= combo_x && x <= combo_x + drop_w && y >= iy && y <= (iy + item_h) {
                            self.dropdown_selected_idx = i;
                            if i == 0 {
                                self.edit_name.clear();
                                self.edit_code.clear();
                                self.edit_admin_key.clear();
                                self.admin_key_expanded = false;
                                self.admin_expand_t = 0.0;
                                self.active_input = ActiveInput::ServerCode;
                            } else {
                                let s = &self.config.servers[i - 1];
                                self.edit_name = s.name.clone();
                                self.edit_code = s.code.clone();
                                self.edit_admin_key = s.admin_key.clone();
                                self.admin_key_expanded = false;
                                self.admin_expand_t = 0.0;
                                self.active_input = ActiveInput::None;
                            }
                            self.input_cursor = self.edit_code.chars().count();
                            self.input_sel_anchor = self.input_cursor;
                            self.input_is_selecting = false;
                            self.dropdown_open = false;
                            return;
                        }
                    }
                    self.dropdown_open = false;
                    return;
                }

                // Geri Butonu (X: 20..88, Y: 9..43)
                if x >= 20.0 && x <= 88.0 && y >= 9.0 && y <= 43.0 {
                    self.switch_to_page(ActivePage::Main);
                    self.active_input = ActiveInput::None;
                    self.dropdown_open = false;
                    return;
                }

                // Sil Butonu (X: 318..370, Y: 9..43)
                if self.dropdown_selected_idx > 0 && x >= 318.0 && x <= 370.0 && y >= 9.0 && y <= 43.0 {
                    let srv_idx = self.dropdown_selected_idx - 1;
                    if srv_idx < self.config.servers.len() {
                        self.config.servers.remove(srv_idx);
                        if self.config.active_server_idx >= self.config.servers.len() {
                            self.config.active_server_idx = self.config.servers.len().saturating_sub(1);
                        }
                        self.sync_and_save_servers();
                        self.open_config_page(if self.config.servers.is_empty() { 0 } else { self.config.active_server_idx + 1 });
                    }
                    return;
                }

                // Dropdown Basligi (X: 38..352, Y: combo_y..(combo_y + 40.0))
                if x >= 38.0 && x <= 352.0 && y >= combo_y && y <= (combo_y + 40.0) {
                    self.dropdown_open = !self.dropdown_open;
                    self.active_input = ActiveInput::None;
                    return;
                }

                // Baglanti Kodu Sag Goz Butonu (X: 310..352, Y: code_inp_y..(code_inp_y + 40.0))
                if x >= 310.0 && x <= 352.0 && y >= code_inp_y && y <= (code_inp_y + 40.0) {
                    self.show_server_code = !self.show_server_code;
                    return;
                }

                // Baglanti Kodu Kutusu (X: 38..310, Y: code_inp_y..(code_inp_y + 40.0))
                if x >= 38.0 && x < 310.0 && y >= code_inp_y && y <= (code_inp_y + 40.0) {
                    self.active_input = ActiveInput::ServerCode;
                    let disp = self.get_displayed_input_string(ActiveInput::ServerCode);
                    let idx = self.char_index_from_rel_x(true, 16.0, &disp, x - (38.0 + 14.0));
                    self.input_cursor = idx;
                    self.input_sel_anchor = idx;
                    self.input_is_selecting = true;
                    self.input_select_all = false;
                    self.cursor_blink = 0.5;
                    self.dropdown_open = false;
                    return;
                }

                // Yonetici Anahtari Acma Satiri (X: 38..220, Y: (admin_lbl_y - 4.0)..(admin_lbl_y + 22.0))
                if x >= 38.0 && x <= 220.0 && y >= (admin_lbl_y - 4.0) && y <= (admin_lbl_y + 22.0) {
                    self.admin_key_expanded = !self.admin_key_expanded;
                    if !self.admin_key_expanded && self.active_input == ActiveInput::AdminKey {
                        self.active_input = ActiveInput::None;
                    }
                    return;
                }

                // Yonetici Anahtari Sag Goz Butonu (X: 310..352, Y: admin_inp_y..(admin_inp_y + 38.0))
                if self.admin_key_expanded && x >= 310.0 && x <= 352.0 && y >= admin_inp_y && y <= (admin_inp_y + 38.0) {
                    self.show_admin_key = !self.show_admin_key;
                    return;
                }

                // Yonetici Anahtari Kutusu (X: 38..310, Y: admin_inp_y..(admin_inp_y + 38.0))
                if self.admin_key_expanded && x >= 38.0 && x < 310.0 && y >= admin_inp_y && y <= (admin_inp_y + 38.0) {
                    self.active_input = ActiveInput::AdminKey;
                    let disp = self.get_displayed_input_string(ActiveInput::AdminKey);
                    let idx = self.char_index_from_rel_x(false, 14.5, &disp, x - (38.0 + 14.0));
                    self.input_cursor = idx;
                    self.input_sel_anchor = idx;
                    self.input_is_selecting = true;
                    self.input_select_all = false;
                    self.cursor_blink = 0.5;
                    self.dropdown_open = false;
                    return;
                }

                // Vazgec Butonu (X: 38..188, Y: btn_y..(btn_y + 42.0))
                if x >= 38.0 && x <= 188.0 && y >= btn_y && y <= (btn_y + 42.0) {
                    self.switch_to_page(ActivePage::Main);
                    self.active_input = ActiveInput::None;
                    self.dropdown_open = false;
                    return;
                }

                // Kaydet Butonu (X: 202..352, Y: btn_y..(btn_y + 42.0))
                if x >= 202.0 && x <= 352.0 && y >= btn_y && y <= (btn_y + 42.0) {
                    self.save_current_edit();
                    self.switch_to_page(ActivePage::Main);
                    self.active_input = ActiveInput::None;
                    self.dropdown_open = false;
                    return;
                }

                self.active_input = ActiveInput::None;
                self.dropdown_open = false;
            }
        }
    }

    pub fn on_mouse_up(&mut self, x: f32, y: f32) {
        self.mouse_pressed = false;
        self.input_is_selecting = false;

        if let Some(ctrl) = self.pressed_control.take() {
            let sw_y = self.current_switch_y();
            if ctrl == "switch" {
                let was_dragging = self.switch_is_dragging;
                self.switch_is_dragging = false;
                let dx = x - self.switch_drag_start_x;
                let is_active = self.state == ConnectionState::Connected || self.state == ConnectionState::Connecting;
                if !was_dragging || dx.abs() < 12.0 {
                    if x >= 125.0 && x <= 265.0 && y >= sw_y && y <= (sw_y + 68.0) {
                        self.toggle_connection();
                    } else {
                        self.switch_anim_from = self.switch_pos;
                        self.switch_anim_to = if is_active { 1.0 } else { 0.0 };
                        self.switch_anim_progress = 0.0;
                    }
                } else {
                    if is_active {
                        if self.switch_pos <= 0.68 {
                            self.toggle_connection();
                        } else {
                            self.switch_anim_from = self.switch_pos;
                            self.switch_anim_to = 1.0;
                            self.switch_anim_progress = 0.0;
                        }
                    } else {
                        if self.switch_pos >= 0.32 {
                            self.toggle_connection();
                        } else {
                            self.switch_anim_from = self.switch_pos;
                            self.switch_anim_to = 0.0;
                            self.switch_anim_progress = 0.0;
                        }
                    }
                }
            }
        }
    }

    pub fn close_update_modal(&mut self) {
        self.update_modal_closing = true;
        self.config.last_seen_version = "2.0.1".to_string();
        save_config(&self.config);
        self.core.config.lock().unwrap().last_seen_version = "2.0.1".to_string();
        let _ = self.core.config.lock().unwrap().save();
    }

    pub fn save_current_edit(&mut self) {
        let norm_code = normalize_server_key(&self.edit_code);
        self.edit_code = norm_code.clone();

        let clean_name = if self.edit_name.trim().is_empty() {
            "Oyun Sunucusu".to_string()
        } else {
            self.edit_name.trim().to_string()
        };
        let clean_code = norm_code;
        let clean_admin = self.edit_admin_key.trim().to_string();

        if self.dropdown_selected_idx == 0 {
            // Ayni koda sahip bir sunucu zaten varsa ustune yaz, duplicate olusturma
            if let Some(existing_idx) = self
                .config
                .servers
                .iter()
                .position(|s| s.code.eq_ignore_ascii_case(&clean_code))
            {
                if let Some(srv) = self.config.servers.get_mut(existing_idx) {
                    srv.name = clean_name;
                    srv.admin_key = clean_admin;
                }
                self.config.active_server_idx = existing_idx;
                self.dropdown_selected_idx = existing_idx + 1;
            } else {
                self.config.servers.push(ServerConfig {
                    name: clean_name,
                    code: clean_code,
                    admin_key: clean_admin,
                });
                let new_idx = self.config.servers.len() - 1;
                self.config.active_server_idx = new_idx;
                self.dropdown_selected_idx = new_idx + 1;
            }
        } else {
            let srv_idx = self.dropdown_selected_idx - 1;
            if let Some(srv) = self.config.servers.get_mut(srv_idx) {
                srv.name = clean_name;
                srv.code = clean_code;
                srv.admin_key = clean_admin;
                self.config.active_server_idx = srv_idx;
            }
        }

        self.sync_and_save_servers();
    }

    fn sync_and_save_servers(&mut self) {
        save_config(&self.config);

        let mut core_cfg = self.core.config.lock().unwrap();
        core_cfg.saved_servers = self
            .config
            .servers
            .iter()
            .map(|s| SavedServer {
                name: s.name.clone(),
                code: normalize_server_key(&s.code),
                admin_key: s.admin_key.clone(),
            })
            .collect();
        core_cfg.active_server_idx = self.config.active_server_idx;
        if let Some(cur) = self.config.servers.get(self.config.active_server_idx) {
            core_cfg.server_name = cur.name.clone();
            core_cfg.server_code = normalize_server_key(&cur.code);
            core_cfg.admin_key = cur.admin_key.clone();
        } else {
            core_cfg.server_code.clear();
            core_cfg.admin_key.clear();
        }
        core_cfg.theme = self.config.theme.clone();
        let _ = core_cfg.save();
    }

    // Aktif input metnine erisim yardimcisi
    #[allow(dead_code)]
    fn get_active_input_text(&self) -> Option<&str> {
        match self.active_input {
            ActiveInput::ServerName => Some(&self.edit_name),
            ActiveInput::ServerCode => Some(&self.edit_code),
            ActiveInput::AdminKey => Some(&self.edit_admin_key),
            ActiveInput::None => None,
        }
    }

    fn clear_active_input(&mut self) {
        match self.active_input {
            ActiveInput::ServerName => self.edit_name.clear(),
            ActiveInput::ServerCode => self.edit_code.clear(),
            ActiveInput::AdminKey => self.edit_admin_key.clear(),
            ActiveInput::None => {}
        }
        self.input_cursor = 0;
        self.input_sel_anchor = 0;
        self.input_select_all = false;
    }

    // --- KLAVYE OLAYLARI ---
    pub fn on_char(&mut self, ch: char, #[allow(unused_variables)] hwnd: isize) {
        if self.page != ActivePage::Config || self.active_input == ActiveInput::None {
            return;
        }

        let total_chars = self.get_active_char_count();
        let (sel_min, sel_max) = if self.input_select_all {
            (0, total_chars)
        } else {
            let c = self.input_cursor.min(total_chars);
            let a = self.input_sel_anchor.min(total_chars);
            (c.min(a), c.max(a))
        };

        // Backspace (\x08) veya Ctrl+Backspace (\x7f)
        if ch == '\x08' || ch == '\x7f' {
            if ch == '\x7f' {
                if sel_min < sel_max {
                    self.delete_active_char_range(sel_min, sel_max);
                    self.input_cursor = sel_min;
                    self.input_sel_anchor = sel_min;
                    self.input_select_all = false;
                } else {
                    self.clear_active_input();
                }
                self.reformat_server_code();
            } else if sel_min < sel_max {
                self.delete_active_char_range(sel_min, sel_max);
                self.input_cursor = sel_min;
                self.input_sel_anchor = sel_min;
                self.input_select_all = false;
                self.reformat_server_code();
            } else if self.input_cursor > 0 {
                if self.active_input == ActiveInput::ServerCode {
                    let chars: Vec<char> = self.edit_code.chars().collect();
                    if self.input_cursor <= chars.len() && self.input_cursor > 0 && chars[self.input_cursor - 1] == '-' && self.input_cursor >= 2 {
                        self.delete_active_char_range(self.input_cursor - 2, self.input_cursor);
                        self.input_cursor -= 2;
                        self.input_sel_anchor = self.input_cursor;
                    } else {
                        self.delete_active_char_range(self.input_cursor - 1, self.input_cursor);
                        self.input_cursor -= 1;
                        self.input_sel_anchor = self.input_cursor;
                    }
                    self.reformat_server_code();
                } else {
                    self.delete_active_char_range(self.input_cursor - 1, self.input_cursor);
                    self.input_cursor -= 1;
                    self.input_sel_anchor = self.input_cursor;
                }
                self.input_select_all = false;
            }
            self.cursor_blink = 0.5;
            return;
        }

        // Enter
        if ch == '\r' || ch == '\n' {
            self.save_current_edit();
            self.switch_to_page(ActivePage::Main);
            self.active_input = ActiveInput::None;
            self.input_select_all = false;
            self.input_is_selecting = false;
            return;
        }

        // Karakter Ekleme
        if !ch.is_control() {
            if self.active_input == ActiveInput::ServerCode && !ch.is_ascii_alphanumeric() {
                return;
            }
            if sel_min < sel_max {
                self.delete_active_char_range(sel_min, sel_max);
                self.input_cursor = sel_min;
                self.input_sel_anchor = sel_min;
                self.input_select_all = false;
            }
            self.insert_active_char(self.input_cursor, ch);
            self.input_cursor += 1;
            self.input_sel_anchor = self.input_cursor;
            self.reformat_server_code();
            self.cursor_blink = 0.5;
        }
    }

    pub fn on_key_down(&mut self, key_code: u32, #[allow(unused_variables)] hwnd: isize) {
        if self.page != ActivePage::Config || self.active_input == ActiveInput::None {
            return;
        }

        #[cfg(windows)]
        let shift_down = unsafe { (GetAsyncKeyState(0x10) as u16 & 0x8000) != 0 };
        #[cfg(not(windows))]
        let shift_down = false;

        let total_chars = self.get_active_char_count();
        let (sel_min, sel_max) = if self.input_select_all {
            (0, total_chars)
        } else {
            let c = self.input_cursor.min(total_chars);
            let a = self.input_sel_anchor.min(total_chars);
            (c.min(a), c.max(a))
        };

        // VK_LEFT (0x25)
        if key_code == 0x25 {
            if shift_down {
                self.input_cursor = self.input_cursor.saturating_sub(1);
                self.input_select_all = false;
            } else if sel_min < sel_max {
                self.input_cursor = sel_min;
                self.input_sel_anchor = sel_min;
                self.input_select_all = false;
            } else {
                self.input_cursor = self.input_cursor.saturating_sub(1);
                self.input_sel_anchor = self.input_cursor;
                self.input_select_all = false;
            }
            self.cursor_blink = 0.5;
            return;
        }

        // VK_RIGHT (0x27)
        if key_code == 0x27 {
            if shift_down {
                self.input_cursor = (self.input_cursor + 1).min(total_chars);
                self.input_select_all = false;
            } else if sel_min < sel_max {
                self.input_cursor = sel_max;
                self.input_sel_anchor = sel_max;
                self.input_select_all = false;
            } else {
                self.input_cursor = (self.input_cursor + 1).min(total_chars);
                self.input_sel_anchor = self.input_cursor;
                self.input_select_all = false;
            }
            self.cursor_blink = 0.5;
            return;
        }

        // VK_HOME (0x24)
        if key_code == 0x24 {
            self.input_cursor = 0;
            if !shift_down {
                self.input_sel_anchor = 0;
            }
            self.input_select_all = false;
            self.cursor_blink = 0.5;
            return;
        }

        // VK_END (0x23)
        if key_code == 0x23 {
            self.input_cursor = total_chars;
            if !shift_down {
                self.input_sel_anchor = total_chars;
            }
            self.input_select_all = false;
            self.cursor_blink = 0.5;
            return;
        }

        // VK_DELETE (0x2E)
        if key_code == 0x2E {
            if sel_min < sel_max {
                self.delete_active_char_range(sel_min, sel_max);
                self.input_cursor = sel_min;
                self.input_sel_anchor = sel_min;
                self.input_select_all = false;
            } else if self.input_cursor < total_chars {
                if self.active_input == ActiveInput::ServerCode {
                    let chars: Vec<char> = self.edit_code.chars().collect();
                    if self.input_cursor < chars.len() && chars[self.input_cursor] == '-' {
                        let end = (self.input_cursor + 2).min(total_chars);
                        self.delete_active_char_range(self.input_cursor, end);
                    } else {
                        self.delete_active_char_range(self.input_cursor, self.input_cursor + 1);
                    }
                } else {
                    self.delete_active_char_range(self.input_cursor, self.input_cursor + 1);
                }
                self.input_sel_anchor = self.input_cursor;
            }
            self.reformat_server_code();
            self.cursor_blink = 0.5;
            return;
        }

        #[cfg(windows)]
        unsafe {
            let ctrl_down = (GetAsyncKeyState(0x11) as u16 & 0x8000) != 0;
            if ctrl_down {
                // 'A' (0x41)
                if key_code == 0x41 {
                    self.input_sel_anchor = 0;
                    self.input_cursor = total_chars;
                    self.input_select_all = false;
                }
                // 'C' (0x43)
                else if key_code == 0x43 {
                    if sel_min < sel_max {
                        if let Some(txt) = self.get_active_input_slice(sel_min, sel_max) {
                            copy_to_clipboard(hwnd as _, &txt);
                        }
                    } else if let Some(txt) = self.get_active_input_text() {
                        if !txt.is_empty() {
                            copy_to_clipboard(hwnd as _, txt);
                        }
                    }
                }
                // 'X' (0x58)
                else if key_code == 0x58 {
                    if sel_min < sel_max {
                        if let Some(txt) = self.get_active_input_slice(sel_min, sel_max) {
                            copy_to_clipboard(hwnd as _, &txt);
                        }
                        self.delete_active_char_range(sel_min, sel_max);
                        self.input_cursor = sel_min;
                        self.input_sel_anchor = sel_min;
                        self.input_select_all = false;
                    } else if let Some(txt) = self.get_active_input_text() {
                        if !txt.is_empty() {
                            copy_to_clipboard(hwnd as _, txt);
                        }
                        self.clear_active_input();
                    }
                    self.reformat_server_code();
                }
                // 'V' (0x56)
                else if key_code == 0x56 {
                    if let Some(text) = paste_from_clipboard(hwnd as _) {
                        if self.active_input == ActiveInput::ServerCode {
                            let clean_alnum: String = text
                                .chars()
                                .filter(|c| c.is_ascii_alphanumeric())
                                .map(|c| c.to_ascii_uppercase())
                                .take(12)
                                .collect();
                            if clean_alnum.len() == 12 {
                                self.edit_code = format!(
                                    "{}-{}-{}",
                                    &clean_alnum[0..4],
                                    &clean_alnum[4..8],
                                    &clean_alnum[8..12]
                                );
                                self.input_cursor = 14;
                                self.input_sel_anchor = 14;
                                self.input_select_all = false;
                            } else if !clean_alnum.is_empty() {
                                if sel_min < sel_max {
                                    self.delete_active_char_range(sel_min, sel_max);
                                    self.input_cursor = sel_min;
                                    self.input_sel_anchor = sel_min;
                                    self.input_select_all = false;
                                }
                                let mut count = 0;
                                for c in clean_alnum.chars() {
                                    self.insert_active_char(self.input_cursor + count, c);
                                    count += 1;
                                }
                                self.input_cursor += count;
                                self.input_sel_anchor = self.input_cursor;
                                self.reformat_server_code();
                            }
                        } else {
                            if sel_min < sel_max {
                                self.delete_active_char_range(sel_min, sel_max);
                                self.input_cursor = sel_min;
                                self.input_sel_anchor = sel_min;
                                self.input_select_all = false;
                            }
                            let mut count = 0;
                            for c in text.chars() {
                                if !c.is_control() {
                                    self.insert_active_char(self.input_cursor + count, c);
                                    count += 1;
                                }
                            }
                            self.input_cursor += count;
                            self.input_sel_anchor = self.input_cursor;
                        }
                    }
                }
            }
        }
    }
}

// Windows Panosundan Metin Okuma
#[cfg(windows)]
pub fn paste_from_clipboard(hwnd: HWND) -> Option<String> {
    unsafe {
        if OpenClipboard(hwnd) != 0 {
            let h_mem = GetClipboardData(13); // CF_UNICODETEXT = 13
            if !h_mem.is_null() {
                let ptr = GlobalLock(h_mem) as *const u16;
                if !ptr.is_null() {
                    let mut len = 0;
                    while *ptr.add(len) != 0 {
                        len += 1;
                    }
                    let slice = std::slice::from_raw_parts(ptr, len);
                    let result = String::from_utf16_lossy(slice);
                    GlobalUnlock(h_mem);
                    CloseClipboard();
                    return Some(result);
                }
            }
            CloseClipboard();
        }
    }
    None
}

// Windows Panosuna Metin Kopyalama
#[cfg(windows)]
pub fn copy_to_clipboard(hwnd: HWND, text: &str) {
    unsafe {
        if OpenClipboard(hwnd) != 0 {
            EmptyClipboard();
            let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
            let size = wide.len() * std::mem::size_of::<u16>();
            let h_mem = GlobalAlloc(GMEM_MOVEABLE, size);
            if !h_mem.is_null() {
                let ptr = GlobalLock(h_mem) as *mut u16;
                if !ptr.is_null() {
                    std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr, wide.len());
                    GlobalUnlock(h_mem);
                    SetClipboardData(13, h_mem as _);
                }
            }
            CloseClipboard();
        }
    }
}

// DWM Baslik Cubugu Koyu / Acik Tema Rengi
#[cfg(windows)]
pub unsafe fn apply_dwm_theme(hwnd: HWND, is_dark: bool) {
    if hwnd.is_null() {
        return;
    }
    let dark_val: i32 = if is_dark { 1 } else { 0 };
    DwmSetWindowAttribute(
        hwnd,
        20, // DWMWA_USE_IMMERSIVE_DARK_MODE
        &dark_val as *const _ as _,
        std::mem::size_of::<i32>() as u32,
    );

    let caption_color: u32 = if is_dark { 0x000E0C0C } else { 0x00F2EFEF }; // BGR
    DwmSetWindowAttribute(
        hwnd,
        35, // DWMWA_CAPTION_COLOR
        &caption_color as *const _ as _,
        std::mem::size_of::<u32>() as u32,
    );

    let text_color: u32 = if is_dark { 0x00F6F4F4 } else { 0x00171414 }; // BGR
    DwmSetWindowAttribute(
        hwnd,
        36, // DWMWA_TEXT_COLOR
        &text_color as *const _ as _,
        std::mem::size_of::<u32>() as u32,
    );
}

// Sistem Tepsisi Bildirimi
#[cfg(windows)]
pub unsafe fn setup_tray_icon(hwnd: HWND, icon: HICON) {
    let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = 1;
    nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    nid.uCallbackMessage = WM_TRAY_ICON;
    nid.hIcon = icon;

    let tip = "maziTunnel\0".encode_utf16().collect::<Vec<u16>>();
    let copy_len = tip.len().min(nid.szTip.len());
    nid.szTip[..copy_len].copy_from_slice(&tip[..copy_len]);

    Shell_NotifyIconW(NIM_ADD, &nid);
}

#[cfg(windows)]
pub unsafe fn remove_tray_icon(hwnd: HWND) {
    let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = 1;
    Shell_NotifyIconW(NIM_DELETE, &nid);
}

#[cfg(windows)]
pub unsafe fn send_tray_notification(hwnd: HWND, title: &str, body: &str) {
    if hwnd.is_null() {
        return;
    }
    const NIM_MODIFY: u32 = 0x00000001;
    const NIF_INFO: u32 = 0x00000010;

    let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = 1;
    nid.uFlags = NIF_INFO;
    nid.dwInfoFlags = 0;

    let title_utf16: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    let copy_title_len = title_utf16.len().min(nid.szInfoTitle.len());
    nid.szInfoTitle[..copy_title_len].copy_from_slice(&title_utf16[..copy_title_len]);

    let body_utf16: Vec<u16> = body.encode_utf16().chain(std::iter::once(0)).collect();
    let copy_body_len = body_utf16.len().min(nid.szInfo.len());
    nid.szInfo[..copy_body_len].copy_from_slice(&body_utf16[..copy_body_len]);

    Shell_NotifyIconW(NIM_MODIFY, &nid);
}

#[cfg(not(windows))]
pub fn send_tray_notification(_hwnd: isize, _title: &str, _body: &str) {}

// Cift Tamponlu GDI Cizim Yardimcisi
#[cfg(windows)]
unsafe fn blit_canvas_to_hdc(hdc: HDC, pixels: &[u32]) {
    blit_canvas_to_hdc_sized(hdc, pixels, WINDOW_WIDTH, WINDOW_HEIGHT);
}

#[cfg(windows)]
unsafe fn blit_canvas_to_hdc_sized(hdc: HDC, pixels: &[u32], width: u32, height: u32) {
    if hdc.is_null() {
        return;
    }
    let mut bmi: BITMAPINFO = std::mem::zeroed();
    bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    bmi.bmiHeader.biWidth = width as i32;
    bmi.bmiHeader.biHeight = -(height as i32);
    bmi.bmiHeader.biPlanes = 1;
    bmi.bmiHeader.biBitCount = 32;
    bmi.bmiHeader.biCompression = BI_RGB;

    SetDIBitsToDevice(
        hdc,
        0,
        0,
        width,
        height,
        0,
        0,
        0,
        height,
        pixels.as_ptr() as *const _,
        &bmi,
        DIB_RGB_COLORS,
    );
}

// Sistem Tepsisi Ozel Monokrom Popup Penceresi
pub struct TrayPopupState {
    pub canvas: Canvas,
    pub main_hwnd: HWND,
    pub subpopup_hwnd: HWND,
    pub hovered_item: Option<usize>,
    pub hover_t: [f32; 5],
    pub open_t: f32,
    pub target_x: i32,
    pub target_y: i32,
    pub start_y: i32,
    pub active_h: i32,
    pub last_tick: Instant,
}

pub const TRAY_SUBPOPUP_WIDTH: u32 = 210;
pub const TRAY_SUBPOPUP_MAX_HEIGHT: u32 = 240;

pub struct TraySubPopupState {
    pub canvas: Canvas,
    pub main_hwnd: HWND,
    pub tray_popup_hwnd: HWND,
    pub hovered_item: Option<usize>,
    pub hover_t: Vec<f32>,
    pub open_t: f32,
    pub start_x: i32,
    pub target_x: i32,
    pub cur_x: i32,
    pub target_y: i32,
    pub active_h: i32,
    pub last_tick: Instant,
    pub is_visible: bool,
}

struct TrayItemData {
    cmd_idx: usize,
    label: String,
    icon: &'static str,
    badge: String,
    is_danger: bool,
    y: f32,
}

fn get_tray_popup_items(main: &GuiAppState) -> (Vec<TrayItemData>, f32) {
    let is_connected = main.state == ConnectionState::Connected;
    let is_connecting = main.state == ConnectionState::Connecting;
    let pal = main.theme;
    let theme_label = if pal.is_dark {
        "Görünüm: Açık Tema"
    } else {
        "Görünüm: Koyu Tema"
    };

    if is_connected {
        let items = vec![
            TrayItemData {
                cmd_idx: 0,
                label: "maziTunnel'ı Göster".to_string(),
                icon: "window",
                badge: "Aç".to_string(),
                is_danger: false,
                y: 70.0,
            },
            TrayItemData {
                cmd_idx: 1,
                label: "Bağlantıyı Kes".to_string(),
                icon: "power",
                badge: "Aktif".to_string(),
                is_danger: false,
                y: 106.0,
            },
            TrayItemData {
                cmd_idx: 2,
                label: "Adresi Kopyala".to_string(),
                icon: "copy",
                badge: main.assigned_address.clone(),
                is_danger: false,
                y: 142.0,
            },
            TrayItemData {
                cmd_idx: 3,
                label: theme_label.to_string(),
                icon: "theme",
                badge: String::new(),
                is_danger: false,
                y: 178.0,
            },
            TrayItemData {
                cmd_idx: 4,
                label: "Çıkış Yap".to_string(),
                icon: "quit",
                badge: String::new(),
                is_danger: true,
                y: 220.0,
            },
        ];
        (items, 217.0)
    } else {
        let conn_label = if is_connecting {
            "Bağlanıyor..."
        } else {
            "Hızlı Bağlan"
        };
        let badge_text = if !is_connecting { ">".to_string() } else { String::new() };
        let items = vec![
            TrayItemData {
                cmd_idx: 0,
                label: "maziTunnel'ı Göster".to_string(),
                icon: "window",
                badge: "Aç".to_string(),
                is_danger: false,
                y: 70.0,
            },
            TrayItemData {
                cmd_idx: 1,
                label: conn_label.to_string(),
                icon: "power",
                badge: badge_text,
                is_danger: false,
                y: 106.0,
            },
            TrayItemData {
                cmd_idx: 3,
                label: theme_label.to_string(),
                icon: "theme",
                badge: String::new(),
                is_danger: false,
                y: 142.0,
            },
            TrayItemData {
                cmd_idx: 4,
                label: "Çıkış Yap".to_string(),
                icon: "quit",
                badge: String::new(),
                is_danger: true,
                y: 184.0,
            },
        ];
        (items, 181.0)
    }
}

#[allow(dead_code)]
fn render_tray_popup(popup: &mut TrayPopupState, main: &GuiAppState) {
    let pal = main.theme;
    let w = TRAY_POPUP_WIDTH as f32;

    let bg_col = if pal.is_dark {
        rgba(16, 16, 19, 255)
    } else {
        rgba(244, 244, 246, 255)
    };
    popup.canvas.clear(bg_col);
    popup.canvas.stroke_rounded_rect(
        0.5,
        0.5,
        w - 1.0,
        popup.active_h as f32 - 1.0,
        8.0,
        1.0,
        pal.card_border,
    );

    // Ust Durum Karti (Header Box - X: 10.0, Y: 10.0, W: 236.0, H: 54.0, radius: 10.0)
    let hdr_bg = if pal.is_dark {
        rgba(24, 24, 29, 255)
    } else {
        rgba(234, 234, 239, 255)
    };
    let hdr_border = if pal.is_dark {
        rgba(38, 38, 46, 255)
    } else {
        rgba(212, 212, 220, 255)
    };
    popup.canvas.fill_rounded_rect(10.0, 10.0, 236.0, 54.0, 10.0, hdr_bg);
    popup.canvas.stroke_rounded_rect(10.0, 10.0, 236.0, 54.0, 10.0, 1.0, hdr_border);

    // Ust satir
    let dot_col = match main.state {
        ConnectionState::Connected => rgba(52, 211, 153, 255),
        ConnectionState::Connecting => rgba(251, 191, 36, 255),
        _ => {
            if pal.is_dark {
                rgba(113, 113, 122, 255)
            } else {
                rgba(161, 161, 170, 255)
            }
        }
    };
    popup.canvas.fill_circle(24.0, 26.0, 4.0, dot_col);

    popup
        .canvas
        .draw_text_weight(FontWeight::Bold, 12.5, 34.0, 17.5, "maziTunnel", pal.text_primary);

    let (vw, _) = popup.canvas.measure_text_semibold(10.5, "v2.0.1");
    popup.canvas.draw_text_weight(
        FontWeight::Semibold,
        10.5,
        234.0 - vw,
        18.5,
        "v2.0.1",
        pal.text_muted,
    );

    // Alt satir (Y: 38.0)
    let srv_name = main
        .config
        .servers
        .get(main.config.active_server_idx)
        .map(|s| s.name.as_str())
        .unwrap_or("Oyun Sunucusu");
    let sub_str = match main.state {
        ConnectionState::Connected => format!("Bağlı • {}", srv_name),
        ConnectionState::Connecting => "Sunucuya bağlanılıyor...".to_string(),
        _ => format!("Bağlı Değil • {}", srv_name),
    };
    popup
        .canvas
        .draw_text(false, 11.5, 20.0, 38.0, &sub_str, pal.text_secondary);

    // Menu Ogeleri ve Ayirici
    let (items, sep_y) = get_tray_popup_items(main);

    // Ayirici cizgi (1px)
    popup
        .canvas
        .draw_line(18.0, sep_y, w - 18.0, sep_y, 1.0, pal.card_border);

    for item in items {
        let row_idx = item.cmd_idx;
        let iy = item.y;
        let ht = ease_out_cubic(popup.hover_t[row_idx].clamp(0.0, 1.0));

        if ht > 0.005 {
            let (fill, border) = if item.is_danger {
                (
                    rgba(239, 68, 68, (38.0 * ht) as u8),
                    rgba(239, 68, 68, (75.0 * ht) as u8),
                )
            } else if pal.is_dark {
                (
                    rgba(255, 255, 255, (20.0 * ht) as u8),
                    rgba(255, 255, 255, (36.0 * ht) as u8),
                )
            } else {
                (
                    rgba(20, 20, 23, (16.0 * ht) as u8),
                    rgba(20, 20, 23, (30.0 * ht) as u8),
                )
            };
            popup.canvas.fill_rounded_rect(10.0, iy + 1.0, 236.0, 34.0, 9.0, fill);
            popup
                .canvas
                .stroke_rounded_rect(10.0, iy + 1.0, 236.0, 34.0, 9.0, 1.0, border);
        }

        let slide_x = 2.5 * ht;
        let fg = if item.is_danger {
            if pal.is_dark {
                rgba(248, 113, 113, 255)
            } else {
                rgba(220, 38, 38, 255)
            }
        } else {
            lerp_color(pal.text_secondary, pal.text_primary, (ht * 1.4).min(1.0))
        };

        let cx = 28.0 + slide_x;
        let cy = iy + 18.0;

        match item.icon {
            "window" => {
                popup.canvas.stroke_rounded_rect(
                    cx - 6.0,
                    cy - 5.0,
                    12.0,
                    10.0,
                    2.2,
                    1.45,
                    fg,
                );
                popup.canvas.draw_line(cx - 5.5, cy - 1.8, cx + 5.5, cy - 1.8, 1.35, fg);
            }
            "power" => {
                popup.canvas.draw_power_icon(cx, cy, 5.5, fg, 1.55);
            }
            "copy" => {
                popup.canvas.stroke_rounded_rect(
                    cx - 5.0,
                    cy - 5.5,
                    7.2,
                    7.2,
                    1.6,
                    1.35,
                    fg,
                );
                popup
                    .canvas
                    .fill_rounded_rect(cx - 2.0, cy - 2.5, 7.2, 7.2, 1.6, bg_col);
                popup.canvas.stroke_rounded_rect(
                    cx - 2.0,
                    cy - 2.5,
                    7.2,
                    7.2,
                    1.6,
                    1.35,
                    fg,
                );
            }
            "theme" => {
                popup.canvas.stroke_circle(cx, cy, 4.8, 1.45, fg);
                popup.canvas.fill_circle(cx, cy, 1.8, fg);
            }
            "quit" => {
                popup.canvas.draw_icon_close(cx, cy, 8.0, 1.6, fg);
            }
            _ => {}
        }

        popup.canvas.draw_text_weight(
            FontWeight::Semibold,
            12.5,
            44.0 + slide_x,
            iy + 9.5,
            &item.label,
            fg,
        );

        if !item.badge.is_empty() {
            let (bw, _) = popup.canvas.measure_text_semibold(10.5, &item.badge);
            popup.canvas.draw_text_weight(
                FontWeight::Semibold,
                10.5,
                234.0 - bw,
                iy + 10.5,
                &item.badge,
                pal.text_muted,
            );
        }
    }
}

#[allow(dead_code)]
fn render_tray_subpopup(sub: &mut TraySubPopupState, main: &GuiAppState) {
    let pal = main.theme;
    let w = TRAY_SUBPOPUP_WIDTH as f32;
    let h = sub.active_h as f32;

    let bg_col = if pal.is_dark {
        rgba(16, 16, 19, 255)
    } else {
        rgba(244, 244, 246, 255)
    };
    sub.canvas.clear(bg_col);
    sub.canvas.stroke_rounded_rect(
        0.5,
        0.5,
        w - 1.0,
        h - 1.0,
        8.0,
        1.0,
        pal.card_border,
    );

    if main.config.servers.is_empty() {
        let empty_msg = "Kayıtlı Sunucu Yok";
        let (tw, _) = sub.canvas.measure_text_semibold(11.0, empty_msg);
        let tx = ((w - tw) / 2.0).max(10.0);
        sub.canvas.draw_text_weight(
            FontWeight::Regular,
            11.0,
            tx,
            15.0,
            empty_msg,
            pal.text_muted,
        );
        return;
    }

    for (idx, srv) in main.config.servers.iter().enumerate() {
        let iy = 8.0 + idx as f32 * 36.0;
        if iy + 32.0 > h {
            break;
        }

        let ht = ease_out_cubic(sub.hover_t.get(idx).copied().unwrap_or(0.0).clamp(0.0, 1.0));
        if ht > 0.005 {
            let (fill, border) = if pal.is_dark {
                (
                    rgba(255, 255, 255, (20.0 * ht) as u8),
                    rgba(255, 255, 255, (36.0 * ht) as u8),
                )
            } else {
                (
                    rgba(20, 20, 23, (16.0 * ht) as u8),
                    rgba(20, 20, 23, (30.0 * ht) as u8),
                )
            };
            sub.canvas.fill_rounded_rect(8.0, iy, w - 16.0, 32.0, 7.0, fill);
            sub.canvas.stroke_rounded_rect(8.0, iy, w - 16.0, 32.0, 7.0, 1.0, border);
        }

        let is_active = idx == main.config.active_server_idx;
        let cy = iy + 16.0;
        if is_active {
            sub.canvas.fill_circle(20.0, cy, 3.5, rgba(52, 211, 153, 255));
        } else {
            sub.canvas.stroke_circle(20.0, cy, 3.5, 1.2, pal.text_muted);
        }

        let fg = if is_active {
            pal.text_primary
        } else {
            lerp_color(pal.text_secondary, pal.text_primary, (ht * 1.4).min(1.0))
        };

        let slide_x = 2.0 * ht;
        let mut display_name = srv.name.clone();
        if display_name.chars().count() > 18 {
            display_name = display_name.chars().take(16).collect::<String>() + "...";
        }

        sub.canvas.draw_text_weight(
            FontWeight::Semibold,
            11.5,
            32.0 + slide_x,
            iy + 8.5,
            &display_name,
            fg,
        );
    }
}

#[cfg(windows)]
pub unsafe fn show_tray_subpopup(hwnd_sub: HWND, hwnd_main_popup: HWND, hwnd_main: HWND) {
    if hwnd_sub.is_null() || hwnd_main_popup.is_null() || hwnd_main.is_null() {
        return;
    }
    let sub_state_ptr = GetWindowLongPtrW(hwnd_sub, GWLP_USERDATA) as *mut TraySubPopupState;
    let main_popup_ptr = GetWindowLongPtrW(hwnd_main_popup, GWLP_USERDATA) as *mut TrayPopupState;
    let main_state_ptr = GetWindowLongPtrW(hwnd_main, GWLP_USERDATA) as *mut GuiAppState;
    if sub_state_ptr.is_null() || main_popup_ptr.is_null() || main_state_ptr.is_null() {
        return;
    }
    let sub = &mut *sub_state_ptr;
    let main_pop = &*main_popup_ptr;
    let main = &*main_state_ptr;

    let srv_count = main.config.servers.len();
    let active_h = if srv_count == 0 {
        44
    } else {
        (16.0 + srv_count as f32 * 36.0).clamp(44.0, 240.0) as i32
    };

    let screen_w = GetSystemMetrics(SM_CXSCREEN);
    let screen_h = GetSystemMetrics(SM_CYSCREEN);

    let pop_x = main_pop.target_x;
    let pop_y = main_pop.target_y;

    let mut target_y = pop_y + 106 - 6;
    if target_y + active_h > screen_h - 10 {
        target_y = screen_h - active_h - 10;
    }
    if target_y < 10 {
        target_y = 10;
    }

    let sub_w = TRAY_SUBPOPUP_WIDTH as i32;
    let (mut target_x, start_x) = if pop_x - sub_w - 4 >= 10 {
        let tx = pop_x - sub_w - 4;
        (tx, tx + 14)
    } else {
        let tx = pop_x + TRAY_POPUP_WIDTH as i32 + 4;
        (tx, tx - 14)
    };
    if target_x + sub_w > screen_w - 10 {
        target_x = screen_w - sub_w - 10;
    }

    sub.open_t = 0.0;
    sub.start_x = start_x;
    sub.target_x = target_x;
    sub.cur_x = start_x;
    sub.target_y = target_y;
    sub.active_h = active_h;
    sub.hovered_item = None;
    sub.hover_t = vec![0.0; srv_count];
    sub.last_tick = Instant::now();
    sub.is_visible = true;

    render_tray_subpopup(sub, main);
    let hdc = GetDC(hwnd_sub);
    blit_canvas_to_hdc_sized(hdc, &sub.canvas.pixels, TRAY_SUBPOPUP_WIDTH, active_h as u32);
    ReleaseDC(hwnd_sub, hdc);

    apply_dwm_theme(hwnd_sub, main.theme.is_dark);
    SetLayeredWindowAttributes(hwnd_sub, 0, 0, LWA_ALPHA);
    SetWindowPos(
        hwnd_sub,
        0 as HWND,
        start_x,
        target_y,
        sub_w,
        active_h,
        SWP_SHOWWINDOW | SWP_NOACTIVATE,
    );
    SetTimer(hwnd_sub, 1, 5, None);
}

#[cfg(windows)]
pub unsafe fn hide_tray_subpopup(hwnd_sub: HWND) {
    if hwnd_sub.is_null() {
        return;
    }
    let sub_state_ptr = GetWindowLongPtrW(hwnd_sub, GWLP_USERDATA) as *mut TraySubPopupState;
    if !sub_state_ptr.is_null() {
        let sub = &mut *sub_state_ptr;
        if !sub.is_visible {
            return;
        }
        sub.is_visible = false;
    }
    KillTimer(hwnd_sub, 1);
    ShowWindow(hwnd_sub, SW_HIDE);
}

#[cfg(not(windows))]
pub fn show_tray_subpopup(_hwnd_sub: isize, _hwnd_main_popup: isize, _hwnd_main: isize) {}
#[cfg(not(windows))]
pub fn hide_tray_subpopup(_hwnd_sub: isize) {}

// Sistem Tepsisi Alt Popup (Flyout Submenu) WinProc
#[cfg(windows)]
unsafe extern "system" fn tray_subpopup_window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut TraySubPopupState;

    match msg {
        WM_ERASEBKGND => 1,
        WM_ACTIVATE => {
            if (wparam & 0xFFFF) == 0 {
                hide_tray_subpopup(hwnd);
            }
            0
        }
        WM_KILLFOCUS => {
            hide_tray_subpopup(hwnd);
            0
        }
        WM_TIMER => {
            if !state_ptr.is_null() {
                let state = &mut *state_ptr;
                let now = Instant::now();
                let dt = now
                    .duration_since(state.last_tick)
                    .as_secs_f32()
                    .clamp(0.001, 0.05);
                state.last_tick = now;
                let mut needs_redraw = false;

                if state.open_t < 1.0 {
                    state.open_t = (state.open_t + dt / 0.14).min(1.0);
                    let ease = ease_out_cubic(state.open_t);
                    state.cur_x = (state.start_x as f32 + (state.target_x - state.start_x) as f32 * ease).round() as i32;
                    let alpha = (state.open_t.sqrt() * 255.0).round().clamp(0.0, 255.0) as u8;
                    SetLayeredWindowAttributes(hwnd, 0, alpha, LWA_ALPHA);
                    SetWindowPos(
                        hwnd,
                        0 as HWND,
                        state.cur_x,
                        state.target_y,
                        TRAY_SUBPOPUP_WIDTH as i32,
                        state.active_h,
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                    needs_redraw = true;
                }

                for i in 0..state.hover_t.len() {
                    let target = if state.hovered_item == Some(i) { 1.0 } else { 0.0 };
                    if (state.hover_t[i] - target).abs() > 0.001 {
                        if state.hover_t[i] < target {
                            state.hover_t[i] = (state.hover_t[i] + dt / 0.12).min(target);
                        } else {
                            state.hover_t[i] = (state.hover_t[i] - dt / 0.12).max(target);
                        }
                        needs_redraw = true;
                    }
                }

                let mut pt: POINT = std::mem::zeroed();
                GetCursorPos(&mut pt);
                let cur_x_min = state.cur_x.min(state.target_x) - 12;
                let cur_x_max = state.cur_x.max(state.target_x) + TRAY_SUBPOPUP_WIDTH as i32 + 12;
                let cur_y_min = state.target_y - 12;
                let cur_y_max = state.target_y + state.active_h + 12;

                let in_sub = pt.x >= cur_x_min && pt.x <= cur_x_max && pt.y >= cur_y_min && pt.y <= cur_y_max;

                let mut in_main_item = false;
                let main_pop_ptr = GetWindowLongPtrW(state.tray_popup_hwnd, GWLP_USERDATA) as *mut TrayPopupState;
                if !main_pop_ptr.is_null() {
                    let main_pop = &*main_pop_ptr;
                    let pop_x = main_pop.target_x;
                    let pop_y = main_pop.target_y;
                    if pt.x >= pop_x - 12 && pt.x <= pop_x + TRAY_POPUP_WIDTH as i32 + 12
                        && pt.y >= pop_y + 90 && pt.y <= pop_y + 155 {
                        in_main_item = true;
                    }
                }

                if !in_sub && !in_main_item {
                    hide_tray_subpopup(hwnd);
                    return 0;
                }

                if needs_redraw {
                    let main_state_ptr = GetWindowLongPtrW(state.main_hwnd, GWLP_USERDATA) as *mut GuiAppState;
                    if !main_state_ptr.is_null() {
                        let main_state = &*main_state_ptr;
                        render_tray_subpopup(state, main_state);
                        let hdc = GetDC(hwnd);
                        blit_canvas_to_hdc_sized(hdc, &state.canvas.pixels, TRAY_SUBPOPUP_WIDTH, state.active_h as u32);
                        ReleaseDC(hwnd, hdc);
                    }
                }
            }
            0
        }
        WM_MOUSEMOVE => {
            if !state_ptr.is_null() {
                let state = &mut *state_ptr;
                let main_state_ptr = GetWindowLongPtrW(state.main_hwnd, GWLP_USERDATA) as *mut GuiAppState;
                if !main_state_ptr.is_null() {
                    let main_state = &*main_state_ptr;
                    let srv_count = main_state.config.servers.len();
                    let x = (lparam & 0xFFFF) as i16 as f32;
                    let y = ((lparam >> 16) & 0xFFFF) as i16 as f32;

                    let mut new_hover = None;
                    if x >= 8.0 && x <= (TRAY_SUBPOPUP_WIDTH as f32 - 8.0) {
                        for idx in 0..srv_count {
                            let iy = 8.0 + idx as f32 * 36.0;
                            if y >= iy && y < iy + 34.0 {
                                new_hover = Some(idx);
                                break;
                            }
                        }
                    }

                    if state.hovered_item != new_hover {
                        state.hovered_item = new_hover;
                    }
                }
            }
            0
        }
        WM_LBUTTONDOWN => {
            if !state_ptr.is_null() {
                let state = &mut *state_ptr;
                let main_state_ptr = GetWindowLongPtrW(state.main_hwnd, GWLP_USERDATA) as *mut GuiAppState;
                if !main_state_ptr.is_null() {
                    let main_state = &*main_state_ptr;
                    let srv_count = main_state.config.servers.len();
                    let x = (lparam & 0xFFFF) as i16 as f32;
                    let y = ((lparam >> 16) & 0xFFFF) as i16 as f32;

                    let mut clicked_idx = None;
                    if x >= 8.0 && x <= (TRAY_SUBPOPUP_WIDTH as f32 - 8.0) {
                        for idx in 0..srv_count {
                            let iy = 8.0 + idx as f32 * 36.0;
                            if y >= iy && y < iy + 34.0 {
                                clicked_idx = Some(idx);
                                break;
                            }
                        }
                    }

                    if let Some(idx) = clicked_idx {
                        hide_tray_subpopup(hwnd);
                        KillTimer(state.tray_popup_hwnd, 1);
                        ShowWindow(state.tray_popup_hwnd, SW_HIDE);
                        PostMessageW(state.main_hwnd, WM_TRAY_COMMAND, (100 + idx) as WPARAM, 0);
                    }
                }
            }
            0
        }
        WM_PAINT => {
            let mut ps: PAINTSTRUCT = std::mem::zeroed();
            let hdc = BeginPaint(hwnd, &mut ps);
            if !state_ptr.is_null() {
                let state = &mut *state_ptr;
                let main_state_ptr = GetWindowLongPtrW(state.main_hwnd, GWLP_USERDATA) as *mut GuiAppState;
                if !main_state_ptr.is_null() {
                    let main_state = &*main_state_ptr;
                    render_tray_subpopup(state, main_state);
                    blit_canvas_to_hdc_sized(
                        hdc,
                        &state.canvas.pixels,
                        TRAY_SUBPOPUP_WIDTH,
                        state.active_h as u32,
                    );
                }
            }
            EndPaint(hwnd, &ps);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

// Sistem Tepsisi Popup WinProc
#[cfg(windows)]
unsafe extern "system" fn tray_popup_window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut TrayPopupState;

    match msg {
        WM_ERASEBKGND => 1,
        WM_ACTIVATE => {
            if (wparam & 0xFFFF) == 0 {
                if !state_ptr.is_null() {
                    let state = &*state_ptr;
                    hide_tray_subpopup(state.subpopup_hwnd);
                }
                KillTimer(hwnd, 1);
                ShowWindow(hwnd, SW_HIDE);
            }
            0
        }
        WM_KILLFOCUS => {
            if !state_ptr.is_null() {
                let state = &*state_ptr;
                hide_tray_subpopup(state.subpopup_hwnd);
            }
            KillTimer(hwnd, 1);
            ShowWindow(hwnd, SW_HIDE);
            0
        }
        WM_TIMER => {
            if !state_ptr.is_null() {
                let state = &mut *state_ptr;
                let now = Instant::now();
                let dt = now
                    .duration_since(state.last_tick)
                    .as_secs_f32()
                    .clamp(0.001, 0.05);
                state.last_tick = now;

                let mut cur_pt: POINT = std::mem::zeroed();
                GetCursorPos(&mut cur_pt);
                let cur_y_now = (state.start_y as f32
                    + (state.target_y - state.start_y) as f32
                        * (1.0 - (1.0 - state.open_t).powi(3)))
                .round() as i32;
                let in_bounds = cur_pt.x >= state.target_x
                    && cur_pt.x < state.target_x + TRAY_POPUP_WIDTH as i32
                    && cur_pt.y >= cur_y_now
                    && cur_pt.y < cur_y_now + state.active_h;
                if !in_bounds && state.hovered_item.is_some() {
                    state.hovered_item = None;
                }

                let mut needs_redraw = false;

                if state.open_t < 1.0 {
                    state.open_t = (state.open_t + dt / 0.17).min(1.0);
                    let ease_pos = 1.0 - (1.0 - state.open_t).powi(3);
                    let cur_y = (state.start_y as f32
                        + (state.target_y - state.start_y) as f32 * ease_pos)
                        .round() as i32;
                    let alpha = (state.open_t.sqrt() * 255.0).round().clamp(0.0, 255.0) as u8;
                    SetLayeredWindowAttributes(hwnd, 0, alpha, LWA_ALPHA);
                    SetWindowPos(
                        hwnd,
                        0 as HWND,
                        state.target_x,
                        cur_y,
                        TRAY_POPUP_WIDTH as i32,
                        state.active_h,
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                    needs_redraw = true;
                }

                for i in 0..5 {
                    let target_h = if state.hovered_item == Some(i) {
                        1.0
                    } else {
                        0.0
                    };
                    if (state.hover_t[i] - target_h).abs() > 0.001 {
                        if state.hover_t[i] < target_h {
                            state.hover_t[i] = (state.hover_t[i] + dt / 0.14).min(target_h);
                        } else {
                            state.hover_t[i] = (state.hover_t[i] - dt / 0.14).max(target_h);
                        }
                        needs_redraw = true;
                    }
                }

                if needs_redraw {
                    let main_state_ptr =
                        GetWindowLongPtrW(state.main_hwnd, GWLP_USERDATA) as *mut GuiAppState;
                    if !main_state_ptr.is_null() {
                        let main_state = &*main_state_ptr;
                        render_tray_popup(state, main_state);
                        let hdc = GetDC(hwnd);
                        blit_canvas_to_hdc_sized(
                            hdc,
                            &state.canvas.pixels,
                            TRAY_POPUP_WIDTH,
                            state.active_h as u32,
                        );
                        ReleaseDC(hwnd, hdc);
                    }
                }
            }
            0
        }
        WM_MOUSEMOVE => {
            if !state_ptr.is_null() {
                let state = &mut *state_ptr;
                let main_state_ptr =
                    GetWindowLongPtrW(state.main_hwnd, GWLP_USERDATA) as *mut GuiAppState;
                if !main_state_ptr.is_null() {
                    let main_state = &*main_state_ptr;
                    let (items, _) = get_tray_popup_items(main_state);
                    let x = (lparam & 0xFFFF) as i16 as f32;
                    let y = ((lparam >> 16) & 0xFFFF) as i16 as f32;

                    let mut new_hover = None;
                    if x >= 10.0 && x <= 246.0 {
                        for it in &items {
                            if y >= it.y && y < it.y + 36.0 {
                                new_hover = Some(it.cmd_idx);
                                break;
                            }
                        }
                    }

                    if state.hovered_item != new_hover {
                        state.hovered_item = new_hover;
                        if new_hover == Some(1) && main_state.state == ConnectionState::Disconnected {
                            show_tray_subpopup(state.subpopup_hwnd, hwnd, state.main_hwnd);
                        } else if new_hover.is_some() && new_hover != Some(1) {
                            hide_tray_subpopup(state.subpopup_hwnd);
                        }
                    }
                }
            }
            0
        }
        WM_LBUTTONDOWN => {
            if !state_ptr.is_null() {
                let state = &mut *state_ptr;
                let main_state_ptr =
                    GetWindowLongPtrW(state.main_hwnd, GWLP_USERDATA) as *mut GuiAppState;
                if !main_state_ptr.is_null() {
                    let main_state = &*main_state_ptr;
                    let (items, _) = get_tray_popup_items(main_state);
                    let x = (lparam & 0xFFFF) as i16 as f32;
                    let y = ((lparam >> 16) & 0xFFFF) as i16 as f32;

                    let mut clicked_cmd = None;
                    if x >= 10.0 && x <= 246.0 {
                        for it in &items {
                            if y >= it.y && y < it.y + 36.0 {
                                clicked_cmd = Some(it.cmd_idx);
                                break;
                            }
                        }
                    }

                    if let Some(cmd) = clicked_cmd {
                        hide_tray_subpopup(state.subpopup_hwnd);
                        KillTimer(hwnd, 1);
                        ShowWindow(hwnd, SW_HIDE);
                        PostMessageW(state.main_hwnd, WM_TRAY_COMMAND, cmd as WPARAM, 0);
                    }
                }
            }
            0
        }
        WM_PAINT => {
            let mut ps: PAINTSTRUCT = std::mem::zeroed();
            let hdc = BeginPaint(hwnd, &mut ps);
            if !state_ptr.is_null() {
                let state = &mut *state_ptr;
                let main_state_ptr =
                    GetWindowLongPtrW(state.main_hwnd, GWLP_USERDATA) as *mut GuiAppState;
                if !main_state_ptr.is_null() {
                    let main_state = &*main_state_ptr;
                    render_tray_popup(state, main_state);
                    blit_canvas_to_hdc_sized(
                        hdc,
                        &state.canvas.pixels,
                        TRAY_POPUP_WIDTH,
                        state.active_h as u32,
                    );
                }
            }
            EndPaint(hwnd, &ps);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

// Windows WinProc ve Ana Dongu
#[cfg(windows)]
unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut GuiAppState;

    match msg {
        WM_CREATE => {
            SetTimer(hwnd, 1, 5, None); // 200Hz akici animasyon dongusu
            0
        }
        WM_TIMER => {
            if !state_ptr.is_null() {
                let state = &mut *state_ptr;
                state.update_animations();

                let should_render = state.is_animating()
                    || state.dirty
                    || state.last_redraw.elapsed().as_millis() >= 50;

                if should_render {
                    state.render();
                    state.dirty = false;
                    state.last_redraw = Instant::now();

                    let hdc = GetDC(hwnd);
                    blit_canvas_to_hdc(hdc, &state.canvas.pixels);
                    ReleaseDC(hwnd, hdc);
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
                state.dirty = false;
                state.last_redraw = Instant::now();
                blit_canvas_to_hdc(hdc, &state.canvas.pixels);
            }
            EndPaint(hwnd, &ps);
            0
        }
        WM_ERASEBKGND => 1,
        WM_MOUSEMOVE => {
            if !state_ptr.is_null() {
                let state = &mut *state_ptr;
                let mut cr: RECT = std::mem::zeroed();
                GetClientRect(hwnd, &mut cr);
                let cw = (cr.right - cr.left).max(1) as f32;
                let ch = (cr.bottom - cr.top).max(1) as f32;
                let raw_x = (lparam & 0xFFFF) as i16 as f32;
                let raw_y = ((lparam >> 16) & 0xFFFF) as i16 as f32;
                let x = (raw_x * (WINDOW_WIDTH as f32 / cw)).clamp(0.0, WINDOW_WIDTH as f32);
                let y = (raw_y * (WINDOW_HEIGHT as f32 / ch)).clamp(0.0, WINDOW_HEIGHT as f32);

                if state.mouse_pressed && (wparam & 0x0001) == 0 {
                    ReleaseCapture();
                    state.on_mouse_up(x, y);
                }

                state.dirty = true;
                state.on_mouse_move(x, y);

                if state.last_redraw.elapsed().as_micros() >= 3800 {
                    state.update_animations();
                    state.render();
                    state.dirty = false;
                    state.last_redraw = Instant::now();
                    let hdc = GetDC(hwnd);
                    blit_canvas_to_hdc(hdc, &state.canvas.pixels);
                    ReleaseDC(hwnd, hdc);
                }
            }
            0
        }
        WM_LBUTTONDOWN => {
            if !state_ptr.is_null() {
                SetCapture(hwnd);
                let state = &mut *state_ptr;
                let mut cr: RECT = std::mem::zeroed();
                GetClientRect(hwnd, &mut cr);
                let cw = (cr.right - cr.left).max(1) as f32;
                let ch = (cr.bottom - cr.top).max(1) as f32;
                let raw_x = (lparam & 0xFFFF) as i16 as f32;
                let raw_y = ((lparam >> 16) & 0xFFFF) as i16 as f32;
                let x = (raw_x * (WINDOW_WIDTH as f32 / cw)).clamp(0.0, WINDOW_WIDTH as f32);
                let y = (raw_y * (WINDOW_HEIGHT as f32 / ch)).clamp(0.0, WINDOW_HEIGHT as f32);

                state.dirty = true;
                state.on_mouse_down(x, y, hwnd as isize);
            }
            0
        }
        WM_LBUTTONUP => {
            if !state_ptr.is_null() {
                ReleaseCapture();
                let state = &mut *state_ptr;
                let mut cr: RECT = std::mem::zeroed();
                GetClientRect(hwnd, &mut cr);
                let cw = (cr.right - cr.left).max(1) as f32;
                let ch = (cr.bottom - cr.top).max(1) as f32;
                let raw_x = (lparam & 0xFFFF) as i16 as f32;
                let raw_y = ((lparam >> 16) & 0xFFFF) as i16 as f32;
                let x = (raw_x * (WINDOW_WIDTH as f32 / cw)).clamp(0.0, WINDOW_WIDTH as f32);
                let y = (raw_y * (WINDOW_HEIGHT as f32 / ch)).clamp(0.0, WINDOW_HEIGHT as f32);

                state.dirty = true;
                state.on_mouse_up(x, y);
            }
            0
        }
        WM_KEYDOWN => {
            if !state_ptr.is_null() {
                let state = &mut *state_ptr;
                state.dirty = true;
                state.on_key_down(wparam as u32, hwnd as isize);
            }
            0
        }
        WM_CHAR => {
            if !state_ptr.is_null() {
                let state = &mut *state_ptr;
                state.dirty = true;
                if let Some(ch) = char::from_u32(wparam as u32) {
                    state.on_char(ch, hwnd as isize);
                }
            }
            0
        }
        WM_CLOSE => {
            ShowWindow(hwnd, SW_HIDE);
            0
        }
        WM_TRAY_COMMAND => {
            if !state_ptr.is_null() {
                let state = &mut *state_ptr;
                let cmd = wparam as usize;
                match cmd {
                    0 => {
                        ShowWindow(hwnd, SW_RESTORE);
                        SetForegroundWindow(hwnd);
                    }
                    1 => {
                        state.toggle_connection();
                    }
                    2 => {
                        copy_to_clipboard(hwnd, &state.assigned_address);
                    }
                    3 => {
                        state.toggle_theme();
                        apply_dwm_theme(hwnd, state.theme.is_dark);
                        if !state.tray_popup_hwnd.is_null() {
                            apply_dwm_theme(state.tray_popup_hwnd, state.theme.is_dark);
                        }
                        if !state.tray_subpopup_hwnd.is_null() {
                            apply_dwm_theme(state.tray_subpopup_hwnd, state.theme.is_dark);
                        }
                    }
                    4 => {
                        state.core.disconnect();
                        remove_tray_icon(hwnd);
                        PostQuitMessage(0);
                    }
                    100..=999 => {
                        let srv_idx = cmd - 100;
                        if srv_idx < state.config.servers.len() {
                            state.config.active_server_idx = srv_idx;
                            state.sync_and_save_servers();
                            if state.state == ConnectionState::Disconnected {
                                state.toggle_connection();
                            }
                        }
                    }
                    _ => {}
                }
            }
            0
        }
        WM_TRAY_ICON => {
            let event = (lparam & 0xFFFF) as u32;
            if event == WM_RBUTTONUP {
                // Ozel Monokrom Sistem Tepsisi Menusu
                if !state_ptr.is_null() {
                    let state = &mut *state_ptr;
                    hide_tray_subpopup(state.tray_subpopup_hwnd);
                    if !state.tray_popup_hwnd.is_null() {
                        let mut pt: POINT = std::mem::zeroed();
                        GetCursorPos(&mut pt);

                        let is_conn = state.state == ConnectionState::Connected;
                        let active_h: i32 = if is_conn { 258 } else { 222 };
                        let popup_w = TRAY_POPUP_WIDTH as i32;

                        let screen_w = GetSystemMetrics(SM_CXSCREEN);
                        let screen_h = GetSystemMetrics(SM_CYSCREEN);

                        let mut px = pt.x - (popup_w / 2);
                        if px + popup_w > screen_w - 10 {
                            px = screen_w - popup_w - 10;
                        }
                        if px < 10 {
                            px = 10;
                        }

                        let (py, start_y) = if pt.y - active_h - 8 >= 10 {
                            let target = pt.y - active_h - 8;
                            (target, target + 12)
                        } else {
                            let target = (pt.y + 8).min(screen_h - active_h - 10);
                            (target, target - 12)
                        };

                        let popup_state_ptr = GetWindowLongPtrW(
                            state.tray_popup_hwnd,
                            GWLP_USERDATA,
                        ) as *mut TrayPopupState;
                        if !popup_state_ptr.is_null() {
                            let popup_state = &mut *popup_state_ptr;
                            popup_state.hovered_item = None;
                            popup_state.hover_t = [0.0; 5];
                            popup_state.open_t = 0.0;
                            popup_state.target_x = px;
                            popup_state.target_y = py;
                            popup_state.start_y = start_y;
                            popup_state.active_h = active_h;
                            popup_state.last_tick = Instant::now();

                            render_tray_popup(popup_state, state);
                            let hdc = GetDC(state.tray_popup_hwnd);
                            blit_canvas_to_hdc_sized(
                                hdc,
                                &popup_state.canvas.pixels,
                                TRAY_POPUP_WIDTH,
                                active_h as u32,
                            );
                            ReleaseDC(state.tray_popup_hwnd, hdc);
                        }

                        apply_dwm_theme(state.tray_popup_hwnd, state.theme.is_dark);
                        SetLayeredWindowAttributes(state.tray_popup_hwnd, 0, 0, LWA_ALPHA);
                        SetWindowPos(
                            state.tray_popup_hwnd,
                            0 as HWND,
                            px,
                            start_y,
                            popup_w,
                            active_h,
                            SWP_SHOWWINDOW,
                        );
                        SetForegroundWindow(state.tray_popup_hwnd);
                        SetTimer(state.tray_popup_hwnd, 1, 5, None);
                    }
                }
            } else if event == WM_LBUTTONDOWN {
                ShowWindow(hwnd, SW_RESTORE);
                SetForegroundWindow(hwnd);
            }
            0
        }
        WM_DESTROY => {
            if !state_ptr.is_null() {
                let state = &mut *state_ptr;
                state.core.disconnect();
                if !state.tray_subpopup_hwnd.is_null() {
                    KillTimer(state.tray_subpopup_hwnd, 1);
                    let sub_ptr = GetWindowLongPtrW(
                        state.tray_subpopup_hwnd,
                        GWLP_USERDATA,
                    ) as *mut TraySubPopupState;
                    if !sub_ptr.is_null() {
                        let _ = Box::from_raw(sub_ptr);
                    }
                    DestroyWindow(state.tray_subpopup_hwnd);
                }
                if !state.tray_popup_hwnd.is_null() {
                    KillTimer(state.tray_popup_hwnd, 1);
                    let popup_ptr = GetWindowLongPtrW(
                        state.tray_popup_hwnd,
                        GWLP_USERDATA,
                    ) as *mut TrayPopupState;
                    if !popup_ptr.is_null() {
                        let _ = Box::from_raw(popup_ptr);
                    }
                    DestroyWindow(state.tray_popup_hwnd);
                }
            }
            remove_tray_icon(hwnd);
            KillTimer(hwnd, 1);
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

pub fn run_gui(core: Arc<MaziTunnelClientCore>, is_updated: bool) {
    #[cfg(windows)]
    unsafe {
        mazitunnel_core::platform::prevent_windows_background_throttling();
        SetProcessDPIAware();

        let h_instance = GetModuleHandleW(std::ptr::null());
        let class_name = "maziTunnelMainClass\0".encode_utf16().collect::<Vec<u16>>();
        let window_title = "maziTunnel\0".encode_utf16().collect::<Vec<u16>>();

        let existing_hwnd = windows_sys::Win32::UI::WindowsAndMessaging::FindWindowW(
            class_name.as_ptr(),
            std::ptr::null(),
        );
        if !existing_hwnd.is_null() {
            ShowWindow(existing_hwnd, SW_RESTORE);
            SetForegroundWindow(existing_hwnd);
            PostMessageW(existing_hwnd, WM_TRAY_COMMAND, 0, 0);
            return;
        }

        let mut icon = LoadIconW(h_instance, 1 as *const u16);
        if icon.is_null() {
            icon = LoadIconW(0 as _, IDI_APPLICATION);
        }
        let cursor = LoadCursorW(0 as _, IDC_ARROW);

        let wnd_class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(window_proc),
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

        // Sabit 390x490 Istemci Alani ve Ekran Merkezleme Hesaplama
        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: WINDOW_WIDTH as i32,
            bottom: WINDOW_HEIGHT as i32,
        };
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

        // Baslik cubugu ve gorev cubugu icin gercek ikonu bildir
        SendMessageW(hwnd, WM_SETICON, ICON_BIG as WPARAM, icon as LPARAM);
        SendMessageW(hwnd, WM_SETICON, ICON_SMALL as WPARAM, icon as LPARAM);

        // Ozel Sistem Tepsisi Popup Pencere Sinifi
        let popup_class_name = "maziTunnelTrayPopupClass\0".encode_utf16().collect::<Vec<u16>>();
        let popup_wnd_class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(tray_popup_window_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: h_instance,
            hIcon: 0 as _,
            hCursor: cursor,
            hbrBackground: 0 as _,
            lpszMenuName: std::ptr::null(),
            lpszClassName: popup_class_name.as_ptr(),
            hIconSm: 0 as _,
        };
        RegisterClassExW(&popup_wnd_class);

        let hwnd_popup = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_LAYERED,
            popup_class_name.as_ptr(),
            std::ptr::null(),
            WS_POPUP,
            0,
            0,
            TRAY_POPUP_WIDTH as i32,
            TRAY_POPUP_HEIGHT as i32,
            0 as _,
            0 as _,
            h_instance,
            std::ptr::null(),
        );

        let round_corner: i32 = 2;
        DwmSetWindowAttribute(hwnd_popup, 33, &round_corner as *const _ as _, 4);
        SetLayeredWindowAttributes(hwnd_popup, 0, 255, LWA_ALPHA);

        // Ozel Sistem Tepsisi Alt Popup (Flyout Submenu) Pencere Sinifi
        let subpopup_class_name =
            "maziTunnelTraySubPopupClass\0".encode_utf16().collect::<Vec<u16>>();
        let subpopup_wnd_class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(tray_subpopup_window_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: h_instance,
            hIcon: 0 as _,
            hCursor: cursor,
            hbrBackground: 0 as _,
            lpszMenuName: std::ptr::null(),
            lpszClassName: subpopup_class_name.as_ptr(),
            hIconSm: 0 as _,
        };
        RegisterClassExW(&subpopup_wnd_class);

        let hwnd_subpopup = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_LAYERED,
            subpopup_class_name.as_ptr(),
            std::ptr::null(),
            WS_POPUP,
            0,
            0,
            TRAY_SUBPOPUP_WIDTH as i32,
            TRAY_SUBPOPUP_MAX_HEIGHT as i32,
            0 as _,
            0 as _,
            h_instance,
            std::ptr::null(),
        );

        DwmSetWindowAttribute(hwnd_subpopup, 33, &round_corner as *const _ as _, 4);
        SetLayeredWindowAttributes(hwnd_subpopup, 0, 255, LWA_ALPHA);

        let subpopup_state = Box::new(TraySubPopupState {
            canvas: Canvas::new(TRAY_SUBPOPUP_WIDTH, TRAY_SUBPOPUP_MAX_HEIGHT),
            main_hwnd: hwnd,
            tray_popup_hwnd: hwnd_popup,
            hovered_item: None,
            hover_t: Vec::new(),
            open_t: 1.0,
            start_x: 0,
            target_x: 0,
            cur_x: 0,
            target_y: 0,
            active_h: 200,
            last_tick: Instant::now(),
            is_visible: false,
        });
        SetWindowLongPtrW(
            hwnd_subpopup,
            GWLP_USERDATA,
            Box::into_raw(subpopup_state) as isize,
        );

        let popup_state = Box::new(TrayPopupState {
            canvas: Canvas::new(TRAY_POPUP_WIDTH, TRAY_POPUP_HEIGHT),
            main_hwnd: hwnd,
            subpopup_hwnd: hwnd_subpopup,
            hovered_item: None,
            hover_t: [0.0; 5],
            open_t: 1.0,
            target_x: 0,
            target_y: 0,
            start_y: 0,
            active_h: 222,
            last_tick: Instant::now(),
        });
        SetWindowLongPtrW(
            hwnd_popup,
            GWLP_USERDATA,
            Box::into_raw(popup_state) as isize,
        );

        // Windows 11 DWM Yuvarlak Kose (DWMWCP_ROUND = 2)
        let round_val = 2i32;
        DwmSetWindowAttribute(hwnd, 33, &round_val as *const _ as _, 4);

        let mut app_state = Box::new(GuiAppState::new(core, is_updated));
        app_state.main_hwnd = hwnd;
        app_state.tray_popup_hwnd = hwnd_popup;
        app_state.tray_subpopup_hwnd = hwnd_subpopup;
        let is_dark = app_state.theme.is_dark;
        let state_ptr = Box::into_raw(app_state);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, state_ptr as isize);

        apply_dwm_theme(hwnd, is_dark);
        apply_dwm_theme(hwnd_popup, is_dark);
        apply_dwm_theme(hwnd_subpopup, is_dark);
        setup_tray_icon(hwnd, icon);

        ShowWindow(hwnd, SW_SHOW);
        windows_sys::Win32::Graphics::Gdi::UpdateWindow(hwnd);

        SetWindowPos(
            hwnd,
            -1isize as HWND,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
        );
        SetWindowPos(
            hwnd,
            -2isize as HWND,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
        );
        SetForegroundWindow(hwnd);

        let mut msg: MSG = std::mem::zeroed();
        'main_loop: loop {
            let state = &mut *state_ptr;
            let active = state.is_animating()
                || state.dirty
                || state.switch_is_dragging
                || state.input_is_selecting;
            if active {
                while PeekMessageW(&mut msg, 0 as _, 0, 0, PM_REMOVE) != 0 {
                    if msg.message == WM_QUIT {
                        break 'main_loop;
                    }
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
                if state.last_redraw.elapsed().as_micros() >= 4000 {
                    state.update_animations();
                    state.render();
                    state.dirty = false;
                    state.last_redraw = Instant::now();
                    let hdc = GetDC(hwnd);
                    blit_canvas_to_hdc(hdc, &state.canvas.pixels);
                    ReleaseDC(hwnd, hdc);
                } else {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
            } else {
                let ret = GetMessageW(&mut msg, 0 as _, 0, 0);
                if ret <= 0 {
                    break 'main_loop;
                }
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }

        let _ = Box::from_raw(state_ptr);
    }

    #[cfg(not(windows))]
    {
        let _ = (core, is_updated);
        println!("maziTunnel GUI yalnızca Windows platformunda çalışır. Başsız çalıştırma için --cli argümanını kullanın.");
    }
}
