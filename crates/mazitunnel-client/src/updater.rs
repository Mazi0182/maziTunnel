use anyhow::{anyhow, Context};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::process::Command;
use std::time::Duration;
use tracing::info;

pub const APP_VERSION: &str = "2.0.1";
pub const DEFAULT_UPDATE_URL: &str = "https://updates.mazidev.xyz/version.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpdateManifest {
    pub version: String,
    #[serde(alias = "download_url")]
    pub url: String,
    pub sha256: Option<String>,
    pub notes: Option<String>,
}

pub fn parse_semver(v: &str) -> Option<(u64, u64, u64)> {
    let clean = v.trim().trim_start_matches('v').trim_start_matches('V');
    let parts: Vec<&str> = clean.split('.').collect();
    if parts.is_empty() {
        return None;
    }

    let major: u64 = parts.get(0)?.parse().ok()?;
    let minor: u64 = parts.get(1).unwrap_or(&"0").split('-').next()?.parse().ok()?;
    let patch: u64 = parts.get(2).unwrap_or(&"0").split('-').next()?.parse().ok()?;

    Some((major, minor, patch))
}

pub fn is_version_newer(remote: &str, current: &str) -> bool {
    let rem = match parse_semver(remote) {
        Some(v) => v,
        None => return false,
    };
    let cur = match parse_semver(current) {
        Some(v) => v,
        None => return false,
    };

    rem > cur
}

pub async fn fetch_url_bytes(url: &str) -> anyhow::Result<Vec<u8>> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        let mut curl_cmd = Command::new("curl.exe");
        curl_cmd.creation_flags(0x08000000);
        let curl_res = curl_cmd
            .args(["-sL", "--connect-timeout", "10", "--max-time", "60", url])
            .output();

        if let Ok(output) = curl_res {
            if output.status.success() && !output.stdout.is_empty() {
                return Ok(output.stdout);
            }
        }

        let ps_cmd = format!(
            "[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12; (New-Object System.Net.WebClient).DownloadData('{}')",
            url
        );
        let mut ps_proc = Command::new("powershell.exe");
        ps_proc.creation_flags(0x08000000);
        let ps_res = ps_proc
            .args(["-NoProfile", "-NonInteractive", "-Command", &ps_cmd])
            .output();

        if let Ok(output) = ps_res {
            if output.status.success() && !output.stdout.is_empty() {
                return Ok(output.stdout);
            }
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        let curl_res = Command::new("curl")
            .args(["-sL", "--connect-timeout", "10", "--max-time", "60", url])
            .output();

        if let Ok(output) = curl_res {
            if output.status.success() && !output.stdout.is_empty() {
                return Ok(output.stdout);
            }
        }

        let wget_res = Command::new("wget")
            .args(["-qO-", "--timeout=10", url])
            .output();

        if let Ok(output) = wget_res {
            if output.status.success() && !output.stdout.is_empty() {
                return Ok(output.stdout);
            }
        }
    }

    if url.starts_with("http://") {
        return fetch_http_raw(url).await;
    }

    Err(anyhow!("URL verisi indirilemedi: {}", url))
}

async fn fetch_http_raw(url: &str) -> anyhow::Result<Vec<u8>> {
    let stripped = url.trim_start_matches("http://");
    let (host_port, path) = match stripped.find('/') {
        Some(idx) => (&stripped[..idx], &stripped[idx..]),
        None => (stripped, "/"),
    };

    let (host, port) = match host_port.find(':') {
        Some(idx) => (&host_port[..idx], host_port[idx + 1..].parse::<u16>().unwrap_or(80)),
        None => (host_port, 80),
    };

    let addr_str = format!("{}:{}", host, port);
    let mut stream = tokio::time::timeout(
        Duration::from_secs(10),
        tokio::net::TcpStream::connect(&addr_str),
    )
    .await
    .context("TCP bağlantı zaman aşımı")??;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: maziTunnel-Updater/{}\r\nConnection: close\r\n\r\n",
        path, host, APP_VERSION
    );

    stream.write_all(request.as_bytes()).await?;

    let mut response = Vec::new();
    stream.read_to_end(&mut response).await?;

    if let Some(pos) = response.windows(4).position(|w| w == b"\r\n\r\n") {
        Ok(response[pos + 4..].to_vec())
    } else {
        Ok(response)
    }
}

