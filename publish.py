#!/usr/bin/env python3
"""
maziTunnel Cloudflare R2 Auto-Publisher.
Uploads maziTunnel releases (exe, msi, portable) and manifests to Cloudflare R2.
"""

import argparse
import datetime
import hashlib
import json
import os
import subprocess
import sys
import urllib.parse
import urllib.request

try:
    import boto3
    from botocore.config import Config
except ImportError:
    print("[-] Hata: 'boto3' kütüphanesi eksik! 'pip install boto3' çalıştırılıyor...")
    subprocess.check_call([sys.executable, "-m", "pip", "install", "boto3"])
    import boto3
    from botocore.config import Config

BASE_DIR = os.path.dirname(os.path.abspath(__file__))
DIST_DIR = os.path.join(BASE_DIR, "dist")

CANDIDATE_CONFIG_PATHS = [
    os.path.join(BASE_DIR, "config", "r2_config.json"),
    os.path.join(os.path.expanduser("~"), ".config", "mazitunnel", "r2_config.json"),
]


def load_r2_config() -> dict:
    for path in CANDIDATE_CONFIG_PATHS:
        if os.path.exists(path):
            with open(path, "r", encoding="utf-8") as f:
                cfg = json.load(f)
                print(f"[+] R2 yapılandırması yüklendi: {path}")
                return cfg
    print("[-] Hata: Hiçbir r2_config.json bulunamadı! Aranan yollar:")
    for path in CANDIDATE_CONFIG_PATHS:
        print(f"    - {path}")
    sys.exit(1)


def calculate_sha256(filepath: str) -> str:
    hasher = hashlib.sha256()
    with open(filepath, "rb") as f:
        while chunk := f.read(65536):
            hasher.update(chunk)
    return hasher.hexdigest().lower()


class ProgressPercentage:
    def __init__(self, filename, total_size):
        self._filename = filename
        self._size = total_size
        self._seen_so_far = 0

    def __call__(self, bytes_amount):
        self._seen_so_far += bytes_amount
        percentage = (self._seen_so_far / self._size) * 100 if self._size > 0 else 100
        mb_done = self._seen_so_far / (1024 * 1024)
        mb_total = self._size / (1024 * 1024)
        sys.stdout.write(
            f"\r[*] Yükleniyor: {self._filename:<32} | {mb_done:5.1f} MB / {mb_total:5.1f} MB (%{percentage:5.1f})"
        )
        sys.stdout.flush()
        if self._seen_so_far >= self._size:
            sys.stdout.write("\n")


def run_build():
    build_script = os.path.join(BASE_DIR, "build_release.sh")
    if not os.path.exists(build_script):
        print(f"[-] Hata: {build_script} bulunamadı!")
        sys.exit(1)
    print(f"[*] Tam derleme başlatılıyor: {build_script}...")
    subprocess.check_call(["bash", build_script], cwd=BASE_DIR)


