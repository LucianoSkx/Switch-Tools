#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Mutex};

const SPLIT_BYTES: u64 = 4294901760; // 4 GiB − 1 (Switch Tools: $SPLITBYTES)
const READ_BYTES: usize = 32768; // Switch Tools: $READBYTES
const MIN_FREE_MB: u64 = 100; // Switch Tools adds +100MB to the size check

static CANCELLED: AtomicBool = AtomicBool::new(false);
static ACTIVE: Mutex<Option<Child>> = Mutex::new(None);

/// Progress sink: `(pct, state text)` pairs drained by the GUI event loop.
pub type ProgressTx = mpsc::Sender<(u32, String)>;

pub mod gui;

// ---------- app dir resolution (AppImage-aware) ----------

fn app_dir() -> PathBuf {
    if let Ok(p) = std::env::var("APPIMAGE") {
        if let Some(parent) = Path::new(&p).parent() {
            return parent.to_path_buf();
        }
    }
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn bin_dir() -> PathBuf {
    app_dir().join("bin")
}

fn find_tool(name: &str) -> Option<PathBuf> {
    let direct = bin_dir().join(name);
    if direct.is_file() {
        return Some(direct);
    }
    None
}

// ---------- activity log ----------

static LOG_LOCK: Mutex<()> = Mutex::new(());

/// Timestamped `[YYYY-MM-DD HH:MM:SS]` prefix, following libc's local time the
/// same way `local_stamp` does.
fn now_stamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as libc::time_t)
        .unwrap_or(0);
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&secs, &mut tm);
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            tm.tm_year + 1900,
            tm.tm_mon + 1,
            tm.tm_mday,
            tm.tm_hour,
            tm.tm_min,
            tm.tm_sec
        )
    }
}

/// Append one timestamped line to SwitchTools.log next to the executable.
/// Logging must never break an operation, so any I/O error is swallowed.
pub fn app_log(msg: &str) {
    if msg.is_empty() {
        return;
    }
    let _g = LOG_LOCK.lock();
    let path = app_dir().join("SwitchTools.log");
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(&path) {
        for line in msg.lines() {
            let _ = writeln!(f, "[{}] {}", now_stamp(), line.trim());
        }
    }
}

/// Clear the log: each operation starts its own fresh log.
pub fn app_log_reset() {
    let _g = LOG_LOCK.lock();
    let _ = fs::write(app_dir().join("SwitchTools.log"), b"");
}

// ---------- keys ----------

#[derive(Serialize)]
pub struct KeysStatus {
    pub found: bool,
    pub path: Option<String>,
}

pub fn keys_status() -> KeysStatus {
    for name in ["prod.keys", "keys.dat"] {
        let p = bin_dir().join(name);
        if p.is_file() {
            return KeysStatus { found: true, path: Some(p.to_string_lossy().into()) };
        }
    }
    KeysStatus { found: false, path: None }
}

// ---------- free space (statvfs) ----------

fn free_space_mb(path: &Path) -> u64 {
    let c_path = match std::ffi::CString::new(path.to_string_lossy().as_bytes()) {
        Ok(c) => c,
        Err(_) => return 0,
    };
    unsafe {
        let mut s: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(c_path.as_ptr(), &mut s) != 0 {
            return 0;
        }
        (s.f_bavail as u64 * s.f_frsize as u64) / (1024 * 1024)
    }
}



// ---------- USB devices ----------

#[derive(Serialize)]
pub struct UsbDevice {
    pub mountpoint: String,
    pub label: String,
    pub free_mb: u64,
}

pub fn usb_devices() -> Vec<UsbDevice> {
    let mut out = Vec::new();
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Ok(user) = std::env::var("USER") {
        roots.push(PathBuf::from(format!("/run/media/{}", user)));
    }
    roots.push(PathBuf::from("/media"));
    roots.push(PathBuf::from("/mnt"));
    for root in roots {
        if let Ok(entries) = fs::read_dir(&root) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    let label = p
                        .file_name()
                        .map(|n| n.to_string_lossy().into())
                        .unwrap_or_else(|| p.to_string_lossy().into());
                    let free_mb = free_space_mb(&p);
                    if free_mb > 0 {
                        out.push(UsbDevice {
                            mountpoint: p.to_string_lossy().into(),
                            label,
                            free_mb,
                        });
                    }
                }
            }
        }
    }
    out
}

// ---------- FAT32 format (Switch Tools launches the guiformat.exe GUI) ----------

#[derive(Deserialize)]
pub struct Fat32Request {
    pub mountpoint: String,
    pub label: String,
}

/// Device node behind a mountpoint, from /proc/mounts (`\040` = space).
fn device_for_mountpoint(mountpoint: &str) -> Option<String> {
    let mounts = fs::read_to_string("/proc/mounts").ok()?;
    for line in mounts.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() >= 2 && cols[1].replace("\\040", " ") == mountpoint {
            return Some(cols[0].to_string());
        }
    }
    None
}

pub fn fat32_format(req: Fat32Request) -> OpResult {
    let device = match device_for_mountpoint(&req.mountpoint) {
        Some(d) => d,
        None => {
            return OpResult {
                ok: false,
                messages: vec![format!(
                    "Formatting for {} aborted\nCouldn't find {} mounted\n",
                    req.label, req.mountpoint
                )],
            }
        }
    };

    // One polkit prompt: unmount first (exit 10 = busy), then format.
    let out = Command::new("pkexec")
        .arg("sh")
        .arg("-c")
        .arg("umount \"$1\" || exit 10; mkfs.vfat -F 32 \"$1\"")
        .arg("sh")
        .arg(&device)
        .output();

    let out = match out {
        Ok(o) => o,
        Err(e) => {
            return OpResult {
                ok: false,
                messages: vec![format!("Formatting for {} failed\n{}\n", req.label, e)],
            }
        }
    };
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    match out.status.code() {
        Some(0) => {
            let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let mut messages = vec![format!("Formatting for {} finished\n", req.label)];
            if !stdout.is_empty() {
                messages.push(stdout + "\n");
            }
            OpResult { ok: true, messages }
        }
        Some(10) => OpResult {
            ok: false,
            messages: vec![format!(
                "Formatting for {} aborted\nCouldn't unmount {}\n{}\nClose all programs using it and try again.\n",
                req.label, device, stderr
            )],
        },
        _ => OpResult {
            ok: false,
            messages: vec![format!("Formatting for {} failed\n{}\n", req.label, stderr)],
        },
    }
}

// ---------- naming helpers (AutoIt StringReplace is case-insensitive) ----------

fn replace_ci(hay: &str, needle: &str, rep: &str) -> String {
    let hl = hay.to_lowercase();
    let nl = needle.to_lowercase();
    match hl.find(&nl) {
        Some(i) => format!("{}{}{}", &hay[..i], rep, &hay[i + nl.len()..]),
        None => hay.to_string(),
    }
}

// ---------- split ----------

#[derive(Deserialize)]
pub struct SplitRequest {
    /// Selected file(s). For XCI: the .xci files. For NSP: the .nsp files.
    pub inputs: Vec<String>,
    /// Destination directory (USB path, or Switch Tools' own dir when empty — Switch Tools: @ScriptDir).
    pub dest: Option<String>,
    /// XCI only: trim empty (0xFF) space from parts after the first.
    pub trim: bool,
}

#[derive(Serialize)]
pub struct OpResult {
    pub ok: bool,
    pub messages: Vec<String>,
}

fn chunk_is_empty_ff(chunk: &[u8]) -> bool {
    // Switch Tools: StringInStr($READDATA, "0xFFFFFFFFFFFFFFFFFFFF") — 10+ consecutive 0xFF bytes.
    let mut run = 0usize;
    for &b in chunk {
        if b == 0xFF {
            run += 1;
            if run >= 10 {
                return true;
            }
        } else {
            run = 0;
        }
    }
    false
}

fn fmt_size(mb: f64) -> String {
    if mb >= 1000.0 {
        format!("{:.2}GB", mb / 1024.0)
    } else {
        format!("{:.2}MB", mb)
    }
}

fn emit_progress(tx: &ProgressTx, pct: u32, state: String) {
    // Skip tqdm-style bar lines: log only meaningful events.
    if !state.is_empty() && !state.contains('%') {
        app_log(&format!("INFO {state}"));
    }
    let _ = tx.send((pct, state));
}

fn check_cancel() -> bool {
    CANCELLED.load(Ordering::SeqCst)
}

// Switch Tools: $SPLITCOUNTER = $SPLITBYTES / $READBYTES (integer division, 131071).
const CHUNKS_PER_PART: usize = (SPLIT_BYTES / READ_BYTES as u64) as usize;

#[derive(Deserialize)]
pub struct MergeRequest {
    /// The selected part file (`.xc0`, `.xci.00`, or a bare `00`).
    pub input: String,
    /// `xci` or `nsp` — decides the naming chain and output extension.
    pub kind: String,
}

fn cleanup_parts(parts: &[PathBuf]) {
    for p in parts {
        let _ = fs::remove_file(p);
    }
}

