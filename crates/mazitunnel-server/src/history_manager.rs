use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use serde::{Deserialize, Serialize};

use crate::ban_manager::{format_current_time, BanManager};
use crate::panel_api::ActiveClientSnapshot;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionHistoryEntry {
    pub id: String,
    pub key: String,
    pub hwid: String,
    pub username: String,
    pub hostname: String,
    pub addr: String,
    pub ip: String,
    pub os: String,
    pub version: String,
    pub role: String,
    pub active: bool,
    pub joined_at: String,
    pub left_at: Option<String>,
    pub login_count: u32,
    pub duration: u64,
    pub rx: u64,
    pub tx: u64,
    #[serde(default)]
    pub hwid_details: serde_json::Value,
    #[serde(default)]
    pub virtual_ip: Option<String>,
}

pub struct HistoryManager {
    config_dir: PathBuf,
    history_file: PathBuf,
    history: HashMap<String, SessionHistoryEntry>,
}

impl HistoryManager {
    pub fn new(config_dir: PathBuf) -> Self {
        let history_file = config_dir.join("client_history.json");
        let mut manager = Self {
            config_dir,
            history_file,
            history: HashMap::new(),
        };
        manager.load_history();
        manager
    }

    pub fn load_history(&mut self) {
        self.history.clear();
        let target_file = if self.history_file.exists() {
            Some(self.history_file.clone())
        } else {
            let legacy_file = self.config_dir.join("connection_history.json");
            if legacy_file.exists() {
                Some(legacy_file)
            } else {
                None
            }
        };

        let Some(file_path) = target_file else {
            return;
        };

        let Ok(content) = fs::read_to_string(&file_path) else {
            return;
        };

        if let Ok(map) = serde_json::from_str::<HashMap<String, SessionHistoryEntry>>(&content) {
            self.history = map;
            self.sanitize_and_deduplicate_on_load();
            return;
        }

        if let Ok(list) = serde_json::from_str::<Vec<SessionHistoryEntry>>(&content) {
            for entry in list {
                self.history.insert(entry.key.clone(), entry);
            }
            self.sanitize_and_deduplicate_on_load();
            return;
        }

        // Support loose Python/Node map format
        if let Ok(raw_map) = serde_json::from_str::<HashMap<String, serde_json::Value>>(&content) {
            for (key, val) in raw_map {
                let id = val.get("id").and_then(|v| v.as_str()).unwrap_or(&key).to_string();
                let rec_key = val.get("key").and_then(|v| v.as_str()).unwrap_or(&key).to_string();
                let hwid = val.get("hwid").and_then(|v| v.as_str()).unwrap_or("N/A").to_string();
                let username = val.get("username").and_then(|v| v.as_str()).unwrap_or("Bilinmeyen").to_string();
                let hostname = val.get("hostname").or_else(|| val.get("computer_name")).and_then(|v| v.as_str()).unwrap_or("-").to_string();
                let addr = val.get("addr").or_else(|| val.get("peer_addr")).and_then(|v| v.as_str()).unwrap_or("-").to_string();
                let ip = val.get("ip").and_then(|v| v.as_str()).unwrap_or("-").to_string();
                let os = val.get("os").and_then(|v| v.as_str()).unwrap_or("Bilinmiyor").to_string();
                let version = val.get("version").and_then(|v| v.as_str()).unwrap_or("-").to_string();
                let role = val.get("role").and_then(|v| v.as_str()).unwrap_or("PLAYER").to_string();
                let active = val.get("active").or_else(|| val.get("is_online")).and_then(|v| v.as_bool()).unwrap_or(false);
                let joined_at = val.get("joined_at").or_else(|| val.get("first_seen")).and_then(|v| v.as_str()).unwrap_or("").to_string();
                let left_at = val.get("left_at").and_then(|v| v.as_str()).map(|s| s.to_string());
                let login_count = val.get("login_count").and_then(|v| v.as_u64()).unwrap_or(1) as u32;
                let duration = val.get("duration").or_else(|| val.get("duration_seconds")).and_then(|v| v.as_u64()).unwrap_or(0);
                let rx = val.get("rx").or_else(|| val.get("bytes_received")).and_then(|v| v.as_u64()).unwrap_or(0);
                let tx = val.get("tx").or_else(|| val.get("bytes_sent")).and_then(|v| v.as_u64()).unwrap_or(0);
                let hwid_details = val.get("hwid_details").cloned().unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
                let virtual_ip = val.get("virtual_ip").and_then(|v| v.as_str()).map(|s| s.to_string());

                self.history.insert(
                    rec_key.clone(),
                    SessionHistoryEntry {
                        id,
                        key: rec_key,
                        hwid,
                        username,
                        hostname,
                        addr,
                        ip,
                        os,
                        version,
                        role,
                        active,
                        joined_at,
                        left_at,
                        login_count,
                        duration,
                        rx,
                        tx,
                        hwid_details,
                        virtual_ip,
                    },
                );
            }
            self.sanitize_and_deduplicate_on_load();
        }
    }

