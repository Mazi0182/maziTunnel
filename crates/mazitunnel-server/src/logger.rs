use std::collections::VecDeque;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

pub const MAX_LOG_HISTORY: usize = 200;

#[derive(Clone)]
pub struct ServerLogger {
    inner: Arc<RwLock<LoggerInner>>,
    pub verbose: bool,
}

struct LoggerInner {
    #[allow(dead_code)]
    config_dir: PathBuf,
    summary_file: PathBuf,
    detailed_file: PathBuf,
    summary_logs: VecDeque<String>,
    detailed_logs: VecDeque<String>,
}

impl ServerLogger {
    pub fn new(config_dir: PathBuf, verbose: bool) -> Self {
        let _ = std::fs::create_dir_all(&config_dir);
        let summary_file = config_dir.join("summary.log");
        let detailed_file = config_dir.join("detailed.log");

        Self {
            inner: Arc::new(RwLock::new(LoggerInner {
                config_dir,
                summary_file,
                detailed_file,
                summary_logs: VecDeque::with_capacity(MAX_LOG_HISTORY),
                detailed_logs: VecDeque::with_capacity(MAX_LOG_HISTORY),
            })),
            verbose,
        }
    }

    pub fn log_summary(&self, text: &str) {
        let time_str = current_local_hms_str();
        let line = format!("[{}] {}", time_str, text);

        // Always print readable summary logs to console
        println!("{}", line);

        if let Ok(mut inner) = self.inner.write() {
            if inner.summary_logs.len() >= MAX_LOG_HISTORY {
                inner.summary_logs.pop_front();
            }
            inner.summary_logs.push_back(line.clone());

            if let Ok(mut file) = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&inner.summary_file)
            {
                let _ = writeln!(file, "{}", line);
            }
        }
    }

    pub fn log_detailed(&self, event_code: &str, text: &str) {
        let dt_str = current_local_datetime_str();
        let line = format!("[{}] [{}] {}", dt_str, event_code, text);

        // Print to console only if verbose flag is set
        if self.verbose {
            println!("{}", line);
        }

        if let Ok(mut inner) = self.inner.write() {
            if inner.detailed_logs.len() >= MAX_LOG_HISTORY {
                inner.detailed_logs.pop_front();
            }
            inner.detailed_logs.push_back(line.clone());

            if let Ok(mut file) = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&inner.detailed_file)
            {
                let _ = writeln!(file, "{}", line);
            }
        }
    }

    pub fn log_connect(
        &self,
        username: &str,
        is_admin: bool,
        peer_addr: &str,
        virtual_ip: &str,
        hwid: &str,
        os: &str,
        version: &str,
    ) {
        if is_admin {
            self.log_summary(&format!("{} (Yonetici) baglandi.", username));
        } else {
            self.log_summary(&format!("{} baglandi.", username));
        }

        self.log_detailed(
            "AUTH_OK",
            &format!(
                "Kullanici: {}, Rol: {}, Adres: {}, Sanal IP: {}, HWID: {}, OS: {}, Surum: {}",
                username,
                if is_admin { "ADMIN" } else { "PLAYER" },
                peer_addr,
                virtual_ip,
                hwid,
                os,
                version
            ),
        );
    }

    pub fn log_game_join(&self, username: &str, peer_addr: &str, stream_id: u32, target_port: u16) {
        self.log_summary(&format!("{} oyuna giris yapti.", username));
        self.log_detailed(
            "STREAM_OPEN",
            &format!(
                "Oyuna giris yapildi. Kullanici: {}, Adres: {}, Stream: {}, Port: {}",
                username, peer_addr, stream_id, target_port
            ),
        );
    }

    pub fn log_disconnect(
        &self,
        username: &str,
        duration_secs: u64,
        rx: u64,
        tx: u64,
        peer_addr: &str,
        is_timeout: bool,
    ) {
        let mins = duration_secs / 60;
        let secs = duration_secs % 60;
        let dur_str = format!("{}dk {}sn", mins, secs);

        self.log_summary(&format!("{} ayrildi (Sure: {}).", username, dur_str));

        let event = if is_timeout {
            "SESSION_TIMEOUT"
        } else {
            "SESSION_DISCONNECT"
        };

        self.log_detailed(
            event,
            &format!(
                "Kullanici ayrildi: {}, Adres: {}, Sure: {}s, RX: {} bytes, TX: {} bytes",
                username, peer_addr, duration_secs, rx, tx
            ),
        );
    }

    pub fn log_kick(&self, username: &str, peer_addr: &str, reason: &str) {
        self.log_summary(&format!("{} sunucudan atildi.", username));
        self.log_detailed(
            "KICK",
            &format!(
                "Kullanici atildi: {}, Adres: {}, Sebep: {}",
                username, peer_addr, reason
            ),
        );
    }

    pub fn log_ban(&self, username: &str, hwid: &str, reason: &str) {
        self.log_summary(&format!("{} yasaklandi (Sebep: {}).", username, reason));
        self.log_detailed(
            "BAN",
            &format!(
                "Cihaz yasaklandi. Kullanici: {}, HWID: {}, Sebep: {}",
                username, hwid, reason
            ),
        );
    }

    pub fn log_banned_attempt(&self, username: &str, hwid: &str, peer_addr: &str) {
        self.log_summary(&format!(
            "Engellenen giris denemesi ({} - Yasakli cihaz).",
            username
        ));
        self.log_detailed(
            "AUTH_FAIL",
            &format!(
                "Yasakli cihaz giris engellendi. Kullanici: {}, HWID: {}, Adres: {}",
                username, hwid, peer_addr
            ),
        );
    }

    pub fn log_key_regenerated(&self, new_key: &str) {
        self.log_summary("Sunucu baglanti kodu yenilendi.");
        self.log_detailed(
            "KEY_REGENERATE",
            &format!("Yeni baglanti kodu olusturuldu: {}", new_key),
        );
    }

    pub fn log_stun(&self, public_endpoint: &str, is_symmetric: bool) {
        self.log_detailed(
            "STUN",
            &format!(
                "Dis ag public endpoint cozuldu: {} (Simetrik NAT: {})",
                public_endpoint, is_symmetric
            ),
        );
    }

    pub fn get_logs(&self) -> (Vec<String>, Vec<String>) {
        if let Ok(inner) = self.inner.read() {
            (
                inner.summary_logs.iter().cloned().collect(),
                inner.detailed_logs.iter().cloned().collect(),
            )
        } else {
            (Vec::new(), Vec::new())
        }
    }
}

