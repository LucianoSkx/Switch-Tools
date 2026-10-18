# Switch Tools — analysis (Linux port)

## What it is
Unofficial Linux port of **SAK v0.7.14** (kempa, psxtools.de) written in **Rust + egui**, with no webview. It centralizes the same flows as the original: `XCI↔NSP`, `NSZ/XCZ`, `split/merge`, `patch`, `extract FW/save`, `NRO forwarder`, `FAT32` — through native C binaries (`hactool`, `4nxci`, `renxpack`, `hacbrewpack`, `hptnacp`) and `squirrel` (Python/NSC_BUILDER).

- Folder: `/home/luciano/Projetos/Switch-Tools`
- App version: `0.7.14` (Cargo), `info_text` claims `1.0`
- Platform: Linux x86_64

## Architecture (summary)
- `src-tauri/src/lib.rs` (2076 lines) — library: `app_dir()` AppImage-aware, `bin_dir()`, helpers `keys_status()`, `free_space_mb()`, fat32, split/merge, `build_plan()` per feature, `run_external()`, `tool_run()` and tests.
- `src-tauri/src/gui.rs` (1355 lines) — egui: sidebar with 19 tools + form + log.
- `src-tauri/src/main.rs` — entry point.
- `scripts/build-tools.sh` — assembles `bin/` and `portable/` (NSC_BUILDER + venv).
- `scripts/make-appimage.sh` — packages the AppImage.
- `setup.sh` — automated pipeline.
- `SWITCH_TOOLS/` — generated final portable folder (203 MB).

## Build verification
- `cargo check`: ✅ 25.6 s, no errors.
- `cargo test`: ✅ **18 tests, 0 failures** (part_naming, free space, hptnacp handling, bin_dir perms, command plan).
- `cargo clippy` / `cargo fmt`: not installed for this toolchain.

## Bugs / inconsistencies found

| # | Severity | File | Description |
|---|---|---|---|
| 1 | P1 (build) | `setup.sh` | Copies **`Switch Army Knife.AppImage`** (old make-appimage name) which does not exist — make-appimage generates **`SwitchTools.AppImage`**. Also moves into `SAK_TOOLS/`, while the README expects `SAK_TOOLS/`/`SAK_TOOLS`. Result: setup finishes without an AppImage even if the build is OK. |
| 2 | P1 (security) | `SWITCH_TOOLS/bin/` | Contains real **`prod.keys` and `title.keys`** (console keys). Not listed in `.gitignore` (only `*.keys` ignores the names, but the whole 203 MB build artifact folder needed an explicit ignore). If the repo becomes git and is pushed, the keys leak. Mitigation applied now: added `/SWITCH_TOOLS/` to `.gitignore`. Revoke/regenerate if already used elsewhere. |
| 3 | P2 (cleanup) | `bin/` | Mixes versioned wrappers (`README.txt`, `squirrel`) with non-versioned build artifacts (`prod.keys`, `title.keys`). `.gitignore` ignores `bin/*` except README/squirrel/hactoolnet, OK, but the keys stay on local disk — check before a broad `git add`. |
| 4 | P2 (naming) | Project | Title varies: `Switch Tools`, `SAK`, `Switch Army Knife`, `sak-linux` (Cargo bin), `SwitchTools.AppImage`, `SAK_TOOLS`. Unify in README/setup/AppImage. |
| 5 | P2 (version) | `info_text` | Says `Version: 1.0`, Cargo `0.7.14`; README says "Linux port". Synchronize. |
| 6 | P3 (UX) | `lib.rs:extract-save` | Save extraction uses `hactool -t save` and a `tmp` directory; if the key is invalid it aborts silently? Check error messages in the squirrel log (via `run_external`). |
| 7 | P3 (compat) | build-tools.sh | SAK 7z from the `dezem/SAK` mirror may go offline; `hactoolnet.exe` (NativeAOT) does not run natively on Linux (comment mentions a wrapper, not implemented in the launcher). Save extraction already uses native `hactool -t save`, so OK. |
| 8 | P3 (network) | build | Pip installs NSC_BUILDER into the venv; pip warning removed and nutdb.json removed (downloaded on demand). Offline builds stop working. |
| 9 | P3 (docs) | SAK original | README points to `~prod.keys` as the only user input — also document `title.keys` if the NRO forwarder needs it. |

## Strengths
- Reproducible build (cargo, detailed scripts).
- `app_dir()` reads `APPIMAGE` and uses the relative `bin/` → truly portable.