    fn extract_clean_ip(ip: &str, addr: &str) -> String {
        let candidate = if !ip.trim().is_empty() && ip.trim() != "-" {
            ip.trim()
        } else {
            addr.trim()
        };
        if let Some((host, _port)) = candidate.rsplit_once(':') {
            if !host.contains(':') {
                return host.to_string();
            }
        }
        candidate.to_string()
    }

    fn sanitize_and_deduplicate_on_load(&mut self) {
        let mut entries: Vec<SessionHistoryEntry> = self.history.drain().map(|(_, v)| v).collect();
        if entries.is_empty() {
            return;
        }

        // Sort oldest to newest by last_seen so newest entry metadata wins during merge
        entries.sort_by(|a, b| {
            let a_seen = a.left_at.as_deref().unwrap_or(&a.joined_at);
            let b_seen = b.left_at.as_deref().unwrap_or(&b.joined_at);
            a_seen.cmp(b_seen)
        });

        let mut merged_list: Vec<SessionHistoryEntry> = Vec::new();

        for mut entry in entries {
            // On daemon startup, no clients are connected yet; clear any stale active flags
            if entry.active {
                entry.active = false;
                if entry.left_at.as_deref().unwrap_or("").is_empty() && !entry.joined_at.is_empty() {
                    entry.left_at = Some(entry.joined_at.clone());
                }
            }
            entry.ip = Self::extract_clean_ip(&entry.ip, &entry.addr);

            let entry_user = entry.username.trim().to_lowercase();
            let entry_host = entry.hostname.trim().to_lowercase();
            let has_valid_host = !entry_host.is_empty() && entry_host != "-" && entry_host != "bilinmiyor";
            let entry_guid = entry
                .hwid_details
                .get("machine_guid")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_lowercase();

            let existing_idx = merged_list.iter().position(|existing| {
                if !entry.hwid.is_empty()
                    && entry.hwid != "N/A"
                    && existing.hwid.eq_ignore_ascii_case(&entry.hwid)
                {
                    return true;
                }
                if !entry_guid.is_empty() {
                    let ex_guid = existing
                        .hwid_details
                        .get("machine_guid")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .trim()
                        .to_lowercase();
                    if ex_guid == entry_guid && existing.username.trim().to_lowercase() == entry_user {
                        return true;
                    }
                }
                if has_valid_host
                    && existing.username.trim().to_lowercase() == entry_user
                    && existing.hostname.trim().to_lowercase() == entry_host
                {
                    return true;
                }
                false
            });

            if let Some(idx) = existing_idx {
                let prev = &mut merged_list[idx];
                let earliest_joined = if !prev.joined_at.is_empty()
                    && (entry.joined_at.is_empty() || prev.joined_at < entry.joined_at)
                {
                    prev.joined_at.clone()
                } else {
                    entry.joined_at.clone()
                };

                let prev_last = prev.left_at.as_deref().unwrap_or(&prev.joined_at);
                let entry_last = entry.left_at.as_deref().unwrap_or(&entry.joined_at);
                let latest_left = if entry_last >= prev_last {
                    entry.left_at.clone().or_else(|| Some(entry.joined_at.clone()))
                } else {
                    prev.left_at.clone().or_else(|| Some(prev.joined_at.clone()))
                };

                let total_logins = prev.login_count.max(1).saturating_add(entry.login_count.max(1));
                let total_duration = prev.duration.saturating_add(entry.duration);
                let total_rx = prev.rx.saturating_add(entry.rx);
                let total_tx = prev.tx.saturating_add(entry.tx);
                let preserved_vip = entry
                    .virtual_ip
                    .clone()
                    .filter(|s| !s.trim().is_empty())
                    .or_else(|| prev.virtual_ip.clone().filter(|s| !s.trim().is_empty()));
                let preserved_role = if entry.role == "ADMIN" || prev.role == "ADMIN" {
                    "ADMIN".to_string()
                } else {
                    entry.role.clone()
                };

                entry.joined_at = earliest_joined;
                entry.left_at = latest_left;
                entry.login_count = total_logins;
                entry.duration = total_duration;
                entry.rx = total_rx;
                entry.tx = total_tx;
                entry.virtual_ip = preserved_vip;
                entry.role = preserved_role;

                *prev = entry;
            } else {
                merged_list.push(entry);
            }
        }

        for entry in merged_list {
            self.history.insert(entry.key.clone(), entry);
        }
        let _ = self.save_history();
    }

