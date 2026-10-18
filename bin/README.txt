Switch Tools (Linux) — bin/ folder
======================================

The app looks ONLY in this folder, next to itself (same logic as Switch Tools
with $CUR_PATH\bin) — everything is portable. `scripts/build-tools.sh` assembles
everything here (clones sources into .build/, compiles, downloads data from the
Windows Switch Tools).

Exact names expected by the app:

  hactool      extracts FW from XCI      https://github.com/SciresM/hactool        C, make
  4nxci        converts XCI to NSP       https://github.com/tiliarou/4NXCI         C, make
  renxpack     patches NSP               https://github.com/tiliarou/reNXpack      C, make
  hacbrewpack  generates the forwarder NSP https://github.com/rlaphoenix/hacBrewPack C, make
  hptnacp      creates control.nacp      https://github.com/46620/hacpack          C++, make
               (subfolder hacPack-Tools/hacPackTools-NACP)
  (save extraction uses hactool with -t save; hactoolnet/Wine is not required)
  squirrel     update/compress/convert    wrapper for portable/venv + NSC_BUILDER
               (Python 3 + nsz, all in portable/ next to bin/).
               On first run creates the symlink keys.txt -> prod.keys/keys.dat.

The-4n deleted the account; the five C/C++ builders use faithful mirrors.

Data already included (extracted from the official Switch Tools v0.7.14 by the build script):
  fw_nca_mapping.csv, nro_fw_main, nro_fw_main.npdm, nro_fw_NintendoLogo.png,
  nro_fw_StartupMovie.gif, tool licenses.

The only missing file (user): your keys
  prod.keys or keys.dat   in bin/ (almost everything needs them)
  Note: recent packs (e.g. ProdKeys.net) ship 33 master_kek/mariko keys with an
  extra trailing "00" (34 digits); hactool rejects the file because of that
  (error "must be 32 hex digits"). If it happens, remove the trailing "00" from
  those lines — the other keys stay intact.

Optional logo override: a custom/ folder next to the app, with NintendoLogo.png
and/or StartupMovie.gif — replaces the corresponding nro_fw_* files.

Outputs (squirrel/, Firmware/, extracted_save_*, nroforwarder/) are written next
to the app — never inside the AppImage mounted under /tmp.