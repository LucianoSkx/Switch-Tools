# Switch Tools — Linux

Unofficial Linux port of [SAK (Switch Army Knife) v0.7.14]((https://github.com/dezem/SAK)) by dezem (archived upstream, read-only since 2023).
Same workflow as the original — app, helpers and data live in one portable folder —
rewritten in pure Rust (`egui`, no webview, no Electron).

![Build AppImage](https://github.com/LucianoSkx/Switch-Tools/actions/workflows/build-appimage.yml/badge.svg)

## Features

Same 18 tools as SAK v0.7.14, grouped the same way in the sidebar:

| Group | Tool | What it does |
|-------|------|--------------|
| XCI | XCI to NSP | Convert XCI to NSP (4NXCI), optional FAT32 split after convert |
| XCI | XCI to XCZ | Compress XCI to XCZ (zstd, levels FAST / INTERMEDIATE / AVERAGE / HARD) |
| XCI | XCZ to XCI | Decompress XCZ back to XCI |
| XCI | XCI split | Split XCI into < 4 GiB parts for FAT32 (optional 0xFF trim) |
| XCI | XCI merge | Merge `.xc0` / `.xci.00` parts back into one XCI |
| XCI | Update XCI | Apply update and/or DLC NSP/NSZ onto a base XCI |
| XCI | Patch XCI | Patch XCI to run on lower firmware |
| XCI | Extract FW | Extract firmware from XCI, renamed to `Firmware X.X.X` via `fw_nca_mapping.csv` |
| NSP | NSP to XCI | Convert NSP to XCI |
| NSP | NSP to NSZ | Compress NSP to NSZ (zstd, same 4 levels) |
| NSP | NSZ to NSP | Decompress NSZ back to NSP |
| NSP | NSP split | Split NSP into < 4 GiB parts (folder with `00`, `01`, …) |
| NSP | NSP merge | Merge split NSP folder back into one NSP |
| NSP | Update NSP | Apply update and/or DLC NSP/NSZ onto a base NSP |
| NSP | Patch NSP | Patch NSP to run on lower firmware (reNXpack) |
| MISC | NRO forwarder | Build a forwarder NSP for homebrew NRO or RetroArch core (custom Title ID, icon, version) |
| MISC | Extract Save | Extract saves from an eMMC / User partition dump |
| MISC | Fat32 Format | Format a USB device to FAT32 (any size, uses `mkfs.vfat`; **erases everything**) |

Every operation shows live progress, a Cancel button, and an activity log
(mirrored to `SwitchTools.log` next to the AppImage).

## Quick start

**Option A — download the release (no compile):**

1. Grab `SwitchTools-linux-x86_64.tar.gz` from the
   [Releases](https://github.com/LucianoSkx/Switch-Tools/releases).
2. Extract it — you get a `SWITCH_TOOLS/` folder.
3. Drop your keys in (see [Keys](#keys)), run the AppImage:

```sh
tar -xzf SwitchTools-linux-x86_64.tar.gz
cp ~/prod.keys SWITCH_TOOLS/bin/   # or keys.dat
./SWITCH_TOOLS/SwitchTools.AppImage
```

**Option B — build from source (one command):**

```sh
git clone https://github.com/LucianoSkx/Switch-Tools.git switch-tools
cd switch-tools
./setup.sh
```

When it finishes, `SWITCH_TOOLS/` inside the repo holds the AppImage + `bin/` +
`NSC_BUILDER/` — self-contained, movable anywhere (including a USB stick).
Intermediate build dirs (`.build/`, `src-tauri/target/`) are removed automatically;
just run `./setup.sh` again to rebuild.

## Requirements

Build-time tools checked by `setup.sh`: `git make gcc g++ python3 curl 7z cargo`,
plus `mksquashfs` (AppImage packing). Runtime for FAT32 format:
`pkexec` (polkit) + `mkfs.vfat` (dosfstools).

Arch:

```sh
sudo pacman -S --needed git make gcc python p7zip curl rust squashfs-tools dosfstools polkit
```

Debian / Ubuntu (same set as CI):

```sh
sudo apt-get install -y build-essential pkg-config curl git make python3 python3-venv \
  p7zip-full squashfs-tools dosfstools policykit-1 \
  libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev \
  libxkbcommon-dev libssl-dev libgtk-3-dev \
  libgl1-mesa-dev libegl1-mesa-dev libwayland-dev
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Other distros: install the equivalents of the packages above.

## Usage

### Keys

The only file you must provide yourself. Copy **one** of them into `bin/`:

```sh
cp ~/prod.keys SWITCH_TOOLS/bin/   # or keys.dat
```

> **34-digit key quirk:** some key packs ship `master_kek` / mariko keys with a
> trailing `00` (34 hex digits). hactool rejects the whole file with
> `must be 32 hex digits`. Open the file and delete the trailing `00` on those
> lines — leave everything else untouched.

### Running

```sh
./SWITCH_TOOLS/SwitchTools.AppImage
```

Dev run without packaging:

```sh
cargo run --manifest-path src-tauri/Cargo.toml
```

Step-by-step manual build (what `setup.sh` does):

```sh
scripts/build-tools.sh          # compiles helpers into bin/, prepares NSC_BUILDER/
cargo build --release --manifest-path src-tauri/Cargo.toml
scripts/make-appimage.sh
```

### Where results go

Outputs are written **next to the AppImage, never inside it**:
`squirrel/`, `Firmware X.X.X/`, `renxpack_extracted_nsp/`, `nroforwarder/`,
`extracted_save_*`, `SwitchTools.log`. Split/merge of XCI writes next to the
parts by default, or to the picked USB stick.

### Portable folder layout

```text
SWITCH_TOOLS/
  SwitchTools.AppImage   # the egui app
  bin/                   # native helpers: hactool, 4nxci, renxpack,
                         # hacbrewpack, hptnacp, squirrel wrapper + data
                         # (fw_nca_mapping.csv, nro_fw_*, licenses) + your keys
  NSC_BUILDER/           # squirrel (ztools) + portable venv with nsz/zstd stack
```

Optional logo override: a `custom/` folder next to the AppImage with
`NintendoLogo.png` and/or `StartupMovie.gif` replaces the bundled forwarder assets.

## How it works

- The GUI (`src-tauri/src/`, Rust + eframe) reproduces SAK's feature set and
  messages 1:1, then shells out to the helper in `bin/` with the same argv SAK used.
- `scripts/build-tools.sh` compiles the five C/C++ helpers from source
  (hactool pinned to `3121a5b`, the SAK-era revision; The-4n's repos via faithful
  mirrors), extracts the data files from official SAK v0.7.14, and builds a
  portable `NSC_BUILDER/py/venv` with `nsz` + patches for Python 3.11+
  (`IntEnum.__str__`, `cls` → `clear`, no 87 MB DB auto-download).
- `scripts/make-appimage.sh` packs the release binary with the official
  `appimagetool` (runs FUSE-less via `APPIMAGE_EXTRACT_AND_RUN=1`).
- Full per-tool source URLs, binary names and quirks: [`bin/README.txt`](bin/README.txt).

## Troubleshooting

| Symptom | Fix |
|---------|-----|
| `Missing: …` from `setup.sh` | Install the [Requirements](#requirements) for your distro |
| `FAIL appimage: mksquashfs missing` | `squashfs-tools` (Arch) / `squashfs-tools` (apt) |
| `failed to run bin/squirrel` / pip errors | Network needed once for the venv; delete `NSC_BUILDER/py/venv` and re-run `setup.sh` |
| `must be 32 hex digits` | [34-digit key quirk](#keys): strip the trailing `00` |
| FAT32 option greyed / `mkfs.vfat` fails | Install `dosfstools` + polkit; close anything using the device before formatting |
| `Free space is to low` | SAK's own gate: needs ~2× the input size + 100 MB free |

Build logs for helpers: `/tmp/sak-build-<name>.log`. App log: `SwitchTools.log`
next to the AppImage.

## Credits

Original SAK (Switch Army Knife): **dezem** (https://github.com/dezem/SAK — archived, read-only since Jul 2023).
Helpers: SciresM (hactool), The-4n (4NXCI, reNXpack), rlaphoenix (hacBrewPack),
46620/hacpack (hptnacp), julesontheroad (NSC_BUILDER), `nsz` maintainers, and the
mirror maintainers. Per-tool licenses in `bin/*_LICENSE`.

Unofficial port, maintained by luciano for Linux. You'll need your own legally
dumped keys, firmware and games — none are included in this repo.