def main():
    parser = argparse.ArgumentParser(description="maziTunnel Cloudflare R2 Publisher")
    parser.add_argument("version", nargs="?", default="2.0.0", help="Yayınlanacak sürüm (varsayılan: 2.0.0)")
    parser.add_argument(
        "notes",
        nargs="?",
        default="maziTunnel v2.0.0: Saf Rust ile yeniden yazılmış yüksek performanslı P2P tünel altyapısı, modern kurulum sihirbazı ve MSI desteği.",
        help="Güncelleme notları"
    )
    parser.add_argument("--skip-build", action="store_true", help="Derleme adımını atla, dist/ içeriğini doğrudan yükle")
    args = parser.parse_args()

    new_ver = args.version.strip()
    notes = args.notes.strip()

    cfg = load_r2_config()
    public_base = cfg.get("public_url_base", "https://updates.mazidev.xyz").rstrip("/")
    bucket = cfg["bucket_name"]

    print("\n" + "=" * 64)
    print("      maziTunnel - CLOUDFLARE R2 RELEASE PUBLISHER")
    print("=" * 64)
    print(f"  Hedef Sürüm     : v{new_ver}")
    print(f"  Güncelleme Notu : {notes}")
    print(f"  R2 Bucket       : {bucket}")
    print(f"  Yayın Adresi    : {public_base}")
    print("=" * 64 + "\n")

    if not args.skip_build:
        run_build()
    else:
        print("[*] Derleme adımı atlandı (--skip-build).")

    # Required files to publish (alt cizgi ve Client Portable olmadan temiz isimlendirme)
    expected_binaries = {
        "maziTunnelSetup.exe": "application/vnd.microsoft.portable-executable",
        "maziTunnel.exe": "application/vnd.microsoft.portable-executable",
        "maziTunnelSetup.msi": "application/x-msi",
        "maziTunnelUninstall.exe": "application/vnd.microsoft.portable-executable",
    }

    files_info = {}
    for filename in expected_binaries:
        fpath = os.path.join(DIST_DIR, filename)
        if not os.path.exists(fpath):
            print(f"[-] Hata: dist/{filename} bulunamadı!")
            sys.exit(1)
        size = os.path.getsize(fpath)
        sha = calculate_sha256(fpath)
        files_info[filename] = {
            "path": fpath,
            "size": size,
            "sha256": sha,
            "url": f"{public_base}/{urllib.parse.quote(filename)}?v={sha[:10]}"
        }
        print(f"[+] Dosya hazır: {filename:<30} ({size / (1024 * 1024):.2f} MB, sha256: {sha[:12]}...)")

    # Connect to S3 / Cloudflare R2
    print(f"\n[*] Cloudflare R2 ({bucket}) uç noktasına bağlanılıyor...")
    s3 = boto3.client(
        "s3",
        endpoint_url=cfg["endpoint_url"],
        aws_access_key_id=cfg["access_key_id"],
        aws_secret_access_key=cfg["secret_access_key"],
        config=Config(signature_version="s3v4"),
        region_name="auto"
    )

    # Eski alt cizgili dosyalari bucket'tan temizle
    legacy_keys = [
        "maziTunnel_Setup.exe",
        "maziTunnel_Client_Portable.exe",
        "maziTunnel_Setup.msi",
        "maziTunnel_Uninstall.exe",
        "mazitunnel_version.json",
    ]
    for old_key in legacy_keys:
        try:
            s3.delete_object(Bucket=bucket, Key=old_key)
        except Exception:
            pass

    # Upload Binaries
    for filename, content_type in expected_binaries.items():
        info = files_info[filename]
        print(f"[*] Yükleniyor: '{filename}' -> R2 '{bucket}/{filename}'...")
        s3.upload_file(
            info["path"],
            bucket,
            filename,
            Callback=ProgressPercentage(filename, info["size"]),
            ExtraArgs={
                "ContentType": content_type,
                "CacheControl": "no-cache, no-store, must-revalidate"
            }
        )
        print(f"[+] '{filename}' başarıyla yüklendi!")

    # Manifests preparation
    setup_exe_info = files_info["maziTunnelSetup.exe"]
    client_exe_info = files_info["maziTunnel.exe"]
    msi_info = files_info["maziTunnelSetup.msi"]
    uninstall_info = files_info["maziTunnelUninstall.exe"]

    now_iso = datetime.datetime.now(datetime.timezone.utc).isoformat()

    manifest_payload = {
        "version": new_ver,
        "url": setup_exe_info["url"],
        "downloadUrl": setup_exe_info["url"],
        "filename": "maziTunnelSetup.exe",
        "clientUrl": client_exe_info["url"],
        "msiUrl": msi_info["url"],
        "uninstallUrl": uninstall_info["url"],
        "notes": notes,
        "sha256": setup_exe_info["sha256"],
        "sha256Setup": setup_exe_info["sha256"],
        "sha256Client": client_exe_info["sha256"],
        "sha256Msi": msi_info["sha256"],
        "fileSize": setup_exe_info["size"],
        "publishedAt": now_iso
    }

    manifest_files = ["version.json"]
    for mfile in manifest_files:
        mpath = os.path.join(DIST_DIR, mfile)
        with open(mpath, "w", encoding="utf-8") as f:
            json.dump(manifest_payload, f, indent=2, ensure_ascii=False)
        print(f"[+] Yerel manifest oluşturuldu: {mpath}")

        print(f"[*] Manifest R2'ye yükleniyor: {bucket}/{mfile}...")
        s3.upload_file(
            mpath,
            bucket,
            mfile,
            ExtraArgs={
                "ContentType": "application/json",
                "CacheControl": "no-cache, no-store, must-revalidate"
            }
        )
        print(f"[+] '{mfile}' başarıyla yüklendi!")

    # Live Healthchecks
    print("\n[*] Canlı R2 uç noktaları kontrol ediliyor...")
    for mfile in manifest_files:
        check_url = f"{public_base}/{mfile}"
        try:
            req = urllib.request.Request(
                check_url,
                headers={"User-Agent": "maziTunnel-Publisher-Check/2.0", "Cache-Control": "no-cache"}
            )
            with urllib.request.urlopen(req, timeout=10.0) as resp:
                if resp.status == 200:
                    data = json.loads(resp.read().decode("utf-8"))
                    print(f"  [+] DOĞRULANDI: {check_url} -> Sürüm: {data.get('version')}, SHA: {data.get('sha256', '')[:10]}...")
                else:
                    print(f"  [-] Beklenmeyen durum kodu ({check_url}): {resp.status}")
        except Exception as e:
            print(f"  [!] Uyarı: Canlı kontrol yanıt vermedi ({check_url}): {e}")

    print("\n" + "=" * 64)
    print(f"  [+] maziTunnel v{new_ver} YAYINI BASARIYLA TAMAMLANDI!")
    print(f"  - Setup Sihirbazı : {setup_exe_info['url']}")
    print(f"  - İstemci (Client): {client_exe_info['url']}")
    print(f"  - MSI Paketi      : {msi_info['url']}")
    print(f"  - Uninstaller     : {uninstall_info['url']}")
    print(f"  - Manifest        : {public_base}/version.json")
    print("=" * 64 + "\n")


if __name__ == "__main__":
    main()
