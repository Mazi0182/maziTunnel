#!/usr/bin/env bash
set -euo pipefail

export PATH="/home/mazidev/.local/bin:$HOME/.cargo/bin:$HOME/.local/bin:$PATH"

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$ROOT_DIR"

mkdir -p dist
rm -f dist/*_*.exe dist/*_*.msi dist/*_*.json

echo "================================================================"
echo "  [1/5] Linux Sunucu Servisi Derleniyor (x86_64-unknown-linux-gnu)"
echo "================================================================"
cargo build --release -p mazitunnel-server --bin mazitunnel-server
cp target/release/mazitunnel-server dist/mazitunnel-server
chmod +x dist/mazitunnel-server

echo "================================================================"
echo "  [2/5] Windows Istemci (maziTunnel.exe) ve Uninstaller Derleniyor"
echo "================================================================"
cargo zigbuild --release --target x86_64-pc-windows-gnu -p mazitunnel-client --bin maziTunnel
cargo zigbuild --release --target x86_64-pc-windows-gnu -p mazitunnel-installer --bin mazitunnel-uninstall

CLIENT_EXE="target/x86_64-pc-windows-gnu/release/maziTunnel.exe"
UNINSTALL_EXE="target/x86_64-pc-windows-gnu/release/mazitunnel-uninstall.exe"

cp "$CLIENT_EXE" dist/maziTunnel.exe
cp "$UNINSTALL_EXE" dist/Uninstall.exe
cp "$UNINSTALL_EXE" dist/maziTunnelUninstall.exe

echo "================================================================"
echo "  [3/5] Kurulum Paketi Gomulu Verileri Sikistiriliyor (GZIP -9)"
echo "================================================================"
gzip -9 -c "$CLIENT_EXE" > crates/mazitunnel-installer/payload_client.bin
gzip -9 -c "$UNINSTALL_EXE" > crates/mazitunnel-installer/payload_uninstall.bin

echo "================================================================"
echo "  [4/5] Windows Setup Sihirbazi Derleniyor -> dist/maziTunnelSetup.exe"
echo "================================================================"
touch crates/mazitunnel-installer/src/setup_main.rs
cargo zigbuild --release --target x86_64-pc-windows-gnu -p mazitunnel-installer --bin mazitunnel-setup

SETUP_EXE="target/x86_64-pc-windows-gnu/release/mazitunnel-setup.exe"
cp "$SETUP_EXE" dist/maziTunnelSetup.exe

echo "================================================================"
echo "  [5/5] Standart MSI Kurulum Paketi Uretiliyor (wixl --ext ui) -> dist/maziTunnelSetup.msi"
echo "================================================================"
wixl -v --ext ui installer.wxs -o dist/maziTunnelSetup.msi

echo ""
echo "================================================================"
echo "  DERLEME TAMAMLANDI - CIKTI DOSYALARI (dist/)"
echo "================================================================"
ls -lh dist/
file dist/*