#[cfg(unix)]
pub fn current_local_hms_str() -> String {
    let now_t = unsafe { libc::time(std::ptr::null_mut()) };
    let mut tm = libc::tm {
        tm_sec: 0,
        tm_min: 0,
        tm_hour: 0,
        tm_mday: 0,
        tm_mon: 0,
        tm_year: 0,
        tm_wday: 0,
        tm_yday: 0,
        tm_isdst: 0,
        tm_gmtoff: 0,
        tm_zone: std::ptr::null(),
    };
    unsafe {
        libc::localtime_r(&now_t, &mut tm);
    }
    format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec)
}

#[cfg(unix)]
pub fn current_local_datetime_str() -> String {
    let now_t = unsafe { libc::time(std::ptr::null_mut()) };
    let mut tm = libc::tm {
        tm_sec: 0,
        tm_min: 0,
        tm_hour: 0,
        tm_mday: 0,
        tm_mon: 0,
        tm_year: 0,
        tm_wday: 0,
        tm_yday: 0,
        tm_isdst: 0,
        tm_gmtoff: 0,
        tm_zone: std::ptr::null(),
    };
    unsafe {
        libc::localtime_r(&now_t, &mut tm);
    }
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec
    )
}

#[cfg(not(unix))]
pub fn current_local_hms_str() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let sec = epoch % 60;
    let min = (epoch / 60) % 60;
    let hour = (epoch / 3600) % 24;
    format!("{:02}:{:02}:{:02}", hour, min, sec)
}

#[cfg(not(unix))]
pub fn current_local_datetime_str() -> String {
    crate::ban_manager::format_current_time()
}