    pub fn save_history(&self) -> bool {
        let _ = fs::create_dir_all(&self.config_dir);
        let tmp_file = self.history_file.with_extension("tmp");

        let serialized = match serde_json::to_string_pretty(&self.history) {
            Ok(s) => s,
            Err(_) => return false,
        };

        if fs::write(&tmp_file, serialized.as_bytes()).is_err() {
            return false;
        }

        if fs::rename(&tmp_file, &self.history_file).is_err() {
            let _ = fs::copy(&tmp_file, &self.history_file);
            let _ = fs::remove_file(&tmp_file);
        }
        true
    }

    pub fn get_or_assign_virtual_ip(&mut self, hwid: &str, username: &str) -> String {
        let hwid_clean = hwid.trim().to_uppercase();
        let user_clean = username.trim().to_string();
        let has_valid_hwid = !hwid_clean.is_empty() && hwid_clean.len() >= 10 && hwid_clean != "N/A";
        let key = if has_valid_hwid {
            hwid_clean.clone()
        } else {
            format!("USER_{}", user_clean.to_lowercase())
        };

        // 1. Direct key match
        if let Some(entry) = self.history.get(&key) {
            if let Some(vip) = &entry.virtual_ip {
                if !vip.trim().is_empty() {
                    return vip.clone();
                }
            }
        }

        // 2. Linear search by hwid or username
        if has_valid_hwid {
            for entry in self.history.values() {
                if entry.hwid.eq_ignore_ascii_case(&hwid_clean) {
                    if let Some(vip) = &entry.virtual_ip {
                        if !vip.trim().is_empty() {
                            return vip.clone();
                        }
                    }
                }
            }
        } else if !user_clean.is_empty() {
            for entry in self.history.values() {
                if entry.username.eq_ignore_ascii_case(&user_clean) {
                    if let Some(vip) = &entry.virtual_ip {
                        if !vip.trim().is_empty() {
                            return vip.clone();
                        }
                    }
                }
            }
        }

        // 3. Collect allocated IPs
        let mut assigned = HashSet::new();
        for entry in self.history.values() {
            if let Some(vip) = &entry.virtual_ip {
                assigned.insert(vip.clone());
            }
        }

        // 4. Allocate next free 100.0.X.Y starting from 100.0.0.2
        let mut allocated = None;
        'outer: for subnet in 0..255 {
            for host in 2..255 {
                let candidate = format!("100.0.{}.{}", subnet, host);
                if !assigned.contains(&candidate) {
                    allocated = Some(candidate);
                    break 'outer;
                }
            }
        }

        let final_ip = allocated.unwrap_or_else(|| "100.0.0.2".to_string());