fn split_xci(
    tx: &ProgressTx,
    src: &Path,
    name: &str,
    dest_dir: &Path,
    size: u64,
    trim: bool,
    state_prefix: &str,
) -> String {
    // Switch Tools: StringReplace($XCIFILENAME, ".xci", "_splitted.xc") — case preserved, no space swap.
    let stem = replace_ci(name, ".xci", "_splitted.xc");
    let chunks_total = size.div_ceil(READ_BYTES as u64);
    let mut chunk_done: u64 = 0;

    let mut input = match File::open(src) {
        Ok(f) => f,
        Err(e) => return format!("Splitting for {} aborted\n{}\n", name, e),
    };

    let mut created: Vec<PathBuf> = Vec::new();
    let mut part_idx: u64 = 0;
    let mut canceled = false;

    'outer: loop {
        let part_path = dest_dir.join(format!("{}{}", stem, part_idx));
        let mut out = match File::create(&part_path) {
            Ok(f) => f,
            Err(e) => {
                cleanup_parts(&created);
                return format!("Splitting for {} aborted\n{}\n", name, e);
            }
        };
        created.push(part_path.clone());

        let mut eof = false;
        for _ in 0..CHUNKS_PER_PART {
            let mut buf = vec![0u8; READ_BYTES];
            let n = match input.read(&mut buf) {
                Ok(0) => {
                    eof = true;
                    break;
                }
                Ok(n) => n,
                Err(e) => {
                    cleanup_parts(&created);
                    return format!("Splitting for {} aborted\n{}\n", name, e);
                }
            };
            buf.truncate(n);
            chunk_done += 1;

            // Switch Tools trim: only parts after the first skip chunks holding 10+ consecutive 0xFF bytes.
            let write = !(trim && part_idx > 0 && chunk_is_empty_ff(&buf));
            if write {
                if let Err(e) = out.write_all(&buf) {
                    cleanup_parts(&created);
                    return format!("Splitting for {} aborted\n{}\n", name, e);
                }
            }

            let pct = ((chunk_done * 100) / chunks_total.max(1)) as u32;
            emit_progress(tx, pct, format!("State: {} is running... {}%", state_prefix, pct));
            if check_cancel() {
                canceled = true;
                break 'outer;
            }
        }
        drop(out);

        part_idx += 1;
        if canceled || eof {
            break;
        }
    }

    if canceled {
        cleanup_parts(&created);
        return format!("User canceled splitting for {}\n", name);
    }

    format!("Splitting for {} finished\nSplitted up to {} parts\n", name, part_idx)
}

fn split_nsp(
    tx: &ProgressTx,
    src: &Path,
    name: &str,
    dest_dir: &Path,
    size: u64,
    state_prefix: &str,
) -> String {
    // Switch Tools: NSP folder name swaps spaces for underscores, then ".nsp" -> "_splitted.nsp".
    let spaced = name.replace(' ', "_");
    let stem = replace_ci(&spaced, ".nsp", "_splitted.nsp");
    let folder = dest_dir.join(&stem);
    if folder.exists() {
        if let Err(e) = fs::remove_dir_all(&folder) {
            return format!("Splitting for {} aborted\n{}\n", name, e);
        }
    }
    if let Err(e) = fs::create_dir_all(&folder) {
        return format!("Splitting for {} aborted\n{}\n", name, e);
    }

    let chunks_total = size.div_ceil(READ_BYTES as u64);
    let mut chunk_done: u64 = 0;

    let mut input = match File::open(src) {
        Ok(f) => f,
        Err(e) => return format!("Splitting for {} aborted\n{}\n", name, e),
    };

    let mut part_idx: u64 = 0;
    let mut canceled = false;

    'outer: loop {
        // Switch Tools: "0" + count while < 10, bare count from 10 up.
        let part_name = if part_idx < 10 {
            format!("0{}", part_idx)
        } else {
            format!("{}", part_idx)
        };
        let part_path = folder.join(&part_name);
        let mut out = match File::create(&part_path) {
            Ok(f) => f,
            Err(e) => {
                let _ = fs::remove_dir_all(&folder);
                return format!("Splitting for {} aborted\n{}\n", name, e);
            }
        };

        let mut eof = false;
        for _ in 0..CHUNKS_PER_PART {
            let mut buf = vec![0u8; READ_BYTES];
            let n = match input.read(&mut buf) {
                Ok(0) => {
                    eof = true;
                    break;
                }
                Ok(n) => n,
                Err(e) => {
                    let _ = fs::remove_dir_all(&folder);
                    return format!("Splitting for {} aborted\n{}\n", name, e);
                }
            };
            buf.truncate(n);
            chunk_done += 1;

            if let Err(e) = out.write_all(&buf) {
                let _ = fs::remove_dir_all(&folder);
                return format!("Splitting for {} aborted\n{}\n", name, e);
            }

            let pct = ((chunk_done * 100) / chunks_total.max(1)) as u32;
            emit_progress(tx, pct, format!("State: {} is running... {}%", state_prefix, pct));
            if check_cancel() {
                canceled = true;
                break 'outer;
            }
        }
        drop(out);

        part_idx += 1;
        if canceled || eof {
            break;
        }
    }

    if canceled {
        let _ = fs::remove_dir_all(&folder);
        return format!("User canceled splitting for {}\n", name);
    }

    format!("Splitting for {} finished\nSplitted up to {} parts\n", name, part_idx)
}

/// Per-file split core, shared by the split dialogs and split-after-convert.
fn split_one(
    tx: &ProgressTx,
    input: &str,
    dest: Option<String>,
    trim: bool,
    state_prefix: &str,
) -> String {
    let src = Path::new(input);
    let name: String = src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let lower = name.to_lowercase();
    let is_xci = lower.ends_with(".xci");
    let size = match fs::metadata(src) {
        Ok(m) => m.len(),
        Err(_) => return format!("Splitting for {} aborted\nCouldn't read file\n", name),
    };

    // Switch Tools: without USB the parts land in @ScriptDir, not next to the input.
    let dest_dir = dest.map(PathBuf::from).unwrap_or_else(app_dir);

    // Switch Tools free-space precheck: Round(size/1MB, 2) + 100 vs DriveSpaceFree.
    let needed_mb = ((size as f64 / 1_048_576.0) * 100.0).round() / 100.0 + MIN_FREE_MB as f64;
    let free_mb = free_space_mb(&dest_dir) as f64;
    if free_mb < needed_mb {
        return format!(
            "Splitting for {} aborted\nFree space is to low\nOnly {:.2}GB free\nNeeded min size: {}\n",
            name,
            free_mb / 1024.0,
            fmt_size(needed_mb)
        );
    }

    // Switch Tools: files smaller than 4GB are rejected.
    if size < SPLIT_BYTES {
        return format!(
            "Splitting for {} aborted\nFileSize is lower than 4GB - Size is: {}\n",
            name,
            fmt_size(size as f64 / 1_048_576.0)
        );
    }

    if is_xci {
        split_xci(tx, src, &name, &dest_dir, size, trim, state_prefix)
    } else {
        split_nsp(tx, src, &name, &dest_dir, size, state_prefix)
    }
}

pub fn split_files(tx: &ProgressTx, req: SplitRequest) -> OpResult {
    CANCELLED.store(false, Ordering::SeqCst);
    let mut messages = Vec::new();
    let total = req.inputs.len();

    for (i, input) in req.inputs.iter().enumerate() {
        messages.push(split_one(
            tx,
            input,
            req.dest.clone(),
            req.trim,
            &format!("{}/{}", i + 1, total),
        ));
        if check_cancel() {
            break;
        }
    }

    OpResult { ok: !messages.is_empty(), messages }
}

/// Switch Tools `_MERGEXCI` / `_MERGENSP`. Picks the part-naming scheme from the selected file, walks
/// consecutive parts, and streams them into one output.
///
/// Linux adaptation: Switch Tools writes the merged file to `@ScriptDir`; we write next to the parts
/// (the AppImage dir is not writable).
pub fn merge_files(tx: &ProgressTx, req: MergeRequest) -> OpResult {
    CANCELLED.store(false, Ordering::SeqCst);
    let first = PathBuf::from(&req.input);
    let dir = first.parent().unwrap_or(Path::new(".")).to_path_buf();
    let first_name: String = first.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let lower = first_name.to_lowercase();

    // Part naming scheme + display name, straight from Switch Tools' If/ElseIf chain.
    let (mut display, base, padded, out_dir): (String, String, bool, PathBuf) =
        if lower.contains("xc0") {
        // `.xc0` -> parts `X.xc1`, `X.xc2`, ... (no zero padding)
        let base = replace_ci(&first_name, ".xc0", ".xc");
        (first_name.clone(), base, false, dir.clone())
    } else if lower.contains("xci.00") {
        // `.xci.00` -> parts `X.xci.01` .. `X.xci.09`, then `.xci.10`, ...
        let base = replace_ci(&first_name, ".xci.00", ".xci.");
        (first_name.clone(), base, true, dir.clone())
    } else if first_name == "00" {
        // Bare `00` inside a folder -> parts `01`, `02`, ... ; display is the folder name.
        let folder = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        (folder, String::new(), true, dir.clone())
    } else {
        return OpResult {
            ok: false,
            messages: vec![format!(
                "Merging for {} aborted\nUnknown part naming (expected .xc0, .xci.00 or 00)\n",
                first_name
            )],
        };
    };

    let is_nsp = req.kind.eq_ignore_ascii_case("nsp");

    // Walk consecutive parts starting at 1. Switch Tools checks bare names in @WorkingDir for the `00`
    // scheme (a bug); we resolve next to the selected file instead.
    let mut parts: Vec<PathBuf> = vec![first.clone()];
    let mut idx: u64 = 1;
    loop {
        let candidate = if base.is_empty() {
            dir.join(part_name(idx, padded))
        } else {
            dir.join(format!("{}{}", base, if padded && idx < 10 {
                format!("0{}", idx)
            } else {
                idx.to_string()
            }))
        };
        if candidate.is_file() {
            parts.push(candidate);
            idx += 1;
        } else {
            break;
        }
    }

    // NSP parts live inside `<name>_splitted.nsp/`; the merged file belongs one level up,
    // where the original `.nsp` was.
    let out_dir = if is_nsp {
        dir.parent().unwrap_or(Path::new(".")).to_path_buf()
    } else {
        out_dir
    };

    let total_size: u64 = parts.iter().filter_map(|p| fs::metadata(p).ok()).map(|m| m.len()).sum();

    // Switch Tools free-space precheck against the output dir.
    let needed_mb = (total_size as f64 / 1_048_576.0).round() + MIN_FREE_MB as f64;
    let free_mb = free_space_mb(&out_dir) as f64;
    if free_mb < needed_mb {
        let msg = format!(
            "Merging for {} aborted\nFree space is to low\nOnly {:.2}GB free\nNeeded min size: {}\n",
            display,
            free_mb / 1024.0,
            fmt_size(needed_mb)
        );
        return OpResult { ok: false, messages: vec![msg] };
    }

    // Switch Tools does nothing (and says nothing) when only one part was found.
    if parts.len() < 2 {
        return OpResult { ok: true, messages: Vec::new() };
    }

    // Switch Tools output naming: three sequential replacements, then the `_merged_merged` fixup.
    let out_name = if is_nsp {
        let n = replace_ci(&display, ".nsp", "_merged.nsp");
        let n = n.replace("splitted_", "");
        if !n.to_lowercase().contains(".nsp") {
            format!("{}_merged.nsp", n)
        } else {
            n
        }
    } else {
        let n = replace_ci(&first_name, ".xc0", "_merged.xci");
        let n = replace_ci(&n, ".xci.00", "_merged.xci");
        let n = replace_ci(&n, ".xci", "_merged.xci");
        n.replace("_merged_merged", "_merged")
    };
    display = if is_nsp {
        dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| display.clone())
    } else {
        first_name.clone()
    };

    let out_path = out_dir.join(&out_name);
    if out_path.exists() {
        let _ = fs::remove_file(&out_path);
    }

    let mut writer = match File::create(&out_path) {
        Ok(f) => f,
        Err(e) => return OpResult {
            ok: false,
            messages: vec![format!("Merging for {} aborted\n{}\n", display, e)],
        },
    };

    let chunks_total = total_size.div_ceil(READ_BYTES as u64);
    let mut chunk_done: u64 = 0;
    let mut canceled = false;

    'outer: for part in &parts {
        let mut input = match File::open(part) {
            Ok(f) => f,
            Err(e) => {
                let _ = fs::remove_file(&out_path);
                return OpResult {
                    ok: false,
                    messages: vec![format!("Merging for {} aborted\n{}\n", display, e)],
                };
            }
        };
        loop {
            let mut buf = vec![0u8; READ_BYTES];
            let n = match input.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) => {
                    let _ = fs::remove_file(&out_path);
                    return OpResult {
                        ok: false,
                        messages: vec![format!("Merging for {} aborted\n{}\n", display, e)],
                    };
                }
            };
            chunk_done += 1;
            if let Err(e) = writer.write_all(&buf[..n]) {
                let _ = fs::remove_file(&out_path);
                return OpResult {
                    ok: false,
                    messages: vec![format!("Merging for {} aborted\n{}\n", display, e)],
                };
            }
            let pct = ((chunk_done * 100) / chunks_total.max(1)) as u32;
            emit_progress(tx, pct, format!("State: is running... {}%", pct));
            if check_cancel() {
                canceled = true;
                break 'outer;
            }
        }
    }
    drop(writer);

    if canceled {
        let _ = fs::remove_file(&out_path);
        return OpResult {
            ok: false,
            messages: vec![format!("User canceled merging for {}\n", display)],
        };
    }

    let msg = format!("Merging for {} finished\nMerged {} parts\n", display, parts.len());
    OpResult { ok: true, messages: vec![msg] }
}