pub async fn check_for_updates(update_url: Option<&str>) -> anyhow::Result<Option<UpdateManifest>> {
    let url = update_url.unwrap_or(DEFAULT_UPDATE_URL);
    info!("Güncelleme kontrol ediliyor: {}", url);

    let bytes = fetch_url_bytes(url).await?;
    let text = std::str::from_utf8(&bytes).context("Güncelleme yanıtı UTF-8 formatında değil")?;

    let manifest: UpdateManifest = serde_json::from_str(text)
        .context("Güncelleme bildirimi (manifest) JSON olarak ayrıştırılamadı")?;

    if is_version_newer(&manifest.version, APP_VERSION) {
        info!(
            "Yeni güncelleme mevcut: v{} (Mevcut: v{})",
            manifest.version, APP_VERSION
        );
        Ok(Some(manifest))
    } else {
        info!("Uygulama güncel (v{})", APP_VERSION);
        Ok(None)
    }
}

pub async fn download_update_installer(
    url: &str,
    target_path: &Path,
    expected_sha256: Option<&str>,
) -> anyhow::Result<()> {
    info!("Güncelleme paketi indiriliyor: {} -> {:?}", url, target_path);

    let bytes = fetch_url_bytes(url).await?;

    if let Some(expected_hash) = expected_sha256 {
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        let actual_hash = hex::encode(hasher.finalize());

        if !actual_hash.eq_ignore_ascii_case(expected_hash.trim()) {
            return Err(anyhow!(
                "SHA256 doğrulaması başarısız! Beklenen: {}, Hesaplanan: {}",
                expected_hash,
                actual_hash
            ));
        }
        info!("SHA256 doğrulaması başarılı: {}", actual_hash);
    }

    if let Some(parent) = target_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    std::fs::write(target_path, &bytes)
        .with_context(|| format!("Güncelleme dosyası diske yazılamadı: {:?}", target_path))?;

    info!("Güncelleme paketi başarıyla kaydedildi: {:?}", target_path);
    Ok(())
}

pub fn apply_update_and_restart(installer_path: &Path) -> anyhow::Result<()> {
    if !installer_path.exists() {
        return Err(anyhow!("Kurulum dosyası bulunamadı: {:?}", installer_path));
    }

    info!("Güncelleme başlatılıyor: {:?}", installer_path);

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        let mut cmd = Command::new(installer_path);
        cmd.creation_flags(0x08000000);
        cmd.args(["--silent", "--updated"])
            .spawn()
            .with_context(|| format!("Kurulum paketi çalıştırılamadı: {:?}", installer_path))?;
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = Command::new("chmod")
            .args(["+x", installer_path.to_str().unwrap_or_default()])
            .status();

        Command::new(installer_path)
            .args(["--silent", "--updated"])
            .spawn()
            .with_context(|| format!("Kurulum paketi çalıştırılamadı: {:?}", installer_path))?;
    }

    info!("Güncelleyici başlatıldı, mevcut süreç sonlandırılıyor.");
    std::process::exit(0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_semver() {
        assert_eq!(parse_semver("1.0.0"), Some((1, 0, 0)));
        assert_eq!(parse_semver("v1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_semver("V2.10.4-beta"), Some((2, 10, 4)));
        assert_eq!(parse_semver("0.9"), Some((0, 9, 0)));
    }

    #[test]
    fn test_is_version_newer() {
        assert!(is_version_newer("1.0.1", "1.0.0"));
        assert!(is_version_newer("1.1.0", "1.0.9"));
        assert!(is_version_newer("2.0.0", "1.99.99"));
        assert!(!is_version_newer("1.0.0", "1.0.0"));
        assert!(!is_version_newer("1.0.0", "1.0.1"));
        assert!(!is_version_newer("0.9.9", "1.0.0"));
        assert!(is_version_newer("v1.0.5", "1.0.4"));
    }
}