        // 5. Store and save
        if let Some(entry) = self.history.get_mut(&key) {
            entry.virtual_ip = Some(final_ip.clone());
        } else {
            self.history.insert(
                key.clone(),
                SessionHistoryEntry {
                    id: key.clone(),
                    key,
                    hwid: if has_valid_hwid { hwid_clean } else { "N/A".to_string() },
                    username: if user_clean.is_empty() { "Bilinmeyen".to_string() } else { user_clean },
                    hostname: "-".to_string(),
                    addr: "-".to_string(),
                    ip: "-".to_string(),
                    os: "Bilinmiyor".to_string(),
                    version: "-".to_string(),
                    role: "PLAYER".to_string(),
                    active: false,
                    joined_at: String::new(),
                    left_at: None,
                    login_count: 0,
                    duration: 0,
                    rx: 0,
                    tx: 0,
                    hwid_details: serde_json::Value::Object(serde_json::Map::new()),
                    virtual_ip: Some(final_ip.clone()),
                },
            );
        }
        let _ = self.save_history();
        final_ip
    }

    pub fn record_join(
        &mut self,
        hwid: &str,
        username: &str,
        hostname: &str,
        addr: &str,
        ip: &str,
        os: &str,
        version: &str,
        role: &str,
        hwid_details: serde_json::Value,
    ) -> String {
        let hwid_clean = hwid.trim().to_uppercase();
        let user_clean = if username.trim().is_empty() { "Bilinmeyen".to_string() } else { username.trim().to_string() };
        let has_valid_hwid = !hwid_clean.is_empty() && hwid_clean.len() >= 10 && hwid_clean != "N/A";
        let key = if has_valid_hwid {
            hwid_clean.clone()
        } else {
            format!("USER_{}", user_clean.to_lowercase())
        };

        let virtual_ip = self.get_or_assign_virtual_ip(&hwid_clean, &user_clean);
        let now = format_current_time();
        let host_clean = if hostname.trim().is_empty() { "-".to_string() } else { hostname.trim().to_string() };
        let has_valid_host = host_clean != "-" && !host_clean.eq_ignore_ascii_case("bilinmiyor");

        let mut login_count = 1;
        let mut joined_at = now.clone();
        let mut existing_rx = 0;
        let mut existing_tx = 0;
        let mut existing_dur = 0;
        let mut entry_id = key.clone();

        // Check if there is an existing record by key OR by (username, hostname)
        let matched_old_key = if self.history.contains_key(&key) {
            Some(key.clone())
        } else if has_valid_host {
            self.history
                .iter()
                .find(|(_, e)| {
                    e.username.trim().eq_ignore_ascii_case(&user_clean)
                        && e.hostname.trim().eq_ignore_ascii_case(&host_clean)
                })
                .map(|(k, _)| k.clone())
        } else {
            None
        };

        if let Some(old_key) = matched_old_key {
            if let Some(existing) = self.history.get(&old_key) {
                login_count = existing.login_count + 1;
                joined_at = if !existing.joined_at.is_empty() {
                    existing.joined_at.clone()
                } else {
                    now.clone()
                };
                existing_rx = existing.rx;
                existing_tx = existing.tx;
                existing_dur = existing.duration;
                entry_id = existing.id.clone();
            }
            if old_key != key {
                self.history.remove(&old_key);
            }
        }

        let clean_ip = Self::extract_clean_ip(ip, addr);

        let entry = SessionHistoryEntry {
            id: entry_id,
            key: key.clone(),
            hwid: if has_valid_hwid { hwid_clean } else { "N/A".to_string() },
            username: user_clean,
            hostname: host_clean,
            addr: addr.to_string(),
            ip: clean_ip,
            os: if os.trim().is_empty() { "Bilinmiyor".to_string() } else { os.trim().to_string() },
            version: if version.trim().is_empty() { "-".to_string() } else { version.trim().to_string() },
            role: role.to_string(),
            active: true,
            joined_at,
            left_at: None,
            login_count,
            duration: existing_dur,
            rx: existing_rx,
            tx: existing_tx,
            hwid_details,
            virtual_ip: Some(virtual_ip.clone()),
        };

        self.history.insert(key, entry);
        self.save_history();
        virtual_ip
    }

    pub fn record_leave(&mut self, addr_or_hwid: &str, duration_secs: u64, rx: u64, tx: u64) {
        let clean = addr_or_hwid.trim();
        let now = format_current_time();
        let mut changed = false;

        for entry in self.history.values_mut() {
            let match_found = entry.addr == clean
                || entry.hwid.eq_ignore_ascii_case(clean)
                || entry.key.eq_ignore_ascii_case(clean);

            if match_found {
                entry.active = false;
                entry.left_at = Some(now.clone());
                entry.duration = entry.duration.saturating_add(duration_secs);
                entry.rx = entry.rx.saturating_add(rx);
                entry.tx = entry.tx.saturating_add(tx);
                changed = true;
            }
        }

        if changed {
            self.save_history();
        }
    }

    pub fn update_metrics(&mut self, addr_or_hwid: &str, duration: u64, rx: u64, tx: u64) {
        let clean = addr_or_hwid.trim();
        for entry in self.history.values_mut() {
            if entry.addr == clean || entry.hwid.eq_ignore_ascii_case(clean) {
                entry.duration = duration;
                entry.rx = rx;
                entry.tx = tx;
                break;
            }
        }
    }

    pub fn clear_history(&mut self) -> bool {
        self.history.clear();
        self.save_history()
    }

    pub fn list_history(
        &self,
        ban_manager: &BanManager,
        active_clients: &[ActiveClientSnapshot],
    ) -> Vec<serde_json::Value> {
        let mut list = Vec::new();

        for entry in self.history.values() {
            let is_banned = ban_manager
                .is_banned(&entry.hwid, &entry.hwid_details)
                .is_some();

            let is_truly_online = active_clients.iter().any(|c| {
                (!entry.hwid.is_empty()
                    && entry.hwid != "N/A"
                    && c.hwid.eq_ignore_ascii_case(&entry.hwid))
                    || (!entry.addr.is_empty() && entry.addr != "-" && c.addr == entry.addr)
            });

            let last_seen = entry.left_at.as_deref().unwrap_or(&entry.joined_at);
            let clean_ip = Self::extract_clean_ip(&entry.ip, &entry.addr);

            let val = serde_json::json!({
                "id": entry.id,
                "key": entry.key,
                "hwid": entry.hwid,
                "username": entry.username,
                "hostname": entry.hostname,
                "computer_name": entry.hostname,
                "addr": entry.addr,
                "peer_addr": entry.addr,
                "ip": clean_ip,
                "os": entry.os,
                "version": entry.version,
                "role": entry.role,
                "active": is_truly_online,
                "is_online": is_truly_online,
                "joined_at": entry.joined_at,
                "first_seen": entry.joined_at,
                "left_at": entry.left_at,
                "last_seen": last_seen,
                "login_count": entry.login_count,
                "duration": entry.duration,
                "duration_seconds": entry.duration,
                "rx": entry.rx,
                "bytes_received": entry.rx,
                "tx": entry.tx,
                "bytes_sent": entry.tx,
                "virtual_ip": entry.virtual_ip.as_deref().unwrap_or(""),
                "hwid_details": entry.hwid_details,
                "is_banned": is_banned,
            });

            list.push(val);
        }

        // Online first, then latest last_seen
        list.sort_by(|a, b| {
            let a_online = a.get("active").and_then(|v| v.as_bool()).unwrap_or(false);
            let b_online = b.get("active").and_then(|v| v.as_bool()).unwrap_or(false);

            if a_online != b_online {
                return b_online.cmp(&a_online);
            }

            let a_seen = a.get("last_seen").and_then(|v| v.as_str()).unwrap_or("");
            let b_seen = b.get("last_seen").and_then(|v| v.as_str()).unwrap_or("");
            b_seen.cmp(a_seen)
        });

        list
    }
}