fn part_name(idx: u64, padded: bool) -> String {
    if padded && idx < 10 {
        format!("0{}", idx)
    } else {
        idx.to_string()
    }
}

pub fn op_cancel() {
    CANCELLED.store(true, Ordering::SeqCst);
    if let Ok(mut active) = ACTIVE.lock() {
        if let Some(child) = active.as_mut() {
            let _ = child.kill();
        }
    }
}

// ---------- TitleID generator (Switch Tools: GENERATETITLEID) ----------

pub fn generate_titleid() -> String {
    const CHARS: [char; 16] = ['A', 'B', 'C', 'D', 'E', 'F', '1', '2', '3', '4', '5', '6', '7', '8', '9', '0'];
    let mut s = String::from("01");
    for _ in 0..11 {
        let i = rand_index(16);
        s.push(CHARS[i]);
    }
    s.push_str("000");
    s
}

fn rand_index(n: usize) -> usize {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    (nanos as usize) % n
}

// ---------- external tools (plan → run bin/<tool> → discover the output, like SAK) ----------

#[derive(Deserialize)]
pub struct ToolRequest {
    pub feature: String,
    /// Paths selected in the dialog, in Switch Tools' order.
    pub inputs: Vec<String>,
    /// Extra options (compression level, title id, app name, …).
    pub opts: serde_json::Value,
}

#[derive(Serialize, Debug)]
pub struct ToolRun {
    pub missing: bool,
    pub tool: String,
    pub command: String,
    pub messages: Vec<String>,
}

/// One helper invocation: exact argv, the working directory Switch Tools uses for the
/// feature (`cd /D …` before the command), and the list.txt content SAK
/// pre-writes for squirrel's `-tfile`.
struct Planned {
    tool: String,
    argv: Vec<String>,
    cwd: PathBuf,
    list: Option<Vec<String>>,
}

fn ps(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

fn sv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| s.to_string()).collect()
}

fn keys_path() -> PathBuf {
    let prod = bin_dir().join("prod.keys");
    if prod.is_file() {
        prod
    } else {
        bin_dir().join("keys.dat")
    }
}

fn feature_verb(feature: &str) -> &str {
    match feature {
        "xci-to-nsp" | "nsp-to-xci" => "Converting",
        "patch-nsp" | "patch-xci" => "Patching",
        "update-xci" | "update-nsp" => "Updating",
        "compress" => "Compressing",
        "decompress" => "Decompressing",
        "extract-fw" => "Extracting FW",
        "extract-save" => "Extraction",
        other => other,
    }
}

/// Build the invocation for one run. `input` is the file for per-file features
/// (the loop drivers); whole-batch features (update) use `req.inputs` instead.
fn build_plan(req: &ToolRequest, input: &str) -> Result<Planned, String> {
    let keys = ps(&keys_path());
    let sq_dir = app_dir().join("squirrel");
    let sq_list = sq_dir.join("list.txt");
    let opts = &req.opts;
    let opt_str = |k: &str, d: &str| -> String {
        opts.get(k).and_then(|v| v.as_str()).unwrap_or(d).to_string()
    };

    let (tool, argv, cwd, list) = match req.feature.as_str() {
        "xci-to-nsp" => {
            let mut a = sv(&["-r", "-k"]);
            a.push(keys);
            a.push(input.to_string());
            ("4nxci".to_string(), a, app_dir(), None)
        }
        "patch-nsp" => {
            let mut a = sv(&["-k"]);
            a.push(keys);
            a.push("-t".to_string());
            a.push(ps(&app_dir().join("renxpack_extracted_nsp")));
            a.push("-o".to_string());
            a.push(ps(&app_dir()));
            a.push(input.to_string());
            ("renxpack".to_string(), a, app_dir(), None)
        }
        "update-xci" | "update-nsp" => {
            let kind = if req.feature == "update-xci" { "xci" } else { "nsp" };
            let mut a = sv(&[
                "-b", "65536", "-pv", "false", "-kp", "false", "--RSVcap", "268435656",
                "-fat", "exfat", "-fx", "files", "-ND", "true", "-t", kind,
            ]);
            a.push("-o".to_string());
            a.push(ps(&sq_dir));
            a.push("-tfile".to_string());
            a.push(ps(&sq_list));
            a.extend(sv(&["-roma", "TRUE", "-dmul", "calculate"]));
            ("squirrel".to_string(), a, bin_dir(), Some(req.inputs.clone()))
        }
        "nsp-to-xci" => {
            // Switch Tools' own `buffer=-b 65536` typo kept verbatim (script.au3:17320).
            let mut a = sv(&[
                "buffer=-b", "65536", "-pv", "false", "-kp", "false", "--RSVcap", "268435656",
                "-fat", "exfat", "-fx", "files", "-ND", "true", "-o",
            ]);
            a.push(ps(&sq_dir));
            a.extend(sv(&["-t", "xci", "-dc"]));
            a.push(input.to_string());
            ("squirrel".to_string(), a, bin_dir(), None)
        }
        "patch-xci" => {
            let mut a = sv(&[
                "-b", "65536", "-pv", "true", "-kp", "0", "--RSVcap", "0", "-fat", "exfat",
                "-fx", "files", "-ND", "true", "-o",
            ]);
            a.push(ps(&sq_dir));
            a.extend(sv(&["-t", "xci", "-dc"]));
            a.push(input.to_string());
            ("squirrel".to_string(), a, bin_dir(), None)
        }
        "compress" => {
            let mut a = sv(&["-b", "65536", "-o"]);
            a.push(ps(&sq_dir));
            a.push("-tfile".to_string());
            a.push(ps(&sq_list));
            a.push("--compress".to_string());
            // Switch Tools level words onto zstd: squirrel int-parses the
            // value, so a word would fall back to 17 for every choice.
            a.push(match opt_str("level", "AVERAGE").as_str() {
                "FAST" => "3".to_string(),
                "INTERMEDIATE" => "9".to_string(),
                "HARD" => "19".to_string(),
                _ => "17".to_string(),
            });
            a.extend(sv(&["--threads", "0", "-ND", "true", "--fexport"]));
            a.push(opt_str("ext", "nsz"));
            ("squirrel".to_string(), a, bin_dir(), Some(vec![input.to_string()]))
        }
        "decompress" => {
            let mut a = sv(&["-b", "65536", "-o"]);
            a.push(ps(&sq_dir));
            a.push("-tfile".to_string());
            a.push(ps(&sq_list));
            a.extend(sv(&["--decompress", "auto"]));
            ("squirrel".to_string(), a, bin_dir(), Some(vec![input.to_string()]))
        }
        "extract-fw" => {
            let mut a = sv(&["-x", "-r", "-y", "-k"]);
            a.push(keys);
            a.extend(sv(&["-t", "xci", "--updatedir=tmpout"]));
            a.push(input.to_string());
            ("hactool".to_string(), a, app_dir(), None)
        }
        "extract-save" => {
            let mut a = sv(&["-k"]);
            a.push(keys);
            a.extend(sv(&["-t", "save"]));
            a.push(input.to_string());
            a.push("--outdir".to_string());
            a.push(ps(&app_dir().join(opt_str("outdir", "tmp"))));
            ("hactool".to_string(), a, app_dir(), None)
        }
        "nro-forwarder" => {
            let nro = app_dir().join("nroforwarder");
            let mut a = sv(&["-k"]);
            a.push(keys);
            for (flag, sub) in [
                ("--nspdir", "hcb_nsp"),
                ("--ncadir", "hcb_nca"),
                ("--tempdir", "hcb_tmp"),
                ("--backupdir", "hcb_back"),
                ("--controldir", "control"),
                ("--exefsdir", "exefs"),
                ("--logodir", "logo"),
                ("--romfsdir", "romfs"),
            ] {
                a.push(flag.to_string());
                a.push(ps(&nro.join(sub)));
            }
            a.extend(sv(&["--titleid"]));
            a.push(opt_str("titleid", "05D37A1090380000"));
            a.extend(sv(&["--titlename"]));
            a.push(opt_str("appname", "TITLE NAME"));
            a.extend(sv(&["--titlepublisher"]));
            a.push(opt_str("author", "AUTHOR"));
            if opts.get("nologo").and_then(|v| v.as_bool()).unwrap_or(false) {
                a.push("--nologo".to_string());
            }
            ("hacbrewpack".to_string(), a, app_dir(), None)
        }
        other => return Err(format!("Unknown feature: {}", other)),
    };

    Ok(Planned { tool, argv, cwd, list })
}

