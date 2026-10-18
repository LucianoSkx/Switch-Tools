# Switch Tools — Linux

Unofficial Linux port of the Switch tooling set (XCI/NSP/NSZ, etc.), maintained by
luciano for Linux, written in pure Rust (egui, no webview). App, tools and data
live in a single folder, just like the original.

## Features

Convert NSP↔XCI, compress/decompress NSZ and XCZ, split, merge, apply updates and
patches, extract firmware from XCI, extract savegames, generate the NRO forwarder
NSP and format SD cards as FAT32 — the same features as Switch Tools.

## 1. Dependencies (Arch)

Copy and paste:

```sh
sudo pacman -S --needed git rust make gcc python p7zip curl squashfs-tools
```

## 2. Build

Copy and paste (single line):

```sh
git clone <url> switch-tools && cd switch-tools && ./setup.sh
```

When it finishes, the `SWITCH_TOOLS/` folder is ready inside the repository —
AppImage + `bin/` + `NSC_BUILDER/` together, self-contained and portable.
Intermediate artifacts are removed automatically; to rebuild, run `./setup.sh`
again.

## 3. Use

Copy and paste (adjust the key path if needed):

```sh
cp ~/prod.keys SWITCH_TOOLS/bin/                          # or keys.dat — the only thing you provide
./SWITCH_TOOLS/SwitchTools.AppImage
```

Results land next to the AppImage (`squirrel/`, `Firmware/`, etc.). The
`SWITCH_TOOLS/` folder can be moved anywhere (including a USB drive).

Development (run without building the AppImage): `cargo run --manifest-path
src-tauri/Cargo.toml`. Manual build, step by step: `scripts/build-tools.sh`
→ `cargo build --release` (in `src-tauri/`) → `scripts/make-appimage.sh`.

Details (source of each tool, the 34-digit key quirk, logo override): see
[`bin/README.txt`](bin/README.txt).

## Credits

Original Switch Tools: **kempa** (psxtools.de). Tools: SciresM (hactool,
hactoolnet), The-4n (4NXCI, hacBrewPack, hptnacp), julesontheroad
(NSC_BUILDER) and the mirrors' maintainers. Licenses in `bin/*_LICENSE`.