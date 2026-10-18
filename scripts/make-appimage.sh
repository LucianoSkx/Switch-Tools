#!/usr/bin/env bash
# Empacota o binário release em um AppImage. Usa o appimagetool oficial
# (baixado uma vez para .build/); roda sem FUSE via APPIMAGE_EXTRACT_AND_RUN.
# Requer mksquashfs do squashfs-tools.
set -u

REPO="$(cd "$(dirname "$0")/.." && pwd)"
BIN="$REPO/src-tauri/target/release/sak-linux"
TOOL="$REPO/.build/appimagetool-x86_64.AppImage"
URL="https://github.com/AppImage/AppImageKit/releases/download/continuous/appimagetool-x86_64.AppImage"
OUT="$REPO/SwitchTools.AppImage"

[ -x "$BIN" ] || { echo "FAIL  appimage: binário não compilado (cargo build --release)"; exit 1; }
command -v mksquashfs >/dev/null 2>&1 || { echo "FAIL  appimage: mksquashfs ausente (pacote squashfs-tools)"; exit 1; }

mkdir -p "$REPO/.build"
if [ ! -f "$TOOL" ]; then
  curl -sL -o "$TOOL" "$URL" || { echo "FAIL  appimage: download do appimagetool"; exit 1; }
fi
chmod +x "$TOOL"

STAGE="$(mktemp -d)"
cp "$BIN" "$STAGE/sak-linux"
cp "$REPO/src-tauri/icons/128x128.png" "$STAGE/sak.png"
cat > "$STAGE/sak.desktop" <<'EOF'
[Desktop Entry]
Name=Switch Tools
Comment=Manage Nintendo Switch XCI/NSP files
Exec=sak-linux
Icon=sak
Terminal=false
Type=Application
Categories=Utility;
EOF
cat > "$STAGE/AppRun" <<'EOF'
#!/bin/sh
HERE="$(dirname "$(readlink -f "$0")")"
exec "$HERE/sak-linux"
EOF
chmod +x "$STAGE/AppRun" "$STAGE/sak-linux"

if ARCH=x86_64 APPIMAGE_EXTRACT_AND_RUN=1 "$TOOL" -n "$STAGE" "$OUT" >/dev/null 2>&1; then
  echo "OK    appimage -> SwitchTools.AppImage"
else
  echo "FAIL  appimage: appimagetool (rode o comando sem >/dev/null para ver o erro)"
  rm -rf "$STAGE"
  exit 1
fi
rm -rf "$STAGE"