fn command_display(plan: &Planned) -> String {
    let mut out = plan.tool.clone();
    for a in &plan.argv {
        out.push(' ');
        if a.contains(' ') || a.contains('"') {
            out.push('"');
            out.push_str(&a.replace('"', "\\\""));
            out.push('"');
        } else {
            out.push_str(a);
        }
    }
    out
}

// ----- runner -----

struct RunOutcome {
    output: String,
    cancelled: bool,
}

fn squirrel_paths() -> (PathBuf, PathBuf) {
    let dir = app_dir().join("squirrel");
    let list = dir.join("list.txt");
    (dir, list)
}

fn write_list_file(lines: &[String]) -> std::io::Result<()> {
    let (dir, list) = squirrel_paths();
    fs::create_dir_all(&dir)?;
    let mut f = File::create(&list)?;
    for line in lines {
        writeln!(f, "{}", line)?;
    }
    Ok(())
}

/// Spawn the helper, capture stdout+stderr (Switch Tools merges both) and honour
/// `op_cancel` by killing the child held in ACTIVE.
fn run_external(exe: &Path, plan: &Planned, tx: &ProgressTx) -> Result<RunOutcome, String> {
    // Fresh work dir per run: list.txt is rewritten and a stale output file
    // must never be picked up as this run's result.
    if plan.tool == "squirrel" {
        let (dir, _) = squirrel_paths();
        let _ = fs::remove_dir_all(&dir);
    }
    if let Some(lines) = &plan.list {
        write_list_file(lines).map_err(|e| format!("couldn't write list.txt: {}", e))?;
    }

    let mut child = Command::new("nice")
        .arg("-n")
        .arg("10")
        .arg(exe)
        .env("PYTHONUNBUFFERED", "1")
        .args(&plan.argv)
        .current_dir(&plan.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            app_log(&format!("ERROR failed to run bin/{}: {e}", plan.tool));
            format!("failed to run bin/{}: {}", plan.tool, e)
        })?;
    app_log(&format!("RUN bin/{} {}", plan.tool, plan.argv.join(" ")));

    let mut out_pipe = child.stdout.take().expect("piped stdout");
    let out_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = out_pipe.read_to_end(&mut buf);
        buf
    });
    let mut err_pipe = child.stderr.take().expect("piped stderr");

    if let Ok(mut guard) = ACTIVE.lock() {
        *guard = Some(child);
    }

    let mut err_buf = Vec::new();
    let mut tmp = [0u8; 4096];
    while !check_cancel() {
        match err_pipe.read(&mut tmp) {
            Ok(0) => break,
            Ok(n) => {
                err_buf.extend_from_slice(&tmp[..n]);
                let chunk = String::from_utf8_lossy(&tmp[..n]);
                for seg in chunk.split(|c: char| c == '\r' || c == '\n') {
                    let t = seg.trim();
                    if t.is_empty() {
                        continue;
                    }
                    // Live progress: squirrel prints percentage bars.
                    if let Some(pos) = t.rfind('%') {
                        let mut i = pos;
                        while i > 0 {
                            let b = t.as_bytes()[i - 1];
                            if b.is_ascii_digit() {
                                i -= 1;
                            } else {
                                break;
                            }
                        }
                        if i < pos {
                            if let Ok(pct) = t[i..pos].parse::<u32>() {
                                emit_progress(tx, pct, t.to_string());
                            }
                        }
                    }
                    let _ = tx.send((0, format!("LOG: {}", t)));
                    if !t.contains('%') {
                        app_log(t);
                    }
                }
            }
            Err(_) => break,
        }
    }

    if let Ok(mut guard) = ACTIVE.lock() {
        if let Some(mut child) = guard.take() {
            if check_cancel() {
                let _ = child.kill();
            }
            let _ = child.wait();
        }
    }
    let stdout = out_thread.join().unwrap_or_default();
    app_log(&format!(
        "DONE bin/{}{}",
        plan.tool,
        if check_cancel() { " (cancelled)" } else { "" }
    ));
    Ok(RunOutcome {
        output: format!("{}{}", String::from_utf8_lossy(&stdout), String::from_utf8_lossy(&err_buf)),
        cancelled: check_cancel(),
    })
}

/// Switch Tools' free-space gate for tool features (`Round(size/1024/1024*2, 2) + 100`
/// against the app dir). Returns the abort message when it fails.
fn precheck_message(feature: &str, display: &str, total_bytes: u64) -> Option<String> {
    let needed = ((total_bytes as f64 / 1_048_576.0) * 2.0 * 100.0).round() / 100.0 + MIN_FREE_MB as f64;
    let free = free_space_mb(&app_dir()) as f64;
    if free < needed {
        // Switch Tools' update flow prints "Converting for" here (copy/paste in the original).
        let verb = if feature.starts_with("update") { "Converting" } else { feature_verb(feature) };
        return Some(format!(
            "{} for {} aborted\nFree space is to low\nOnly {:.2}GB free\nNeeded min size: {}\n",
            verb,
            display,
            free / 1024.0,
            fmt_size(needed)
        ));
    }
    None
}

fn summary_lines(output: &str) -> Vec<String> {
    let mut found = false;
    let mut out = Vec::new();
    for line in output.lines() {
        let line = line.trim();
        if line.contains("Summary") {
            found = true;
        }
        if found && !line.is_empty() {
            out.push(line.to_string());
        }
    }
    out
}

/// Switch Tools (script.au3:16876): a Summary line naming an .nsp → the token after the
/// first ":" (2nd field, else 3rd), prefixed with @ScriptDir. Falls back to the
/// XCI's own directory, where 4nxci actually writes. No .nsp line, or nothing
/// on disk → None: Switch Tools splits silently or not at all.
fn converted_nsp(output: &str, input: &str) -> Option<PathBuf> {
    let mut token = String::new();
    for line in summary_lines(output) {
        // Switch Tools: StringInStr is case-insensitive.
        if !line.to_ascii_lowercase().contains(".nsp") {
            continue;
        }
        // StringSplit(line, ":") is 1-based: [2] after the 1st colon, [3] after the 2nd.
        let fields: Vec<&str> = line.split(':').collect();
        let mut field = fields.get(1).copied().unwrap_or("");
        let mut lfield = field.to_ascii_lowercase();
        if !lfield.contains(".nsp") {
            field = fields.get(2).copied().unwrap_or("");
            lfield = field.to_ascii_lowercase();
        }
        if let Some(i) = lfield.find(".nsp") {
            token = field[..i + 4].trim().to_string();
            break;
        }
    }
    if token.is_empty() {
        return None;
    }
    // Windows-style tokens from the summary behave like Switch Tools' @ScriptDir & "\" & token.
    let portable = token.replace('\\', "/");
    let tok = Path::new(&portable);
    let direct = app_dir().join(tok);
    if direct.exists() {
        return Some(direct);
    }
    let src = Path::new(input);
    let name = tok.file_name()?;
    let beside = src.parent().unwrap_or(Path::new("")).join(name);
    beside.exists().then_some(beside)
}

fn find_newest_of_ext(dir: &Path, ext: &str) -> Option<PathBuf> {
    use std::time::{SystemTime, UNIX_EPOCH};
    let mut best: Option<(SystemTime, PathBuf)> = None;
    for e in fs::read_dir(dir).ok()?.flatten() {
        let p = e.path();
        if !(p.is_file() && p.extension().map(|x| x.eq_ignore_ascii_case(ext)).unwrap_or(false)) {
            continue;
        }
        let m = e.metadata().and_then(|m| m.modified()).unwrap_or(UNIX_EPOCH);
        if best.as_ref().map(|(t, _)| m > *t).unwrap_or(true) {
            best = Some((m, p));
        }
    }
    best.map(|(_, p)| p)
}

/// Basename of the reNXpack output named in a Summary line, with Switch Tools' rename
/// (`reNXpack` → `reNXpack_patched`), and the rename itself in the app dir.
fn renxpack_patched(output: &str) -> Option<String> {
    for line in output.lines() {
        let tail = match line.find(':') {
            Some(i) => line[i + 1..].trim(),
            None => line.trim(),
        };
        let end = tail.find(".nsp")? + 4;
        let path = &tail[..end];
        let base = path.rsplit(['/', '\\']).next().unwrap_or(path);
        let patched = base.replace("reNXpack", "reNXpack_patched");
        if patched != base {
            let src = app_dir().join(base);
            if src.is_file() {
                let _ = fs::rename(&src, app_dir().join(&patched));
            }
            return Some(patched);
        }
    }
    None
}

