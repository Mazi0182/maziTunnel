use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use rand::RngCore;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BanEntry {
    pub id: String,
    pub hwid: String,
    pub username: String,
    pub hostname: String,
    pub ip: String,
    pub reason: String,
    pub banned_at: String,
    #[serde(default)]
    pub hwid_details: serde_json::Value,
}

pub struct BanManager {
    config_dir: PathBuf,
    bans_file: PathBuf,
    bans: HashMap<String, BanEntry>,
}

impl BanManager {
    pub fn new(config_dir: PathBuf) -> Self {
        let bans_file = config_dir.join("banned_clients.json");
        let mut manager = Self {
            config_dir,
            bans_file,
            bans: HashMap::new(),
        };
        manager.load_bans();
        manager
    }

    pub fn load_bans(&mut self) {
        self.bans.clear();
        let target_file = if self.bans_file.exists() {
            Some(self.bans_file.clone())
        } else {
            let legacy_file = self.config_dir.join("bans.json");
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

        if let Ok(map) = serde_json::from_str::<HashMap<String, BanEntry>>(&content) {
            self.bans = map;
            return;
        }

        if let Ok(list) = serde_json::from_str::<Vec<BanEntry>>(&content) {
            for entry in list {
                self.bans.insert(entry.id.clone(), entry);
            }
            return;
        }

        // Support loose dictionary format from older Python/JS servers
        if let Ok(raw_map) = serde_json::from_str::<HashMap<String, serde_json::Value>>(&content) {
            for (key, val) in raw_map {
                let id = val.get("id").and_then(|v| v.as_str()).unwrap_or(&key).to_string();
                let hwid = val.get("hwid").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let username = val.get("username").and_then(|v| v.as_str()).unwrap_or("Bilinmeyen").to_string();
                let hostname = val.get("hostname").or_else(|| val.get("computer_name")).and_then(|v| v.as_str()).unwrap_or("-").to_string();
                let ip = val.get("ip").and_then(|v| v.as_str()).unwrap_or("-").to_string();
                let reason = val.get("reason").and_then(|v| v.as_str()).unwrap_or("Yönetici tarafından yasaklandı").to_string();
                let banned_at = val.get("banned_at").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let hwid_details = val.get("hwid_details").cloned().unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

                self.bans.insert(
                    id.clone(),
                    BanEntry {
                        id,
                        hwid,
                        username,
                        hostname,
                        ip,
                        reason,
                        banned_at,
                        hwid_details,
                    },
                );
            }
        }
    }

    pub fn save_bans(&self) -> bool {
        let _ = fs::create_dir_all(&self.config_dir);
        let tmp_file = self.bans_file.with_extension("tmp");

        let serialized = match serde_json::to_string_pretty(&self.bans) {
            Ok(s) => s,
            Err(_) => return false,
        };

        if fs::write(&tmp_file, serialized.as_bytes()).is_err() {
            return false;
        }

        if fs::rename(&tmp_file, &self.bans_file).is_err() {
            let _ = fs::copy(&tmp_file, &self.bans_file);
            let _ = fs::remove_file(&tmp_file);
        }
        true
    }

    pub fn is_banned(&self, hwid: &str, hwid_details: &serde_json::Value) -> Option<BanEntry> {
        let hwid_clean = hwid.trim().to_uppercase();
        let details_obj = hwid_details.as_object();

        let client_mb = extract_field(details_obj, "motherboard_uuid");
        let client_mguid = extract_field(details_obj, "machine_guid");
        let client_vol = extract_field(details_obj, "volume_serial");
        let client_mac = extract_field(details_obj, "mac");
        let client_dev = extract_field(details_obj, "device_id");

        if (hwid_clean.is_empty() || hwid_clean.len() < 12)
            && client_mb.is_none()
            && client_mguid.is_none()
            && client_vol.is_none()
            && client_dev.is_none()
        {
            return None;
        }

        for record in self.bans.values() {
            let b_hwid = record.hwid.trim().to_uppercase();
            let b_details = record.hwid_details.as_object();

            // 1. Composite HWID match (exact or prefix match if ban was truncated)
            if !hwid_clean.is_empty() && hwid_clean.len() >= 12 && !b_hwid.is_empty() && b_hwid.len() >= 12 {
                let clean_b = b_hwid.trim_end_matches('.');
                if hwid_clean == clean_b {
                    return Some(record.clone());
                }
                if (b_hwid.ends_with("...") || clean_b.len() < 24) && clean_b.len() >= 12 {
                    if hwid_clean.starts_with(clean_b) {
                        return Some(record.clone());
                    }
                }
            }

            // 2. Motherboard UUID match
            let b_mb = extract_field(b_details, "motherboard_uuid");
            if let (Some(c_mb), Some(b_mb)) = (&client_mb, &b_mb) {
                if is_valid_mb_uuid(c_mb) && is_valid_mb_uuid(b_mb) && c_mb.eq_ignore_ascii_case(b_mb) {
                    return Some(record.clone());
                }
            }

            // 3. MachineGuid match
            let b_mguid = extract_field(b_details, "machine_guid");
            if let (Some(c_g), Some(b_g)) = (&client_mguid, &b_mguid) {
                if c_g.len() > 12 && c_g.eq_ignore_ascii_case(b_g) {
                    return Some(record.clone());
                }
            }

            // 4. Volume Serial match
            let b_vol = extract_field(b_details, "volume_serial");
            if let (Some(c_v), Some(b_v)) = (&client_vol, &b_vol) {
                if c_v.len() > 4 && c_v.eq_ignore_ascii_case(b_v) {
                    return Some(record.clone());
                }
            }

            // 5. Persistent Device ID match
            let b_dev = extract_field(b_details, "device_id");
            if let (Some(c_d), Some(b_d)) = (&client_dev, &b_dev) {
                if c_d.len() >= 16 && c_d.eq_ignore_ascii_case(b_d) {
                    return Some(record.clone());
                }
            }

            // 6. MAC address match
            let b_mac = extract_field(b_details, "mac");
            if let (Some(c_m), Some(b_m)) = (&client_mac, &b_mac) {
                if c_m.len() >= 12 && c_m.eq_ignore_ascii_case(b_m) {
                    return Some(record.clone());
                }
            }
        }

        None
    }

    pub fn add_ban(
        &mut self,
        hwid: &str,
        username: &str,
        hostname: &str,
        ip: &str,
        reason: &str,
        hwid_details: serde_json::Value,
    ) -> BanEntry {
        let hwid_clean = hwid.trim().to_uppercase();
        let details_obj = hwid_details.as_object();
        let target_mb = extract_field(details_obj, "motherboard_uuid");

        let mut existing_id = None;
        for (id, record) in &self.bans {
            if !hwid_clean.is_empty() && record.hwid.eq_ignore_ascii_case(&hwid_clean) {
                existing_id = Some(id.clone());
                break;
            }
            if let Some(target_mb_val) = &target_mb {
                let rec_mb = extract_field(record.hwid_details.as_object(), "motherboard_uuid");
                if let Some(rec_mb_val) = rec_mb {
                    if is_valid_mb_uuid(&rec_mb_val) && rec_mb_val.eq_ignore_ascii_case(target_mb_val) {
                        existing_id = Some(id.clone());
                        break;
                    }
                }
            }
        }

        let ban_id = existing_id.unwrap_or_else(|| {
            let mut rand_bytes = [0u8; 4];
            rand::thread_rng().fill_bytes(&mut rand_bytes);
            format!("ban_{}", hex::encode(rand_bytes))
        });

        let entry = BanEntry {
            id: ban_id.clone(),
            hwid: hwid_clean,
            username: if username.trim().is_empty() { "Bilinmeyen".to_string() } else { username.trim().to_string() },
            hostname: if hostname.trim().is_empty() { "-".to_string() } else { hostname.trim().to_string() },
            ip: if ip.trim().is_empty() { "-".to_string() } else { ip.trim().to_string() },
            reason: if reason.trim().is_empty() { "Yönetici tarafından yasaklandı".to_string() } else { reason.trim().to_string() },
            banned_at: format_current_time(),
            hwid_details,
        };

        self.bans.insert(ban_id, entry.clone());
        self.save_bans();
        entry
    }

    pub fn remove_ban(&mut self, identifier: &str) -> bool {
        let clean = identifier.trim().to_uppercase();
        if clean.is_empty() {
            return false;
        }

        let mut to_remove = Vec::new();
        for (id, record) in &self.bans {
            if id.eq_ignore_ascii_case(&clean)
                || record.hwid.eq_ignore_ascii_case(&clean)
            {
                to_remove.push(id.clone());
                continue;
            }

            let rec_mb = extract_field(record.hwid_details.as_object(), "motherboard_uuid");
            if let Some(mb) = rec_mb {
                if mb.eq_ignore_ascii_case(&clean) {
                    to_remove.push(id.clone());
                }
            }
        }

        if to_remove.is_empty() {
            return false;
        }

        for id in to_remove {
            self.bans.remove(&id);
        }
        self.save_bans();
        true
    }

    pub fn list_bans(&self) -> Vec<BanEntry> {
        let mut list: Vec<BanEntry> = self.bans.values().cloned().collect();
        list.sort_by(|a, b| b.banned_at.cmp(&a.banned_at));
        list
    }
}

fn extract_field(obj: Option<&serde_json::Map<String, serde_json::Value>>, field: &str) -> Option<String> {
    let val = obj?.get(field)?;
    let s = val.as_str()?.trim();
    if s.is_empty() || s == "-" || s == "N/A" {
        None
    } else {
        Some(s.to_string())
    }
}

fn is_valid_mb_uuid(uuid: &str) -> bool {
    if uuid.len() <= 8 {
        return false;
    }
    let lower = uuid.to_lowercase();
    !matches!(
        lower.as_str(),
        "default string" | "none" | "unknown" | "to be filled by o.e.m." | "00000000-0000-0000-0000-000000000000"
    )
}

pub fn format_current_time() -> String {
    let now = SystemTime::now();
    let epoch = now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    format_timestamp(epoch)
}

pub fn format_timestamp(secs: u64) -> String {
    let sec = secs % 60;
    let min = (secs / 60) % 60;
    let hour = (secs / 3600) % 24;
    let days = secs / 86400;

    // Gregorian calculation from 1970-01-01
    let mut year = 1970;
    let mut d = days;

    loop {
        let leap = is_leap_year(year);
        let days_in_year = if leap { 366 } else { 365 };
        if d < days_in_year {
            break;
        }
        d -= days_in_year;
        year += 1;
    }

    let leap = is_leap_year(year);
    let month_days = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];

    let mut month = 1;
    for &md in &month_days {
        if d < md {
            break;
        }
        d -= md;
        month += 1;
    }
    let day = d + 1;

    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        year, month, day, hour, min, sec
    )
}

fn is_leap_year(year: u64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}
