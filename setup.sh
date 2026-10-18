#!/usr/bin/env bash
# Switch Tools Linux — setup completo para usuário novo: clonou, rodou, usou.
# Checa dependências, monta bin/ + NSC_BUILDER/ (scripts/build-tools.sh), compila
# o app (cargo + eframe) e gera a pasta portátil final em SWITCH_TOOLS/.
set -u
REPO="$(cd "$(dirname "$0")" && pwd)"

echo "== 1/4  System dependencies =="
falta=""
for c in git make gcc g++ python3 curl 7z cargo; do
  command -v "$c" >/dev/null 2>&1 || falta="$falta $c"
done
if [ -n "$falta" ]; then
  echo "Missing:$falta"
  echo "Arch:  sudo pacman -S --needed git make gcc python p7zip curl rust"
  exit 1
fi

echo "== 2/4  Tools and data (bin/ and NSC_BUILDER/) =="
bash "$REPO/scripts/build-tools.sh" || { echo "FAIL: build-tools.sh"; exit 1; }

echo "== 3/4  App (cargo release) =="
(cd "$REPO/src-tauri" && cargo build --release) || { echo "FAIL: cargo build" ; exit 1; }

echo "== 4/4  AppImage + final folder =="
bash "$REPO/scripts/make-appimage.sh" || { echo "FAIL: make-appimage.sh"; exit 1; }

FINAL="$REPO/SWITCH_TOOLS"
mkdir -p "$FINAL"
cp -a "$REPO/SwitchTools.AppImage" "$FINAL/"
cp -a "$REPO/bin" "$REPO/NSC_BUILDER" "$FINAL/"

# Limpeza: tudo que está em SWITCH_TOOLS/ foi copiado; o resto é lixo de build
# (recompila do zero se precisar).
rm -rf "$REPO/src-tauri/target" "$REPO/.build" "$REPO/NSC_BUILDER" \
       "$REPO/SwitchTools.AppImage"
cd "$REPO/bin" && rm -f hactool 4nxci renxpack hacbrewpack hptnacp hactoolnet.exe \
  fw_nca_mapping.csv nro_fw* *_LICENSE

echo
echo "DONE. Self-contained portable folder (move it anywhere you want):"
echo "  $FINAL"
echo
echo "To use:"
echo "  1. Copy your keys (prod.keys or keys.dat) to: $FINAL/bin/"
echo "  2. Open:  $FINAL/SwitchTools.AppImage"
echo "  3. Results are written next to the AppImage"