fn fw_version(nca_names: &[String], csv: &str) -> String {
    for row in csv.lines() {
        let cols: Vec<&str> = row.split(';').collect();
        if cols.len() >= 2 {
            let (version, content_id) = (cols[0].trim(), cols[1].trim());
            if !content_id.is_empty() && nca_names.iter().any(|n| n.contains(content_id)) {
                return version.to_string();
            }
        }
    }
    String::new()
}

fn squirrel_output_ext(feature: &str, input: &str, opts: &serde_json::Value) -> String {
    match feature {
        "update-xci" | "nsp-to-xci" | "patch-xci" => "xci".to_string(),
        "update-nsp" => "nsp".to_string(),
        "compress" => opts.get("ext").and_then(|v| v.as_str()).unwrap_or("nsz").to_string(),
        // Decompress: .xcz → .xci, .nsz → .nsp.
        _ if input.to_lowercase().ends_with(".xcz") => "xci".to_string(),
        _ => "nsp".to_string(),
    }
}

fn failed_message(feature: &str, input: &str) -> String {
    match feature {
        "update-xci" => format!(
            "Updating failed {}\nSomething failed!?\nIs the selected XCI file a game and the NSP the update?\n",
            input
        ),
        "update-nsp" => format!(
            "Updating failed {}\nSomething failed!?\nIs the selected NSP file a game and the NSP the update?\n",
            input
        ),
        "nsp-to-xci" => format!(
            "Converting failed {}\nAre the keys.dat/prod.keys correct?\nIs the selected NSP file a game? (only games supported)\n",
            input
        ),
        "patch-xci" => format!(
            "Patching failed {}\nAre the keys.dat/prod.keys correct?\nIs the selected NSP file a game? (only games supported)\n",
            input
        ),
        "decompress" => format!("Decompressing failed {}\nAre the keys.dat/prod.keys correct?\n", input),
        _ => format!("{} failed {}\nAre the keys.dat/prod.keys correct?\n", feature_verb(feature), input),
    }
}

/// Switch Tools' post-run output discovery: the helper wrote `<something>.<ext>` into
/// `squirrel/` (or Summary lines, or tmpout); move/report it like Switch Tools does.
fn feature_messages(req: &ToolRequest, input: &str, out: &RunOutcome) -> Vec<String> {
    let feature = req.feature.as_str();
    let verb = feature_verb(feature);

    if out.cancelled {
        return vec![format!("{} canceled {}\n", verb, input)];
    }

    match feature {
        "extract-fw" => {
            let tmpout = app_dir().join("tmpout");
            let ncas: Vec<String> = fs::read_dir(&tmpout)
                .map(|rd| {
                    rd.flatten()
                        .filter(|e| e.path().is_file())
                        .map(|e| e.file_name().to_string_lossy().into_owned())
                        .collect()
                })
                .unwrap_or_default();
            if ncas.is_empty() {
                let _ = fs::remove_dir_all(&tmpout);
                return vec![failed_message(feature, input)];
            }
            let csv = fs::read_to_string(bin_dir().join("fw_nca_mapping.csv")).unwrap_or_default();
            let version = fw_version(&ncas, &csv);
            let dir_name = if version.is_empty() {
                "Firmware".to_string()
            } else {
                format!("Firmware {}", version)
            };
            let dest = app_dir().join(&dir_name);
            if dest.exists() {
                let _ = fs::remove_dir_all(&dest);
            }
            if let Err(e) = fs::rename(&tmpout, &dest) {
                return vec![format!("Extracting FW failed {}\n{}\n", input, e)];
            }
            vec![format!("Extracting FW {} from {} finished.\n", version, input)]
        }
        "xci-to-nsp" => {
            let lines = summary_lines(&out.output);
            if lines.is_empty() {
                vec![format!("Converting failed {}\nAre the prod.keys correct?\n", input)]
            } else {
                vec![format!("Converting finished {}\n{}\n", input, lines.join("\n"))]
            }
        }
        "patch-nsp" => {
            let lines = summary_lines(&out.output);
            let msg = if lines.is_empty() {
                format!("Patching failed {}\nAre the prod.keys correct?\n", input)
            } else {
                let patched = renxpack_patched(&out.output).unwrap_or_else(|| "unknown".to_string());
                format!("Patching finished {}\nPatched file: {}\n{}\n", input, patched, lines.join("\n"))
            };
            let _ = fs::remove_dir_all(app_dir().join("renxpack_extracted_nsp"));
            vec![msg]
        }
        // squirrel-driven features: find the produced file and move it to the app dir.
        "update-xci" | "update-nsp" | "nsp-to-xci" | "patch-xci" | "compress" | "decompress" => {
            let ext = squirrel_output_ext(feature, input, &req.opts);
            let (sq_dir, _) = squirrel_paths();
            match find_newest_of_ext(&sq_dir, &ext) {
                Some(path) => {
                    let mut name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    if feature == "patch-xci" {
                        // Switch Tools: StringReplace ".xci" → "_patched.xci".
                        name = name.replace(".xci", "_patched.xci");
                    }
                    let _ = fs::rename(&path, app_dir().join(&name));
                    let label = if feature.starts_with("update") { "updated file" } else { "converted file" };
                    vec![format!("{} finished {}\n{}: {}\n", verb, input, label, name)]
                }
                None => vec![failed_message(feature, input)],
            }
        }
        _ => vec![failed_message(feature, input)],
    }
}

fn local_stamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as libc::time_t)
        .unwrap_or(0);
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&secs, &mut tm);
        format!(
            "{:04}-{:02}-{:02} {:02}-{:02}-{:02}",
            tm.tm_year + 1900,
            tm.tm_mon + 1,
            tm.tm_mday,
            tm.tm_hour,
            tm.tm_min,
            tm.tm_sec
        )
    }
}

/// hactoolnet save header → Switch Tools' `<timestamp> <saveid>` folder parts.
fn parse_save_header(output: &str) -> (bool, String, String, String, String) {
    let (mut is_save, mut title, mut user, mut save_id, mut timestamp) =
        (false, String::new(), String::new(), String::new(), String::new());
    for line in output.lines() {
        let parts: Vec<&str> = line.split(':').collect();
        if parts.len() < 2 {
            continue;
        }
        let key = parts[0];
        let val = parts[1].trim();
        if key.contains("Title ID") {
            title = val.to_string();
        } else if key.contains("User ID") {
            user = val.to_string();
        } else if key.contains("Save ID") {
            save_id = val.to_string();
        } else if key.contains("Save Type") {
            is_save = true;
        } else if key.contains("Timestamp") && parts.len() >= 4 {
            // Switch Tools: first three ':' segments joined with '-'.
            timestamp = format!("{}-{}-{}", parts[1].trim(), parts[2].trim(), parts[3].trim());
        }
    }
    (is_save, title, user, save_id, timestamp)
}

fn file_size(path: &str) -> u64 {
    fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

/// Runs the helper once; returns the messages plus the raw stdout so callers
/// can keep parsing it (Switch Tools reads the Summary lines after the run).
fn run_single(exe: &Path, req: &ToolRequest, input: &str, tx: &ProgressTx) -> (Vec<String>, String) {
    let plan = match build_plan(req, input) {
        Ok(p) => p,
        Err(e) => return (vec![e], String::new()),
    };
    match run_external(exe, &plan, tx) {
        Ok(out) => {
            let raw = out.output.clone();
            (feature_messages(req, input, &out), raw)
        }
        Err(e) => (
            vec![format!("{} failed {}\n{}\n", feature_verb(&req.feature), input, e)],
            String::new(),
        ),
    }
}

/// Switch Tools' static control.xml template (script.au3:18363); only the screenshot
/// flag, video-capture flag and display version vary.
fn control_xml(version: &str, screenshots: bool, video: bool) -> String {
    const LANGS: [&str; 12] = [
        "AmericanEnglish", "BritishEnglish", "Japanese", "French", "German",
        "LatinAmericanSpanish", "Spanish", "Italian", "Dutch", "CanadianFrench",
        "Portuguese", "Russian",
    ];
    const SUPPORTED: [&str; 11] = [
        "AmericanEnglish", "BritishEnglish", "Japanese", "French", "German",
        "LatinAmericanSpanish", "Spanish", "Italian", "Dutch", "CanadianFrench", "Russian",
    ];
    const RATINGS: [(&str, &str); 9] = [
        ("CERO", "12"), ("ESRB", "10"), ("USK", "12"), ("PEGI", "12"),
        ("PEGIPortugal", "12"), ("PEGIBBFC", "12"), ("Russian", "12"), ("ACB", "13"), ("OFLC", "13"),
    ];

    let mut x = String::from(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<Application>\n",
    );
    for lang in LANGS {
        x += "<Title>\n";
        x += &format!("<Language>{}</Language>\n", lang);
        x += "<Name>hbl</Name>\n<Publisher>Unspecified Author</Publisher>\n</Title>\n";
    }
    x += "<Isbn/>\n<StartupUserAccount>None</StartupUserAccount>\n";
    x += "<UserAccountSwitchLock>Disable</UserAccountSwitchLock>\n<ParentalControl>None</ParentalControl>\n";
    for lang in SUPPORTED {
        x += &format!("<SupportedLanguage>{}</SupportedLanguage>\n", lang);
    }
    x += &format!("<Screenshot>{}</Screenshot>\n", if screenshots { "Allow" } else { "Deny" });
    x += &format!("<VideoCapture>{}</VideoCapture>\n", if video { "Enable" } else { "Disable" });
    x += "<PresenceGroupId>0x0104444444441001</PresenceGroupId>\n";
    x += &format!("<DisplayVersion>{}</DisplayVersion>\n", version);
    for (org, age) in RATINGS {
        x += &format!("<Rating>\n<Organization>{}</Organization>\n<Age>{}</Age>\n</Rating>\n", org, age);
    }
    x += concat!(
        "<DataLossConfirmation>Required</DataLossConfirmation>\n",
        "<PlayLogPolicy>All</PlayLogPolicy>\n",
        "<SaveDataOwnerId>0x0104444444441001</SaveDataOwnerId>\n",
        "<UserAccountSaveDataSize>0x0000000000000000</UserAccountSaveDataSize>\n",
        "<UserAccountSaveDataJournalSize>0x0000000000000000</UserAccountSaveDataJournalSize>\n",
        "<DeviceSaveDataSize>0x0000000000000000</DeviceSaveDataSize>\n",
        "<DeviceSaveDataJournalSize>0x0000000000000000</DeviceSaveDataJournalSize>\n",
        "<BcatDeliveryCacheStorageSize>0x0000000000000000</BcatDeliveryCacheStorageSize>\n",
        "<ApplicationErrorCodeCategory/>\n",
        "<AddOnContentBaseId>0x0104444444442001</AddOnContentBaseId>\n",
        "<LogoType>Nintendo</LogoType>\n",
        "<LocalCommunicationId>0x0104444444441001</LocalCommunicationId>\n",
        "<LogoHandling>Auto</LogoHandling>\n",
        "<SeedForPseudoDeviceId>0x0000000000000000</SeedForPseudoDeviceId>\n",
        "<BcatPassphrase/>\n",
        "<AddOnContentRegistrationType>AllOnLaunch</AddOnContentRegistrationType>\n",
        "<UserAccountSaveDataSizeMax>0x0000000000000000</UserAccountSaveDataSizeMax>\n",
        "<UserAccountSaveDataJournalSizeMax>0x0000000000000000</UserAccountSaveDataJournalSizeMax>\n",
        "<DeviceSaveDataSizeMax>0x0000000000000000</DeviceSaveDataSizeMax>\n",
        "<DeviceSaveDataJournalSizeMax>0x0000000000000000</DeviceSaveDataJournalSizeMax>\n",
        "<TemporaryStorageSize>0x0000000000000000</TemporaryStorageSize>\n",
        "<CacheStorageSize>0x0000000000000000</CacheStorageSize>\n",
        "<CacheStorageJournalSize>0x0000000000000000</CacheStorageJournalSize>\n",
        "<CacheStorageDataAndJournalSizeMax>0x0000000000000000</CacheStorageDataAndJournalSizeMax>\n",
        "<CacheStorageIndexMax>0x0000000000000000</CacheStorageIndexMax>\n",
        "<Hdcp>None</Hdcp>\n",
        "<CrashReport>Deny</CrashReport>\n",
        "<RuntimeAddOnContentInstall>Deny</RuntimeAddOnContentInstall>\n",
        "<PlayLogQueryableApplicationId>0x0000000000000000</PlayLogQueryableApplicationId>\n",
        "<PlayLogQueryCapability>None</PlayLogQueryCapability>\n",
        "<Repair>None</Repair>\n",
        "<Attribute>None</Attribute>\n",
        "<ProgramIndex>0</ProgramIndex>\n",
        "<RequiredNetworkServiceLicenseOnLaunch>None</RequiredNetworkServiceLicenseOnLaunch>\n",
        "</Application>\n",
    );
    x
}

const NRO_MISSING: &str =
    "failed, missing files!\nPlease check for updates or redownload latest SAK\n";

/// Switch Tools' _NROFORWARDER (script.au3:18318): hptnacp builds control.nacp from
/// control.xml, the icon/bin assets fill control/exefs/logo, romfs gets the
/// argv files, then hacbrewpack packs the NSP.
/// Switch Tools removes the work dir whenever the feature ends (script.au3:18642).
fn nro_forwarder(req: &ToolRequest, hacbrewpack: &Path, tx: &ProgressTx) -> String {
    let msg = nro_forwarder_inner(req, hacbrewpack, tx);
    let _ = fs::remove_dir_all(app_dir().join("nroforwarder"));
    msg
}

fn nro_forwarder_inner(req: &ToolRequest, hacbrewpack: &Path, _tx: &ProgressTx) -> String {
    let o = &req.opts;
    let s = |k: &str, d: &str| -> String { o.get(k).and_then(|v| v.as_str()).unwrap_or(d).to_string() };
    let titleid = s("titleid", "");
    if titleid.len() != 16 {
        return "failed, TitleID must be 16 chars!\n".to_string();
    }
    let appname = s("appname", "TITLE NAME");
    let version = s("version", "1.0");
    let nro_path = s("path", "/switch/App-Dir/App.nro");
    let rom_path = s("rompath", "");
    let core = o.get("mode").and_then(|v| v.as_str()) == Some("core");
    let screenshots = o.get("screenshots").and_then(|v| v.as_bool()).unwrap_or(false);
    let video = o.get("videocapture").and_then(|v| v.as_bool()).unwrap_or(false);
    let icon = req.inputs.first().map(String::as_str).unwrap_or("");

    let nro = app_dir().join("nroforwarder");
    let _ = fs::remove_dir_all(&nro);
    for sub in ["control", "exefs", "logo", "romfs"] {
        if fs::create_dir_all(nro.join(sub)).is_err() {
            return NRO_MISSING.to_string();
        }
    }

    let xml_path = nro.join("control.xml");
    let nacp_path = nro.join("control/control.nacp");
    if fs::write(&xml_path, control_xml(&version, screenshots, video)).is_err() {
        return NRO_MISSING.to_string();
    }

    let hptnacp = find_tool("hptnacp").expect("checked in tool_run");
    let plan = Planned {
        tool: "hptnacp".to_string(),
        argv: vec![
            "--action".to_string(),
            "createnacp".to_string(),
            "--input".to_string(),
            ps(&xml_path),
            "--output".to_string(),
            ps(&nacp_path),
        ],
        cwd: app_dir(),
        list: None,
    };
    match run_external(&hptnacp, &plan, _tx) {
        Ok(out) if out.cancelled => return "canceled\n".to_string(),
        Ok(_) => {}
        Err(e) => return format!("failed\n{}\n", e),
    }
    let mut missing = !nacp_path.is_file();
    if !missing {
        let _ = fs::remove_file(&xml_path);
    }

    // Switch Tools copies the JPG as-is; no conversion.
    if !icon.is_empty() && Path::new(icon).is_file() {
        let _ = fs::copy(icon, nro.join("control/icon_AmericanEnglish.dat"));
    } else {
        missing = true;
    }
    for (src_name, dst) in [("nro_fw_main", "exefs/main"), ("nro_fw_main.npdm", "exefs/main.npdm")] {
        let src = bin_dir().join(src_name);
        if src.is_file() {
            let _ = fs::copy(&src, nro.join(dst));
        } else {
            missing = true;
        }
    }
    // custom/ overrides the packaged logo files.
    for (custom_name, bin_name, dst) in [
        ("NintendoLogo.png", "nro_fw_NintendoLogo.png", "logo/NintendoLogo.png"),
        ("StartupMovie.gif", "nro_fw_StartupMovie.gif", "logo/StartupMovie.gif"),
    ] {
        let custom = app_dir().join("custom").join(custom_name);
        let fallback = bin_dir().join(bin_name);
        if custom.is_file() {
            let _ = fs::copy(&custom, nro.join(dst));
        } else if fallback.is_file() {
            let _ = fs::copy(&fallback, nro.join(dst));
        } else {
            missing = true;
        }
    }
    if missing {
        return NRO_MISSING.to_string();
    }

    let next_argv = if core {
        format!("sdmc:{} \"sdmc:{}\"", nro_path, rom_path)
    } else {
        format!("sdmc:{}", nro_path)
    };
    if fs::write(nro.join("romfs/nextArgv"), next_argv).is_err()
        || fs::write(nro.join("romfs/nextNroPath"), format!("sdmc:{}", nro_path)).is_err()
    {
        return NRO_MISSING.to_string();
    }

    let plan = match build_plan(req, "") {
        Ok(p) => p,
        Err(e) => return e,
    };
    match run_external(hacbrewpack, &plan, _tx) {
        Ok(out) if out.cancelled => "canceled\n".to_string(),
        Err(e) => format!("failed\n{}\n", e),
        Ok(_) => match find_newest_of_ext(&nro.join("hcb_nsp"), "nsp") {
            Some(path) => {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let dest = app_dir().join(format!("{}-{}", appname, name));
                let _ = fs::copy(&path, &dest);
                format!("created: {}\n", ps(&dest))
            }
            None => format!("create failed: {}\n", appname),
        },
    }
}

pub fn tool_run(tx: &ProgressTx, req: ToolRequest) -> ToolRun {
    CANCELLED.store(false, Ordering::SeqCst);

    let feature = req.feature.clone();
    app_log(&format!("START {} inputs: {}", feature, req.inputs.join(", ")));
    if req.inputs.is_empty() {
        return ToolRun {
            missing: false,
            tool: feature,
            command: String::new(),
            messages: vec!["No input selected\n".to_string()],
        };
    }
    let input0 = req.inputs[0].clone();
    let plan0 = match build_plan(&req, &input0) {
        Ok(p) => p,
        Err(e) => {
            return ToolRun { missing: false, tool: feature, command: String::new(), messages: vec![e] }
        }
    };
    let command = command_display(&plan0);
    let exe = match find_tool(&plan0.tool) {
        Some(p) => p,
        None => {
            let msg = format!("couldn't find bin/{}", plan0.tool);
            return ToolRun { missing: true, tool: plan0.tool, command, messages: vec![msg] };
        }
    };

    // The forwarder is two steps; Switch Tools checks both helpers up front.
    if feature == "nro-forwarder" && find_tool("hptnacp").is_none() {
        return ToolRun {
            missing: true,
            tool: "hptnacp".to_string(),
            command: format!(
                "hptnacp --action createnacp --input {} --output {}",
                ps(&app_dir().join("nroforwarder/control.xml")),
                ps(&app_dir().join("nroforwarder/control/control.nacp"))
            ),
            messages: vec!["couldn't find bin/hptnacp".to_string()],
        };
    }

    let mut messages: Vec<String> = Vec::new();
    match feature.as_str() {
        // Switch Tools loops these per input (the progress bar advances once per file).
        "compress" | "decompress" | "extract-save" => {
            let mut had_error = false;
            let total = req.inputs.len() as u64;
            for (i, input) in req.inputs.iter().enumerate() {
                if check_cancel() {
                    messages.push(format!("{} canceled {}\n", feature_verb(&feature), input));
                    break;
                }
                // Switch Tools has no free-space gate for save extraction.
                if feature != "extract-save" {
                    if let Some(msg) = precheck_message(&feature, input, file_size(input)) {
                        messages.push(msg);
                        continue;
                    }
                }
                let plan = match build_plan(&req, input) {
                    Ok(p) => p,
                    Err(e) => {
                        had_error = true;
                        messages.push(e);
                        break;
                    }
                };
                match run_external(&exe, &plan, &tx) {
                    Ok(out) if feature == "extract-save" => {
                        // Switch Tools: hactoolnet dumps into <tmp>; a real save moves to
                        // extracted_save_<date>/<user>/<title>/<timestamp> <saveid>.
                        let tmp = app_dir().join(
                            req.opts.get("outdir").and_then(|v| v.as_str()).unwrap_or("tmp"),
                        );
                        let (is_save, title, user, save_id, timestamp) = parse_save_header(&out.output);
                        if is_save && !out.cancelled {
                            let dest = app_dir()
                                .join(format!("extracted_save_{}", local_stamp()))
                                .join(&user)
                                .join(&title);
                            let _ = fs::create_dir_all(&dest);
                            let _ = fs::rename(&tmp, dest.join(format!("{} {}", timestamp, save_id)));
                        } else if !is_save {
                            let _ = fs::remove_dir_all(&tmp);
                        }
                    }
                    Ok(out) => messages.extend(feature_messages(&req, input, &out)),
                    Err(e) => {
                        had_error = true;
                        messages.push(format!("{} failed {}\n{}\n", feature_verb(&feature), input, e));
                    }
                }
                // Switch Tools advances the bar once per finished file.
                let pct = ((((i + 1) as u64) * 100) / total) as u32;
                emit_progress(tx, pct, format!("State: {}/{} is running...", i + 1, total));
            }
            // Keep spawn/build errors visible instead of masking them.
            if feature == "extract-save" && !had_error {
                messages = if check_cancel() {
                    let tmp = app_dir().join(req.opts.get("outdir").and_then(|v| v.as_str()).unwrap_or("tmp"));
                    let _ = fs::remove_dir_all(tmp);
                    vec!["User canceled extraction\n".to_string()]
                } else {
                    vec!["Extraction finished\n".to_string()]
                };
            }
        }
        // One run over the whole selection (update joins base + updates in list.txt).
        "update-xci" | "update-nsp" | "xci-to-nsp" | "patch-nsp" | "nsp-to-xci" | "patch-xci"
        | "extract-fw" => {
            let total: u64 = req.inputs.iter().map(|i| file_size(i)).sum();
            // Switch Tools gates everything but extract-fw on free space.
            if feature == "extract-fw" {
                messages.extend(run_single(&exe, &req, &input0, &tx).0);
                emit_progress(tx, 100, "State: finished".to_string());
            } else if let Some(msg) = precheck_message(&feature, &input0, total) {
                messages.push(msg);
            } else {
                let (msgs, raw) = run_single(&exe, &req, &input0, &tx);
                let converted = msgs.iter().any(|m| m.starts_with("Converting finished"));
                messages.extend(msgs);
                // Switch Tools (script.au3:16876): "Split NSP file after converting" —
                // resolve the NSP named in the Summary and split it, optionally
                // straight to USB.
                if feature == "xci-to-nsp"
                    && converted
                    && req.opts.get("split").and_then(|v| v.as_bool()).unwrap_or(false)
                    && !check_cancel()
                {
                    if let Some(nsp) = converted_nsp(&raw, &input0) {
                        let dest = req.opts.get("dest").and_then(|v| v.as_str()).map(String::from);
                        messages.push(split_one(tx, &nsp.to_string_lossy(), dest, false, "1/1"));
                    }
                }
                emit_progress(tx, 100, "State: finished".to_string());
            }
        }
        // Switch Tools prefixes every outcome with "NRO forwarder ".
        "nro-forwarder" => messages.push(format!("NRO forwarder {}", nro_forwarder(&req, &exe, &tx))),
        other => messages.push(format!("Unknown feature: {}", other)),
    }

    // Switch Tools removes its squirrel work dir when the feature ends.
    let (sq_dir, _) = squirrel_paths();
    let _ = fs::remove_dir_all(&sq_dir);

    ToolRun { missing: false, tool: plan0.tool, command, messages }
}

// ---------- URL opener (forum / updates) ----------

pub fn open_url(url: String) -> Result<(), String> {
    if !(url.starts_with("https://psxtools.de/") || url.starts_with("https://github.com/dezem/SAK")) {
        return Err("URL not allowed".into());
    }
    Command::new("xdg-open")
        .arg(&url)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

pub fn info_text() -> String {
    "Switch Tools (Linux port)\nVersion: 0.7.14\nMaintained by luciano\n\n\
Built on the Switch Tools v0.7.14 feature set (original by kempa / psxtools.de),\n\
ported to a native Rust (egui) portable app for Linux.\n\n\
With \"Switch Tools\" you can easily manage files for your Switch:\n\
- Patch XCI/NSP to lower firmware\n\
- Split / Merge XCI / NSP\n\
- Update XCI/NSP with updates and/or DLCs\n\
- Convert XCI <-> NSP\n\
- Compress XCI -> XCZ / NSP -> NSZ\n\
- Decompress XCZ -> XCI / NSZ -> NSP\n\
- Extract FW from XCI / Extract Savegames\n\
- Create NRO / RetroArch forwarder (installable NSP)\n\
- Format storage (all sizes) to FAT32\n\n\
Thanks goes to:\n\
- kempa for the original Switch Tools (psxtools.de)\n\
- The-4n and SciresM for 4NXCI and reNXpack\n\
- The-4n for hacBrewPack / hptnacp\n\
- switchbrew for nx-hbloader\n\
- SciresM for hactool\n\
- Thealexbarney for LibHac\n\
- julesontheroad for NSC_BUILDER\n\
- Ridgecrop Consultants Ltd for FAT32 Format as GUI version\n\
- luciano: Linux port and packaging"
        .to_string()
}

pub fn changelog_text() -> String {
    include_str!("../changelog.txt").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn part_naming_xci() {
        let name = "Game.xci";
        let stem = name.to_lowercase().replace(".xci", "_splitted.xc");
        assert_eq!(stem, "game_splitted.xc");
        assert_eq!(format!("{}{}", stem, 0), "game_splitted.xc0");
        assert_eq!(format!("{}{}", stem, 1), "game_splitted.xc1");
    }

    #[test]
    fn part_naming_nsp() {
        let name = "Game.nsp";
        let folder = name.to_lowercase().replace(".nsp", "_splitted.nsp");
        assert_eq!(folder, "game_splitted.nsp");
        assert_eq!(format!("{:02}", 0), "00");
        assert_eq!(format!("{:02}", 12), "12");
    }

    #[test]
    fn trim_detects_ff_runs() {
        let empty = vec![0xFFu8; 4096];
        let data = vec![0x41u8; 4096];
        let mixed = {
            let mut v = vec![0x41u8; 100];
            v.extend(std::iter::repeat(0xFFu8).take(12));
            v.extend(std::iter::repeat(0x42u8).take(100));
            v
        };
        assert!(chunk_is_empty_ff(&empty));
        assert!(!chunk_is_empty_ff(&data));
        assert!(chunk_is_empty_ff(&mixed));
    }

    #[test]
    fn titleid_format() {
        let id = generate_titleid();
        assert_eq!(id.len(), 16);
        assert!(id.starts_with("01"));
        assert!(id.ends_with("000"));
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
    }

    fn tool_req(feature: &str, inputs: &[&str]) -> ToolRequest {
        ToolRequest {
            feature: feature.to_string(),
            inputs: inputs.iter().map(|s| s.to_string()).collect(),
            opts: serde_json::json!({}),
        }
    }

    #[test]
    fn plan_xci_to_nsp_argv() {
        let r = tool_req("xci-to-nsp", &["/tmp/Game.xci"]);
        let p = build_plan(&r, "/tmp/Game.xci").unwrap();
        assert_eq!(p.tool, "4nxci");
        assert_eq!(p.argv[0], "-r");
        assert_eq!(p.argv[1], "-k");
        assert!(p.argv[2].ends_with("prod.keys") || p.argv[2].ends_with("keys.dat"));
        assert_eq!(p.argv[3], "/tmp/Game.xci");
        assert!(p.list.is_none());
    }

    #[test]
    fn plan_update_batches_all_inputs_into_list() {
        let r = tool_req("update-xci", &["/tmp/base.xci", "/tmp/upd.nsp"]);
        let p = build_plan(&r, "/tmp/base.xci").unwrap();
        assert_eq!(p.tool, "squirrel");
        assert_eq!(
            p.list.unwrap(),
            vec!["/tmp/base.xci".to_string(), "/tmp/upd.nsp".to_string()]
        );
        let joined = p.argv.join(" ");
        assert!(joined.contains("-t xci"));
        assert!(joined.contains("-tfile"));
    }

    #[test]
    fn summary_lines_start_at_summary() {
        let out = "stuff\nnoise\nSummary:\n  Foo: 1\n\n  Bar: 2\n";
        assert_eq!(summary_lines(out), vec!["Summary:", "Foo: 1", "Bar: 2"]);
    }

    #[test]
    fn fw_version_matches_content_id() {
        let csv = "13.1.0;AAA111\n14.2.0;BBB222\n";
        assert_eq!(fw_version(&["system_1420.nca".to_string()], csv), "");
        assert_eq!(fw_version(&["0100BBB222999000.nca".to_string()], csv), "14.2.0");
    }

    #[test]
    fn parse_save_header_fields() {
        let out = "Title ID: 0100ABCD12340000\nUser ID: 0000000000000001\nSave ID: F00DBEEF\nSave Type: 1\nTimestamp: 2021-05-04 13:37:00\n";
        let (is_save, title, user, save_id, ts) = parse_save_header(out);
        assert!(is_save);
        assert_eq!(title, "0100ABCD12340000");
        assert_eq!(user, "0000000000000001");
        assert_eq!(save_id, "F00DBEEF");
        assert_eq!(ts, "2021-05-04 13-37-00");
    }

    #[test]
    fn parse_save_header_rejects_non_save() {
        let (is_save, ..) = parse_save_header("error: something went wrong\n");
        assert!(!is_save);
    }

    #[test]
    fn tool_run_reports_missing_tool_with_command() {
        // renxpack: no other test creates it, so the "missing" branch is stable.
        let r = tool_req("patch-nsp", &["/tmp/Base.nsp"]);
        let (tx, _rx) = std::sync::mpsc::channel();
        let res = tool_run(&tx, r);
        assert!(res.missing);
        assert!(res.command.starts_with("renxpack "));
        assert!(res.command.ends_with("/tmp/Base.nsp"));
    }

    #[cfg(unix)]
    #[test]
    fn tool_run_executes_helper_and_finds_output() {
        use std::os::unix::fs::PermissionsExt;
        let exe = bin_dir().join("squirrel");
        fs::create_dir_all(bin_dir()).unwrap();
        // Fake squirrel: parses -o, writes <out>/Game.nsp.
        fs::write(
            &exe,
            "#!/bin/sh\nwhile [ $# -gt 0 ]; do if [ \"$1\" = \"-o\" ]; then out=$2; fi; shift; done\nmkdir -p \"$out\" && echo fake > \"$out/Game.nsp\"\n",
        )
        .unwrap();
        fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();

        let r = tool_req("update-nsp", &["/tmp/base.nsp"]);
        let (tx, _rx) = std::sync::mpsc::channel();
        let res = tool_run(&tx, r);
        let text = res.messages.join("");

        let _ = fs::remove_file(&exe);
        let _ = fs::remove_file(app_dir().join("Game.nsp"));

        assert!(!res.missing, "fake squirrel exists: {:?}", res);
        assert!(text.contains("Updating finished /tmp/base.nsp"), "{}", text);
        assert!(text.contains("updated file: Game.nsp"), "{}", text);
    }

    #[test]
    fn control_xml_matches_sak_template() {
        let xml = control_xml("2.5", true, false);
        assert_eq!(xml.matches("<Title>").count(), 12);
        assert_eq!(xml.matches("<SupportedLanguage>").count(), 11);
        assert!(xml.contains("<Name>hbl</Name>"));
        assert!(xml.contains("<DisplayVersion>2.5</DisplayVersion>"));
        assert!(xml.contains("<Screenshot>Allow</Screenshot>"));
        assert!(xml.contains("<VideoCapture>Disable</VideoCapture>"));
        assert!(xml.contains("<Organization>CERO</Organization>"));
        assert!(xml.trim_end().ends_with("</Application>"));
    }

    #[test]
    fn nro_forwarder_rejects_bad_titleid() {
        let r = tool_req("nro-forwarder", &["/tmp/icon.jpg"]);
        let (tx, _rx) = std::sync::mpsc::channel();
        let msg = nro_forwarder(&r, Path::new("/nonexistent/hacbrewpack"), &tx);
        assert_eq!(msg, "failed, TitleID must be 16 chars!\n");
    }

    #[cfg(unix)]
    #[test]
    fn nro_forwarder_end_to_end_with_fake_helpers() {
        use std::os::unix::fs::PermissionsExt;
        fs::create_dir_all(bin_dir()).unwrap();

        // Fake hptnacp: --output <file> → writes the nacp.
        let hptnacp = bin_dir().join("hptnacp");
        fs::write(
            &hptnacp,
            "#!/bin/sh\nwhile [ $# -gt 0 ]; do if [ \"$1\" = \"--output\" ]; then out=$2; fi; shift; done\nmkdir -p \"$(dirname \"$out\")\" && echo nacp > \"$out\"\n",
        )
        .unwrap();
        fs::set_permissions(&hptnacp, fs::Permissions::from_mode(0o755)).unwrap();
        // Fake hacbrewpack: --nspdir <dir> → writes one nsp.
        let hacbrewpack = bin_dir().join("hacbrewpack");
        fs::write(
            &hacbrewpack,
            "#!/bin/sh\nwhile [ $# -gt 0 ]; do if [ \"$1\" = \"--nspdir\" ]; then d=$2; fi; shift; done\nmkdir -p \"$d\" && echo nsp > \"$d/05D37A1090380000.nsp\"\n",
        )
        .unwrap();
        fs::set_permissions(&hacbrewpack, fs::Permissions::from_mode(0o755)).unwrap();
        for asset in ["nro_fw_main", "nro_fw_main.npdm", "nro_fw_NintendoLogo.png", "nro_fw_StartupMovie.gif"] {
            fs::write(bin_dir().join(asset), b"x").unwrap();
        }
        let icon = std::env::temp_dir().join("sak_nro_test_icon.jpg");
        fs::write(&icon, b"jpg").unwrap();

        let r = ToolRequest {
            feature: "nro-forwarder".to_string(),
            inputs: vec![icon.to_string_lossy().into_owned()],
            opts: serde_json::json!({
                "appname": "MyApp", "titleid": "05D37A1090380000", "author": "me",
                "version": "2.5", "screenshots": true, "videocapture": false,
                "mode": "nro", "path": "/switch/App/App.nro", "rompath": ""
            }),
        };
        let (tx, _rx) = std::sync::mpsc::channel();
        let res = tool_run(&tx, r);
        let text = res.messages.join("");
        let dest = app_dir().join("MyApp-05D37A1090380000.nsp");

        // Switch Tools removes the work dir when the feature ends.
        let workdir_gone = !app_dir().join("nroforwarder").exists();
        for p in [
            hptnacp, hacbrewpack, dest, icon,
            bin_dir().join("nro_fw_main"),
            bin_dir().join("nro_fw_main.npdm"),
            bin_dir().join("nro_fw_NintendoLogo.png"),
            bin_dir().join("nro_fw_StartupMovie.gif"),
        ] {
            let _ = fs::remove_file(p);
        }

        assert!(!res.missing, "{:?}", res);
        assert!(text.starts_with("NRO forwarder created: "), "{}", text);
        assert!(text.contains("MyApp-05D37A1090380000.nsp"), "{}", text);
        assert!(workdir_gone);
    }

    #[cfg(unix)]
    #[test]
    fn convert_with_split_option_splits_the_nsp() {
        use std::os::unix::fs::PermissionsExt;
        fs::create_dir_all(bin_dir()).unwrap();
        // Fake 4nxci: writes <stem>.nsp next to the input and prints a Summary,
        // like the real tool.
        let exe = bin_dir().join("4nxci");
        fs::write(
            &exe,
            "#!/bin/sh\nfor a in \"$@\"; do if [ -f \"$a\" ]; then in=\"$a\"; fi; done\nbase=${in%.xci}\necho nsp > \"$base.nsp\"\necho \"Summary:\"\necho \"  TitleID: 0100ABCD12340000\"\necho \"  Filename: sak_conv_split_test.nsp\"\n",
        )
        .unwrap();
        fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
        let xci = std::env::temp_dir().join("sak_conv_split_test.xci");
        fs::write(&xci, b"x").unwrap();

        let r = ToolRequest {
            feature: "xci-to-nsp".to_string(),
            inputs: vec![xci.to_string_lossy().into_owned()],
            opts: serde_json::json!({ "split": true }),
        };
        let (tx, _rx) = std::sync::mpsc::channel();
        let res = tool_run(&tx, r);
        let text = res.messages.join("");

        let _ = fs::remove_file(&exe);
        let _ = fs::remove_file(&xci);
        let _ = fs::remove_file(std::env::temp_dir().join("sak_conv_split_test.nsp"));

        assert!(text.starts_with("Converting finished "), "{}", text);
        // The fake nsp is tiny, so the split stops at Switch Tools' 4GB gate — which
        // proves the post-convert split actually ran.
        assert!(text.contains("Splitting for sak_conv_split_test.nsp aborted"), "{}", text);
        assert!(text.contains("FileSize is lower than 4GB"), "{}", text);
    }

    #[test]
    fn device_for_mountpoint_reads_proc_mounts() {
        assert!(device_for_mountpoint("/").is_some());
        assert!(device_for_mountpoint("/definitely/not/mounted-xyz").is_none());
    }

    #[test]
    fn converted_nsp_parses_summary_and_resolves_paths() {
        let dir = std::env::temp_dir().join("sak_conv_resolve_test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let xci = dir.join("sample_conv.xci");
        let nsp = dir.join("sample_conv.nsp");
        fs::write(&xci, b"x").unwrap();
        fs::write(&nsp, b"y").unwrap();
        let xi = xci.to_string_lossy().into_owned();

        // Relative token → resolved next to the input (4nxci's output dir).
        let out = "4nxci\nSummary:\n  Filename: sample_conv.nsp\n";
        assert_eq!(converted_nsp(out, &xi), Some(nsp.clone()));

        // Windows drive token (fields [2]/[3]) → <stem>.nsp fallback.
        let out = "Summary:\n  Filename: D:\\dir\\sample_conv.nsp\n";
        assert_eq!(converted_nsp(out, &xi), Some(nsp.clone()));

        // No .nsp named in the Summary → Switch Tools never splits.
        assert_eq!(converted_nsp("Summary:\n  TitleID: 0100\n", &xi), None);
        // Summary naming an .nsp that exists nowhere → silent, no split.
        assert_eq!(converted_nsp("Summary:\n  Filename: ghost.nsp\n", &xi), None);

        fs::remove_dir_all(&dir).unwrap();
    }
}
