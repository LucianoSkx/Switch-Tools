//! egui GUI — single-window app: sidebar navigation + form area + live output log.
//! Same tools, labels and tooltips as Switch Tools v0.7.14; the flow is modernized.

use crate::{
    app_log, app_log_reset, fat32_format, generate_titleid, info_text, keys_status, merge_files,
    op_cancel, split_files, tool_run, usb_devices, Fat32Request, MergeRequest, OpResult,
    ProgressTx, SplitRequest, ToolRequest, ToolRun, UsbDevice,
};
use eframe::egui;
use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

const TITLE: &str = "Switch Tools";
const LEVELS: [&str; 4] = ["FAST", "INTERMEDIATE", "AVERAGE", "HARD"];
/// Cyan from the Switch Tools logo — used as the app accent (selection, links, primary buttons).
const ACCENT: egui::Color32 = egui::Color32::from_rgb(0x2A, 0xB8, 0xD8);
/// Secondary text — readable on the dark theme (much brighter than `weak()`).
const MUTED: egui::Color32 = egui::Color32::from_rgb(195, 202, 212);
/// Card outline — subtle definition on the dark background.
const CARD_STROKE: egui::Color32 = egui::Color32::from_rgb(66, 70, 76);
/// Background for list rows inside cards.
const ROW_BG: egui::Color32 = egui::Color32::from_rgb(26, 28, 32);

pub fn run() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(TITLE)
            .with_inner_size([960.0, 640.0])
            .with_min_inner_size([760.0, 520.0])
            .with_icon(egui::IconData {
                rgba: include_bytes!("../icons/sak-icon.rgba").to_vec(),
                width: 128,
                height: 128,
            }),
        ..Default::default()
    };
    eframe::run_native(
        TITLE,
        options,
        Box::new(|cc| Ok(Box::new(Sak::new(cc.egui_ctx.clone())))),
    )
}

// ---------- state ----------

#[derive(Clone, Copy, PartialEq)]
enum Tool {
    XciSplit,
    XciMerge,
    UpdateXci,
    UpdateNsp,
    XciToNsp,
    PatchNsp,
    PatchXci,
    NspToXci,
    ExtractFw,
    NspSplit,
    NspMerge,
    CompressXci,
    CompressNsp,
    DecompressXcz,
    DecompressNsz,
    ExtractSave,
    Fat32,
    Nro,
}

/// Which file slot a file card writes to.
#[derive(Clone, Copy)]
enum Slot {
    Main,
    Updates,
}

#[derive(Default)]
struct Form {
    files: Vec<String>,
    updates: Vec<String>,
    usb_on: bool,
    usb_sel: usize,
    path_sel: usize,
    trim: bool,
    split_chk: bool,
    level_sel: usize,
    // FAT32
    fmt_sel: usize,
    fmt_confirm: bool,
    // Overwrite warning (compress / decompress)
    ow_confirm: bool,
    // NRO forwarder
    appname: String,
    titleid: String,
    author: String,
    version: String,
    icon: Option<String>,
    shots: bool,
    video: bool,
    nologo: bool,
    core: Option<bool>,
    nro_path: String,
    rom_path: String,
}

enum Job {
    Tool(ToolRun),
    Op(OpResult),
}

struct Worker {
    done: mpsc::Receiver<Job>,
}

struct Sak {
    sel: Tool,
    form: Form,
    worker: Option<Worker>,
    prog_rx: Option<mpsc::Receiver<(u32, String)>>,
    pct: u32,
    state_text: String,
    started: Option<Instant>,
    msgbox: Option<(String, String)>,
    usb: Vec<UsbDevice>,
    run_title: String,
    log: Vec<String>,
    logo: Option<egui::TextureHandle>,
}

impl Sak {
    fn new(ctx: egui::Context) -> Self {
        apply_theme(&ctx);
        // Use Inter (variable weight) as the proportional family — modern sans.
        let mut fonts = egui::FontDefinitions::default();
        fonts.font_data.insert(
            "inter".to_owned(),
            egui::FontData::from_owned(include_bytes!("../fonts/Inter-Regular.ttf").to_vec()).into(),
        );
        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, "inter".to_owned());
        ctx.set_fonts(fonts);
        let logo = image::load_from_memory(include_bytes!("../icons/icon.png")).ok().map(|img| {
            let size = [img.width() as usize, img.height() as usize];
            let rgba = img.to_rgba8();
            ctx.load_texture(
                "header-logo",
                egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()),
                egui::TextureOptions::default(),
            )
        });
        Sak {
            sel: Tool::XciSplit,
            form: Form::default(),
            worker: None,
            prog_rx: None,
            pct: 0,
            state_text: String::new(),
            started: None,
            msgbox: None,
            usb: Vec::new(),
            run_title: String::new(),
            log: Vec::new(),
            logo,
        }
    }

    // ---------- jobs ----------

    fn attach(&mut self, done: mpsc::Receiver<Job>, prog: mpsc::Receiver<(u32, String)>) {
        self.worker = Some(Worker { done });
        self.prog_rx = Some(prog);
        self.pct = 0;
        self.state_text.clear();
        self.started = Some(Instant::now());
    }

    fn spawn_tool(&mut self, feature: &str, inputs: Vec<String>, opts: serde_json::Value) {
        let (ptx, prx) = mpsc::channel();
        let (dtx, drx) = mpsc::channel();
        let feature = feature.to_string();
        std::thread::spawn(move || {
            let req = ToolRequest { feature, inputs, opts };
            let _ = dtx.send(Job::Tool(tool_run(&ptx, req)));
        });
        self.attach(drx, prx);
    }

    fn spawn_op<F>(&mut self, op: F)
    where
        F: FnOnce(&ProgressTx) -> OpResult + Send + 'static,
    {
        let (ptx, prx) = mpsc::channel();
        let (dtx, drx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = dtx.send(Job::Op(op(&ptx)));
        });
        self.attach(drx, prx);
    }

    fn start(&mut self, feature: &str, inputs: Vec<String>, opts: serde_json::Value) {
        self.run_title = feature.to_string();
        self.log.clear();
        app_log_reset();
        app_log(&format!("=== {} started, {} input(s)", feature, inputs.len()));
        self.spawn_tool(feature, inputs, opts);
    }

    fn start_split(&mut self, req: SplitRequest, title: &str) {
        self.run_title = title.to_string();
        self.log.clear();
        app_log_reset();
        app_log(&format!("=== {} started, inputs: {}", title, req.inputs.join(", ")));
        self.spawn_op(move |tx| split_files(tx, req));
    }

    fn start_merge(&mut self, req: MergeRequest, title: &str) {
        self.run_title = title.to_string();
        self.log.clear();
        app_log_reset();
        app_log(&format!("=== {} started, input: {} ({})", title, req.input, req.kind));
        self.spawn_op(move |tx| merge_files(tx, req));
    }

    fn drain(&mut self) {
        let mut last = None;
        if let Some(w) = &self.worker {
            while let Ok(job) = w.done.try_recv() {
                last = Some(job);
            }
        }
        if let Some(rx) = &self.prog_rx {
            while let Ok((pct, state)) = rx.try_recv() {
                if let Some(line) = state.strip_prefix("LOG: ") {
                    let line = line.trim();
                    if !line.is_empty() {
                        self.log.push(line.to_string());
                    }
                    continue;
                }
                if !state.is_empty() {
                    self.pct = pct;
                    self.state_text = state;
                } else if pct > 0 {
                    self.pct = pct;
                }
            }
        }
        if let Some(job) = last {
            self.on_done(job);
        }
    }

    fn on_done(&mut self, job: Job) {
        let elapsed = self.started.map(|s| fmt_elapsed(s.elapsed())).unwrap_or_default();
        self.worker = None;
        self.prog_rx = None;
        let (messages, missing, tool, command) = match job {
            Job::Tool(r) => (r.messages, r.missing, r.tool, r.command),
            Job::Op(r) => (r.messages, false, String::new(), String::new()),
        };
        if missing {
            app_log(&format!("MISSING bin/{}; command ready: {}", tool, command));
            self.msgbox = Some((
                if self.run_title.is_empty() {
                    "Command ready".to_string()
                } else {
                    self.run_title.clone()
                },
                format!(
                    "This tool is not installed yet.\n\nCommand that will run:\n\n{}\n\nPlace the Linux build of {} in the bin folder to enable this feature.",
                    command, tool
                ),
            ));
            return;
        }
        for m in &messages {
            for line in m.lines() {
                app_log(line);
            }
        }
        app_log(&format!(
            "FINISHED {}{} Needed time {}.",
            self.run_title,
            if messages.is_empty() { "" } else { ": " },
            elapsed
        ));
        self.log.extend(messages);
        self.log.push(format!("Needed time {}.", elapsed));
    }

    // ---------- navigation ----------

    fn select(&mut self, t: Tool) {
        let ok = match t {
            Tool::PatchXci
            | Tool::CompressXci
            | Tool::DecompressXcz
            | Tool::NspToXci
            | Tool::CompressNsp
            | Tool::DecompressNsz => self.gate("squirrel"),
            Tool::XciToNsp => self.gate("4nxci"),
            Tool::PatchNsp => self.gate("renxpack"),
            Tool::ExtractFw => self.gate("hactool"),
            Tool::ExtractSave => self.gate("hactool"),
            Tool::Nro => self.gate("hacbrewpack"),
            _ => true,
        };
        if !ok {
            return;
        }
        self.sel = t;
        self.form = Form::default();
        match t {
            Tool::CompressXci | Tool::CompressNsp => self.form.level_sel = 2,
            Tool::Nro => {
                self.form.appname = "TITLE NAME".into();
                self.form.author = "AUTHOR".into();
                self.form.version = "1.0".into();
                self.form.nro_path = "/switch/App-Dir/App.nro".into();
                self.form.rom_path = "/retroarch/roms/ROMNAME.extension".into();
                self.form.titleid = generate_titleid();
            }
            Tool::Fat32 => self.refresh_usb(),
            _ => {}
        }
        self.log.clear();
        self.pct = 0;
        self.state_text.clear();
    }

    fn gate(&mut self, tool: &str) -> bool {
        if keys_status().found {
            return true;
        }
        self.msgbox = Some((
            format!("{} - keys.dat/prod.keys missing", tool),
            "Place your keys.dat or prod.keys inside the bin folder to use this feature!"
                .to_string(),
        ));
        false
    }

    fn refresh_usb(&mut self) {
        self.usb = usb_devices();
    }

    /// Switch Tools's USB destination: mountpoint + selected SD path (`SD root` = none).
    fn usb_dest(&self, paths: &[&str]) -> Option<String> {
        if !self.form.usb_on {
            return None;
        }
        let dev = self.usb.get(self.form.usb_sel)?;
        let sub = paths[self.form.path_sel.min(paths.len() - 1)];
        if sub == "SD root" {
            Some(dev.mountpoint.clone())
        } else {
            Some(format!("{}/{}", dev.mountpoint, sub.replace('\\', "/")))
        }
    }

    fn usb_items(&self) -> Vec<String> {
        if self.usb.is_empty() {
            return vec!["(no devices)".to_string()];
        }
        self.usb
            .iter()
            .map(|d| format!("{} ({:.1}GB)", d.label, d.free_mb as f64 / 1024.0))
            .collect()
    }
}

fn apply_theme(ctx: &egui::Context) {
    let mut s = (*ctx.style()).clone();
    s.spacing.item_spacing = egui::vec2(8.0, 6.0);
    s.spacing.button_padding = egui::vec2(10.0, 3.0);
    let v = &mut s.visuals;
    v.panel_fill = egui::Color32::from_rgb(32, 34, 39);
    v.window_fill = egui::Color32::from_rgb(38, 40, 45);
    v.faint_bg_color = egui::Color32::from_rgb(48, 50, 56);
    v.window_corner_radius = egui::CornerRadius::same(12);
    v.menu_corner_radius = egui::CornerRadius::same(8);
    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
    ] {
        w.corner_radius = egui::CornerRadius::same(6);
    }
    v.widgets.inactive.bg_fill = egui::Color32::from_rgb(44, 48, 54);
    v.widgets.inactive.weak_bg_fill = egui::Color32::from_rgb(44, 48, 54);
    v.widgets.hovered.bg_fill = egui::Color32::from_rgb(78, 86, 96);
    v.widgets.hovered.weak_bg_fill = egui::Color32::from_rgb(78, 86, 96);
    v.widgets.active.bg_fill = ACCENT;
    v.widgets.active.weak_bg_fill = ACCENT;
    v.override_text_color = Some(egui::Color32::from_rgb(248, 250, 252));
    v.widgets.inactive.fg_stroke.color = egui::Color32::from_rgb(240, 244, 248);
    v.widgets.hovered.fg_stroke.color = egui::Color32::from_rgb(248, 252, 255);
    v.widgets.active.fg_stroke.color = egui::Color32::from_rgb(255, 255, 255);
    v.selection.bg_fill = ACCENT;
    v.hyperlink_color = ACCENT;
    v.warn_fg_color = egui::Color32::from_rgb(0xE2, 0x7A, 0x66);
    v.faint_bg_color = egui::Color32::from_rgb(58, 62, 70);
    ctx.set_style(s);
}

// ---------- app shell ----------

impl eframe::App for Sak {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain();
        if self.worker.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        let blocked = self.msgbox.is_some();

        egui::TopBottomPanel::top("header")
            .frame(
                egui::Frame::NONE
                    .fill(egui::Color32::from_rgb(40, 43, 48))
                    .inner_margin(egui::Margin::same(10)),
            )
            .show(ctx, |ui| {
                if blocked {
                    ui.disable();
                }
                self.header(ui);
            });

        egui::TopBottomPanel::bottom("status")
            .frame(
                egui::Frame::NONE
                    .fill(egui::Color32::from_rgb(40, 43, 48))
                    .inner_margin(egui::Margin::same(10)),
            )
            .show(ctx, |ui| {
                if blocked {
                    ui.disable();
                }
                self.status(ui);
            });

        egui::SidePanel::left("nav")
            .resizable(false)
            .exact_width(200.0)
            .frame(
                egui::Frame::NONE
                    .fill(egui::Color32::from_rgb(36, 39, 44))
                    .inner_margin(egui::Margin::same(10)),
            )
            .show(ctx, |ui| {
                if blocked {
                    ui.disable();
                }
                self.nav(ui);
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            if blocked {
                ui.disable();
            }
            let target = ui.available_height();
            egui::ScrollArea::vertical().show(ui, |ui| {
                self.content(ui, target);
            });
        });

        if let Some((title, text)) = self.msgbox.clone() {
            let rect = ctx.screen_rect();
            let painter = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::new("msgbox-dim"),
            ));
            painter.rect_filled(rect, 0.0, egui::Color32::from_black_alpha(78));
            let mut close = false;
            egui::Window::new(title)
                .id(egui::Id::new("msgbox"))
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.set_min_width(400.0);
                    egui::Frame::NONE
                        .fill(ui.visuals().faint_bg_color)
                        .corner_radius(egui::CornerRadius::same(8))
                        .stroke(egui::Stroke::new(1.0_f32, CARD_STROKE))
                        .inner_margin(egui::Margin::same(14))
                        .show(ui, |ui| {
                            ui.add(
                                egui::Label::new(egui::RichText::new(text).monospace().size(13.0))
                                    .wrap(),
                            );
                            ui.add_space(10.0);
                            ui.separator();
                            ui.add_space(6.0);
                            ui.vertical_centered(|ui| {
                                if ui
                                    .add_sized([120.0, 30.0], egui::Button::new(egui::RichText::new("OK").strong().color(egui::Color32::WHITE)).fill(ACCENT))
                                    .clicked()
                                {
                                    close = true;
                                }
                            });
                        });
                });
            if close {
                self.msgbox = None;
            }
        }
    }
}

impl Sak {
    fn header(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if let Some(logo) = &self.logo {
                ui.add(egui::Image::new(logo).fit_to_exact_size(egui::vec2(36.0, 36.0)));
            }
            ui.label(egui::RichText::new("Switch Tools").size(20.0).strong().color(ACCENT));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(egui::RichText::new("by luciano").color(MUTED));
            });
        });
    }

    fn nav(&mut self, ui: &mut egui::Ui) {
        let avail = ui.available_height();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .max_height((avail - 66.0).max(120.0))
            .show(ui, |ui| {
                for (group, items) in nav_items() {
                    ui.add_space(6.0);
                    ui.label(egui::RichText::new(group).small().strong().color(ACCENT));
                    ui.add_space(2.0);
                    for (tool, label) in items {
                        let selected = self.sel == tool;
                        let resp = ui.add_sized(
                            [ui.available_width(), 30.0],
                            egui::Button::selectable(selected, label),
                        );
                        if resp.clicked() && !selected {
                            self.select(tool);
                        }
                    }
                }
            });
        ui.add_space(6.0);
        ui.separator();
        ui.add_space(4.0);
        if ui.small_button("Info").on_hover_text("Sobre o Switch Tools").clicked() {
            self.msgbox = Some(("Info".to_string(), info_text()));
        }
        ui.label(egui::RichText::new("Switch Tools by luciano").size(10.0).color(MUTED));
    }

    fn status(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let running = self.worker.is_some();
            ui.add_enabled(
                running,
                egui::ProgressBar::new(self.pct as f32 / 100.0)
                    .desired_width(320.0)
                    .show_percentage(),
            );
            let text = if self.state_text.is_empty() {
                format!("Ready — {}", label_of(self.sel))
            } else {
                self.state_text.clone()
            };
            ui.label(egui::RichText::new(text).monospace().color(MUTED));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if running && ui.button("Cancel").on_hover_text("Cancel the running operation").clicked()
                {
                    op_cancel();
                }
            });
        });
    }
}

// ---------- content ----------

fn nav_items() -> [(&'static str, Vec<(Tool, &'static str)>); 3] {
    [
        (
            "XCI",
            vec![
                (Tool::PatchXci, "Patch XCI"),
                (Tool::XciSplit, "XCI split"),
                (Tool::XciMerge, "XCI merge"),
                (Tool::UpdateXci, "Update XCI"),
                (Tool::XciToNsp, "XCI to NSP"),
                (Tool::CompressXci, "XCI to XCZ"),
                (Tool::DecompressXcz, "XCZ to XCI"),
                (Tool::ExtractFw, "Extract FW"),
            ],
        ),
        (
            "NSP",
            vec![
                (Tool::PatchNsp, "Patch NSP"),
                (Tool::NspSplit, "NSP split"),
                (Tool::NspMerge, "NSP merge"),
                (Tool::UpdateNsp, "Update NSP"),
                (Tool::NspToXci, "NSP to XCI"),
                (Tool::CompressNsp, "NSP to NSZ"),
                (Tool::DecompressNsz, "NSZ to NSP"),
            ],
        ),
        (
            "MISC",
            vec![
                (Tool::Fat32, "Fat32 Format"),
                (Tool::ExtractSave, "Extract Save"),
                (Tool::Nro, "NRO forwarder"),
            ],
        ),
    ]
}

fn group_of(t: Tool) -> &'static str {
    match t {
        Tool::XciSplit
        | Tool::XciMerge
        | Tool::UpdateXci
        | Tool::XciToNsp
        | Tool::PatchXci
        | Tool::ExtractFw
        | Tool::CompressXci
        | Tool::DecompressXcz => "XCI",
        Tool::PatchNsp
        | Tool::NspSplit
        | Tool::NspMerge
        | Tool::UpdateNsp
        | Tool::NspToXci
        | Tool::CompressNsp
        | Tool::DecompressNsz => "NSP",
        Tool::Fat32 | Tool::ExtractSave | Tool::Nro => "MISC",
    }
}

fn label_of(t: Tool) -> &'static str {
    for (_, items) in nav_items() {
        for (tool, label) in items {
            if tool == t {
                return label;
            }
        }
    }
    ""
}

impl Sak {
    fn content(&mut self, ui: &mut egui::Ui, target_h: f32) {
        ui.add_space(12.0);
        let avail0 = ui.available_size_before_wrap().y;
        match self.sel {
            Tool::PatchXci => self.simple_form(
                ui,
                "Patch XCI",
                "Patches an XCI to run on lower firmware.",
                "patch-xci",
                ("XCI", &["xci"][..]),
                "Patch XCI",
            ),
            Tool::PatchNsp => self.simple_form(
                ui,
                "Patch NSP",
                "Patches an NSP to run on lower firmware.",
                "patch-nsp",
                ("NSP", &["nsp"][..]),
                "Patch NSP",
            ),
            Tool::NspToXci => self.simple_form(
                ui,
                "NSP to XCI",
                "Converts an NSP into an XCI file.",
                "nsp-to-xci",
                ("NSP", &["nsp"][..]),
                "Convert NSP",
            ),
            Tool::XciSplit => self.split_form(ui, true),
            Tool::NspSplit => self.split_form(ui, false),
            Tool::XciMerge | Tool::NspMerge => self.merge_form(ui),
            Tool::UpdateXci | Tool::UpdateNsp => self.update_form(ui),
            Tool::XciToNsp => self.xci_to_nsp_form(ui),
            Tool::CompressXci | Tool::CompressNsp => self.compress_form(ui),
            Tool::DecompressXcz | Tool::DecompressNsz => self.decompress_form(ui),
            Tool::ExtractFw => self.extract_fw_form(ui),
            Tool::ExtractSave => self.extract_save_form(ui),
            Tool::Fat32 => self.fat32_form(ui),
            Tool::Nro => self.nro_form(ui),
        }
        let form_h = avail0 - ui.available_size_before_wrap().y;
        ui.add_space(10.0);
        let log_h = (target_h - form_h - 36.0).max(150.0);
        self.log_card(ui, log_h);
        ui.add_space(12.0);
    }

    fn title(&self, ui: &mut egui::Ui, name: &str, desc: &str) {
        egui::Frame::NONE
            .fill(ACCENT.gamma_multiply(0.16))
            .corner_radius(egui::CornerRadius::same(4))
            .inner_margin(egui::Margin::symmetric(6, 2))
            .show(ui, |ui| {
                ui.label(
                    egui::RichText::new(group_of(self.sel)).size(11.0).strong().color(ACCENT),
                );
            });
        ui.add_space(6.0);
        ui.label(egui::RichText::new(name).size(23.0).strong());
        ui.label(egui::RichText::new(desc).size(14.0).color(MUTED));
        ui.add_space(10.0);
    }

    fn files_card(
        &mut self,
        ui: &mut egui::Ui,
        caption: &str,
        filter: (&str, &[&str]),
        multiple: bool,
        slot: Slot,
    ) {
        egui::Frame::NONE
            .fill(ui.visuals().faint_bg_color)
            .corner_radius(egui::CornerRadius::same(10))
            .inner_margin(egui::Margin::same(12))
            .stroke(egui::Stroke::new(1.0_f32, CARD_STROKE))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(caption).strong().size(14.0).color(ACCENT));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let label = if multiple { "Choose files…" } else { "Choose file…" };
                        if ui.button(label).clicked() {
                            let files = pick(filter, multiple);
                            if !files.is_empty() {
                                match slot {
                                    Slot::Main => self.form.files = files,
                                    Slot::Updates => self.form.updates = files,
                                }
                            }
                        }
                    });
                });
                ui.add_space(4.0);
                let list = match slot {
                    Slot::Main => self.form.files.clone(),
                    Slot::Updates => self.form.updates.clone(),
                };
                if list.is_empty() {
                    ui.add_space(2.0);
                    ui.label(egui::RichText::new("No file selected").color(MUTED).italics());
                    ui.add_space(2.0);
                } else {
                    for f in list {
                        egui::Frame::NONE
                            .fill(ROW_BG)
                            .corner_radius(egui::CornerRadius::same(6))
                            .inner_margin(egui::Margin::symmetric(8, 4))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(egui::RichText::new(short_path(&f)).monospace());
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if ui.small_button("✕").on_hover_text("Remove").clicked()
                                            {
                                                match slot {
                                                    Slot::Main => {
                                                        self.form.files.retain(|x| x != &f)
                                                    }
                                                    Slot::Updates => {
                                                        self.form.updates.retain(|x| x != &f)
                                                    }
                                                }
                                            }
                                            if let Some(sz) = file_size(&f) {
                                                ui.label(
                                                    egui::RichText::new(sz)
                                                        .monospace()
                                                        .size(12.0)
                                                        .color(MUTED),
                                                );
                                            }
                                        },
                                    );
                                });
                            });
                    }
                }
            });
        ui.add_space(8.0);
    }

    fn run_row(&mut self, ui: &mut egui::Ui, label: &str, ready: bool) -> bool {
        let mut clicked = false;
        ui.horizontal(|ui| {
            let b = egui::Button::new(egui::RichText::new(label).strong().color(egui::Color32::WHITE))
                .fill(ACCENT)
                .min_size(egui::vec2(180.0, 36.0));
            if ui
                .add_enabled(ready && self.worker.is_none(), b)
                .on_hover_text(label)
                .clicked()
            {
                clicked = true;
            }
            let hint = if !ready {
                Some("Select the required files first")
            } else if self.worker.is_some() {
                Some("Working…")
            } else {
                None
            };
            if let Some(hint) = hint {
                ui.label(egui::RichText::new(hint).size(13.0).color(MUTED));
            }
        });
        clicked
    }

    fn usb_block(&mut self, ui: &mut egui::Ui, paths: &[&str]) {
        ui.checkbox(&mut self.form.usb_on, "Direct split to USB")
            .on_hover_text("Writes the split parts straight to the USB device");
        if self.form.usb_on {
            ui.horizontal(|ui| {
                if ui.small_button("Refresh").on_hover_text("Refresh connected USB devices").clicked()
                {
                    self.refresh_usb();
                }
                combo(ui, "usb-dev", &self.usb_items(), &mut self.form.usb_sel);
            });
            let items: Vec<String> = paths.iter().map(|s| s.to_string()).collect();
            combo(ui, "usb-path", &items, &mut self.form.path_sel);
        }
    }

    fn log_card(&self, ui: &mut egui::Ui, height: f32) {
        egui::Frame::NONE
            .fill(ui.visuals().faint_bg_color)
            .corner_radius(egui::CornerRadius::same(10))
            .inner_margin(egui::Margin::same(12))
            .stroke(egui::Stroke::new(1.0_f32, CARD_STROKE))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Output").strong().size(14.0).color(ACCENT));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if !self.log.is_empty() {
                            ui.label(
                                egui::RichText::new(format!("{} lines", self.log.len()))
                                    .color(MUTED),
                            );
                        }
                    });
                });
                ui.add_space(6.0);
                let inner_h = (height - 70.0).max(80.0);
                egui::ScrollArea::vertical()
                    .max_height(inner_h)
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        ui.set_min_height(inner_h - 10.0);
                        if self.log.is_empty() {
                            ui.vertical_centered(|ui| {
                                ui.add_space((inner_h * 0.3).max(16.0));
                                ui.label(
                                    egui::RichText::new("Output will appear here…")
                                        .color(MUTED)
                                        .italics(),
                                );
                            });
                        } else {
                            for line in &self.log {
                                let rich =
                                    egui::RichText::new(line.clone()).monospace().size(13.0);
                                if line.contains("aborted")
                                    || line.contains("missing")
                                    || line.contains("couldn't find")
                                    || line.contains("Unknown feature")
                                {
                                    ui.label(rich.color(ui.visuals().warn_fg_color));
                                } else {
                                    ui.label(rich);
                                }
                            }
                        }
                    });
            });
    }

    // ---------- tool forms ----------

    fn simple_form(
        &mut self,
        ui: &mut egui::Ui,
        name: &str,
        desc: &str,
        feature: &str,
        filter: (&str, &[&str]),
        run_label: &str,
    ) {
        self.title(ui, name, desc);
        self.files_card(ui, "Input file", filter, false, Slot::Main);
        if self.run_row(ui, run_label, !self.form.files.is_empty()) {
            self.start(feature, self.form.files.clone(), serde_json::json!({}));
        }
    }

    fn split_form(&mut self, ui: &mut egui::Ui, is_xci: bool) {
        let name = if is_xci { "XCI split" } else { "NSP split" };
        let desc = if is_xci {
            "Splits an XCI into parts below 4 GiB (FAT32 limit)."
        } else {
            "Splits an NSP into parts below 4 GiB (FAT32 limit)."
        };
        self.title(ui, name, desc);
        let filter = if is_xci { ("XCI", &["xci"][..]) } else { ("NSP", &["nsp"][..]) };
        self.files_card(ui, "Input files", filter, true, Slot::Main);
        egui::Frame::NONE
            .fill(ui.visuals().faint_bg_color)
            .corner_radius(egui::CornerRadius::same(10))
            .inner_margin(egui::Margin::same(12))
            .stroke(egui::Stroke::new(1.0_f32, CARD_STROKE))
            .show(ui, |ui| {
                ui.label(
                    egui::RichText::new("Split options").strong().size(14.0).color(ACCENT),
                );
                ui.add_space(6.0);
                if is_xci {
                    ui.checkbox(&mut self.form.trim, "Trim XCI empty space")
                        .on_hover_text("Trims the empty space at the end of the XCI file");
                }
                let paths = ["SD root", "switch\\xci", "sxos\\xci"];
                let paths_nsp = ["nsp", "switch\\nsp", "sxos\\nsp"];
                self.usb_block(ui, if is_xci { &paths } else { &paths_nsp });
            });
        ui.add_space(8.0);
        let ready = !self.form.files.is_empty();
        if self.run_row(ui, "Split files", ready) {
            let req = SplitRequest {
                inputs: self.form.files.clone(),
                dest: self.usb_dest(if is_xci { &["SD root", "switch\\xci", "sxos\\xci"] } else {
                    &["nsp", "switch\\nsp", "sxos\\nsp"]
                }),
                trim: self.form.trim,
            };
            let title = format!("{} splitting...", if is_xci { "XCI" } else { "NSP" });
            self.start_split(req, &title);
        }
    }

    fn merge_form(&mut self, ui: &mut egui::Ui) {
        let is_xci = self.sel == Tool::XciMerge;
        let name = if is_xci { "XCI merge" } else { "NSP merge" };
        let desc = "Merges the split parts back into one file.";
        self.title(ui, name, desc);
        let caption = if is_xci { "First part (.xc0 or .00)" } else { "First part (00)" };
        let filter = if is_xci { ("Parts", &["xc0", "00"][..]) } else { ("Parts", &["00"][..]) };
        self.files_card(ui, caption, filter, false, Slot::Main);
        let ready = !self.form.files.is_empty();
        if self.run_row(ui, "Merge parts", ready) {
            let req = MergeRequest {
                input: self.form.files[0].clone(),
                kind: if is_xci { "xci" } else { "nsp" }.to_string(),
            };
            let title = format!("{} merging...", if is_xci { "XCI" } else { "NSP" });
            self.start_merge(req, &title);
        }
    }

    fn update_form(&mut self, ui: &mut egui::Ui) {
        let is_xci = self.sel == Tool::UpdateXci;
        let name = if is_xci { "Update XCI" } else { "Update NSP" };
        let desc = "Applies update and/or DLC NSPs onto a base file.";
        self.title(ui, name, desc);
        let caption = if is_xci { "Base XCI" } else { "Base NSP" };
        let filter = if is_xci { ("XCI", &["xci"][..]) } else { ("NSP", &["nsp"][..]) };
        self.files_card(ui, caption, filter, false, Slot::Main);
        self.files_card(
            ui,
            "Update / DLC files",
            ("NSP/NSZ", &["nsp", "nsz"][..]),
            true,
            Slot::Updates,
        );
        let ready = !self.form.files.is_empty() && !self.form.updates.is_empty();
        if self.run_row(ui, "Update file", ready) {
            let mut inputs = self.form.files.clone();
            inputs.extend(self.form.updates.clone());
            let feature = if is_xci { "update-xci" } else { "update-nsp" };
            self.start(feature, inputs, serde_json::json!({}));
        }
    }

    fn xci_to_nsp_form(&mut self, ui: &mut egui::Ui) {
        self.title(ui, "XCI to NSP", "Converts an XCI into an NSP file.");
        self.files_card(ui, "Input XCI", ("XCI", &["xci"][..]), false, Slot::Main);
        egui::Frame::NONE
            .fill(ui.visuals().faint_bg_color)
            .corner_radius(egui::CornerRadius::same(10))
            .inner_margin(egui::Margin::same(12))
            .stroke(egui::Stroke::new(1.0_f32, CARD_STROKE))
            .show(ui, |ui| {
                ui.label(
                    egui::RichText::new("Convert options").strong().size(14.0).color(ACCENT),
                );
                ui.add_space(6.0);
                ui.checkbox(&mut self.form.split_chk, "Split NSP file for FAT32")
                    .on_hover_text("Split NSP file after converting");
                if !self.form.split_chk {
                    self.form.usb_on = false;
                }
                if self.form.split_chk {
                    self.usb_block(ui, &["nsp", "switch\\nsp", "sxos\\nsp"]);
                }
            });
        ui.add_space(8.0);
        let ready = !self.form.files.is_empty();
        if self.run_row(ui, "Convert to NSP", ready) {
            let opts = serde_json::json!({
                "split": self.form.split_chk,
                "dest": if self.form.usb_on { self.usb_dest(&["nsp", "switch\\nsp", "sxos\\nsp"]) } else { None::<String> },
            });
            self.start("xci-to-nsp", self.form.files.clone(), opts);
        }
    }

    fn compress_form(&mut self, ui: &mut egui::Ui) {
        let is_xci = self.sel == Tool::CompressXci;
        let src = if is_xci { "XCI" } else { "NSP" };
        let dst = if is_xci { "XCZ" } else { "NSZ" };
        self.title(
            ui,
            &format!("{} to {}", src, dst),
            &format!("Compresses {} files to {}.", src, dst),
        );
        let filter = if is_xci { ("XCI", &["xci"][..]) } else { ("NSP", &["nsp"][..]) };
        self.files_card(ui, "Input files", filter, true, Slot::Main);
        egui::Frame::NONE
            .fill(ui.visuals().faint_bg_color)
            .corner_radius(egui::CornerRadius::same(10))
            .inner_margin(egui::Margin::same(12))
            .stroke(egui::Stroke::new(1.0_f32, CARD_STROKE))
            .show(ui, |ui| {
                ui.label(
                    egui::RichText::new("Compression level").strong().size(14.0).color(ACCENT),
                );
                ui.add_space(6.0);
                let items: Vec<String> = LEVELS.iter().map(|s| s.to_string()).collect();
                combo(ui, "compress-level", &items, &mut self.form.level_sel);
            });
        ui.add_space(8.0);
        let ready = !self.form.files.is_empty();
        let out_ext = dst.to_lowercase();
        let conflicts = self.output_conflicts(&out_ext);
        let mut start_now = false;
        if self.run_row(ui, &format!("Compress to {}", dst), ready) {
            if conflicts.is_empty() {
                start_now = true;
            } else {
                self.form.ow_confirm = true;
            }
        }
        if self.overwrite_card(ui, &conflicts, &out_ext) {
            start_now = true;
        }
        if start_now {
            self.form.ow_confirm = false;
            let opts = serde_json::json!({
                "level": LEVELS[self.form.level_sel],
                "ext": out_ext,
            });
            self.start("compress", self.form.files.clone(), opts);
        }
    }

    fn decompress_form(&mut self, ui: &mut egui::Ui) {
        let is_xcz = self.sel == Tool::DecompressXcz;
        let src = if is_xcz { "XCZ" } else { "NSZ" };
        let dst = if is_xcz { "XCI" } else { "NSP" };
        self.title(
            ui,
            &format!("{} to {}", src, dst),
            &format!("Decompresses {} files back to {}.", src, dst),
        );
        let filter = if is_xcz { ("XCZ", &["xcz"][..]) } else { ("NSZ", &["nsz"][..]) };
        self.files_card(ui, "Input files", filter, true, Slot::Main);
        let ready = !self.form.files.is_empty();
        // Decompress writes its output next to each input, with the same base
        // name — flag existing outputs so they never get clobbered silently.
        let out_ext = if is_xcz { "xci" } else { "nsp" };
        let conflicts = self.output_conflicts(&out_ext);
        let mut start_now = false;
        if self.run_row(ui, &format!("Decompress to {}", dst), ready) {
            if conflicts.is_empty() {
                start_now = true;
            } else {
                self.form.ow_confirm = true;
            }
        }
        if self.overwrite_card(ui, &conflicts, &out_ext) {
            start_now = true;
        }
        if start_now {
            self.form.ow_confirm = false;
            self.start("decompress", self.form.files.clone(), serde_json::json!({}));
        }
    }

    /// Outputs the decompress/compress step would write next to each input,
    /// matching base names that already exist on disk.
    fn output_conflicts(&self, out_ext: &str) -> Vec<String> {
        self.form
            .files
            .iter()
            .filter(|f| Path::new(*f).with_extension(out_ext).exists())
            .cloned()
            .collect()
    }

    /// Confirmation card listing colliding outputs; returns true when the user
    /// chose to overwrite.
    fn overwrite_card(&mut self, ui: &mut egui::Ui, conflicts: &[String], out_ext: &str) -> bool {
        let mut run = false;
        if self.form.ow_confirm && !conflicts.is_empty() {
            ui.add_space(8.0);
            egui::Frame::NONE
                .fill(ui.visuals().faint_bg_color)
                .corner_radius(egui::CornerRadius::same(10))
                .inner_margin(egui::Margin::same(12))
                .stroke(egui::Stroke::new(1.0_f32, CARD_STROKE))
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new("Confirm").strong().size(14.0).color(ACCENT),
                    );
                    ui.add_space(6.0);
                    ui.colored_label(
                        ui.visuals().warn_fg_color,
                        "Output file(s) already exist and will be replaced:",
                    );
                    ui.add_space(4.0);
                    for c in conflicts {
                        ui.monospace(
                            Path::new(c)
                                .with_extension(out_ext)
                                .display()
                                .to_string(),
                        );
                    }
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        if ui.button("Overwrite and continue").clicked() {
                            self.form.ow_confirm = false;
                            run = true;
                        }
                        if ui.button("Cancel").clicked() {
                            self.form.ow_confirm = false;
                        }
                    });
                });
        }
        run
    }

    fn extract_fw_form(&mut self, ui: &mut egui::Ui) {
        self.title(
            ui,
            "Extract FW",
            "Extracts the firmware update from an XCI file.",
        );
        self.files_card(ui, "Input XCI", ("XCI", &["xci"][..]), false, Slot::Main);
        let ready = !self.form.files.is_empty();
        if self.run_row(ui, "Extract firmware", ready) {
            self.start("extract-fw", self.form.files.clone(), serde_json::json!({}));
        }
    }

    fn extract_save_form(&mut self, ui: &mut egui::Ui) {
        self.title(
            ui,
            "Extract Save",
            "Extracts savegames from a eMMC / User partition dump (into tmpout/).",
        );
        self.files_card(ui, "Save files", ("Save", &["sav", "bin", "dat"][..]), true, Slot::Main);
        let ready = !self.form.files.is_empty();
        if self.run_row(ui, "Extract saves", ready) {
            self.start(
                "extract-save",
                self.form.files.clone(),
                serde_json::json!({ "outdir": "tmpout" }),
            );
        }
    }

    fn fat32_form(&mut self, ui: &mut egui::Ui) {
        self.title(
            ui,
            "Fat32 Format",
            "Formats a USB device to FAT32 (all sizes). ERASES ALL DATA on it.",
        );
        egui::Frame::NONE
            .fill(ui.visuals().faint_bg_color)
            .corner_radius(egui::CornerRadius::same(10))
            .inner_margin(egui::Margin::same(12))
            .stroke(egui::Stroke::new(1.0_f32, CARD_STROKE))
            .show(ui, |ui| {
                ui.label(
                    egui::RichText::new("Device").strong().size(14.0).color(ACCENT),
                );
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui
                        .button("Refresh")
                        .on_hover_text("Refresh connected USB devices")
                        .clicked()
                    {
                        self.refresh_usb();
                    }
                    combo(ui, "fmt-dev", &self.usb_items(), &mut self.form.fmt_sel);
                });
                ui.add_space(4.0);
                let valid = !self.usb.is_empty() && !self.form.fmt_confirm;
                if ui
                    .add_enabled(valid, egui::Button::new("Format device…"))
                    .clicked()
                {
                    self.form.fmt_confirm = true;
                }
            });
        ui.add_space(8.0);
        if self.form.fmt_confirm {
            let dev = self.usb.get(self.form.fmt_sel).map(|d| {
                (
                    d.mountpoint.clone(),
                    format!("{} ({:.1}GB)", d.label, d.free_mb as f64 / 1024.0),
                )
            });
            if let Some((mountpoint, display)) = dev {
                egui::Frame::NONE
                    .fill(ui.visuals().faint_bg_color)
                    .corner_radius(egui::CornerRadius::same(10))
                    .inner_margin(egui::Margin::same(12))
                    .show(ui, |ui| {
                        ui.label(
                            egui::RichText::new("Confirm").strong().size(14.0).color(ACCENT),
                        );
                        ui.add_space(6.0);
                        ui.colored_label(
                            ui.visuals().warn_fg_color,
                            format!("ERASE ALL DATA on {}?", display),
                        );
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            if ui.button("Format it!").clicked() {
                                let req =
                                    Fat32Request { mountpoint, label: display };
                                app_log_reset();
                                app_log(&format!(
                                    "=== fat32-format started on {} (label: {})",
                                    req.mountpoint, req.label
                                ));
                                self.form.fmt_confirm = false;
                                self.log.clear();
                                self.run_title = "fat32-format".to_string();
                                self.spawn_op(move |_tx| fat32_format(req));
                            }
                            if ui.button("Cancel").clicked() {
                                self.form.fmt_confirm = false;
                            }
                        });
                    });
                ui.add_space(8.0);
            }
        }
    }

    fn nro_form(&mut self, ui: &mut egui::Ui) {
        self.title(
            ui,
            "NRO forwarder",
            "Creates a forwarder NSP that boots an NRO (homebrew or RetroArch core).",
        );
        egui::Frame::NONE
            .fill(ui.visuals().faint_bg_color)
            .corner_radius(egui::CornerRadius::same(10))
            .inner_margin(egui::Margin::same(12))
            .stroke(egui::Stroke::new(1.0_f32, CARD_STROKE))
            .show(ui, |ui| {
                ui.label(egui::RichText::new("Metadata").strong().size(14.0).color(ACCENT));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label("App name");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.form.appname).desired_width(240.0),
                    );
                });
                ui.horizontal(|ui| {
                    ui.label("Author");
                    ui.add(egui::TextEdit::singleline(&mut self.form.author).desired_width(240.0));
                });
                ui.horizontal(|ui| {
                    ui.label("Version");
                    ui.add(egui::TextEdit::singleline(&mut self.form.version).desired_width(240.0));
                });
                ui.horizontal(|ui| {
                    ui.label("Title ID");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.form.titleid).desired_width(240.0),
                    );
                    if ui.small_button("Generate").clicked() {
                        self.form.titleid = generate_titleid();
                    }
                });
            });
        ui.add_space(8.0);
        egui::Frame::NONE
            .fill(ui.visuals().faint_bg_color)
            .corner_radius(egui::CornerRadius::same(10))
            .inner_margin(egui::Margin::same(12))
            .stroke(egui::Stroke::new(1.0_f32, CARD_STROKE))
            .show(ui, |ui| {
                ui.label(egui::RichText::new("Icon & options").strong().size(14.0).color(ACCENT));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label("Icon (256×256 JPG)");
                    if ui.button("Choose…").clicked() {
                        let files = pick(("JPG", &["jpg", "jpeg"][..]), false);
                        if !files.is_empty() {
                            self.form.icon = Some(files[0].clone());
                        }
                    }
                    if let Some(icon) = &self.form.icon {
                        ui.label(egui::RichText::new(short_path(icon)).monospace().color(MUTED));
                    }
                });
                ui.checkbox(&mut self.form.shots, "Allow screenshots");
                ui.checkbox(&mut self.form.video, "Allow video capture");
                ui.checkbox(&mut self.form.nologo, "No startup logo")
                    .on_hover_text("No startup logo, only a black screen at startup");
            });
        ui.add_space(8.0);
        egui::Frame::NONE
            .fill(ui.visuals().faint_bg_color)
            .corner_radius(egui::CornerRadius::same(10))
            .inner_margin(egui::Margin::same(12))
            .stroke(egui::Stroke::new(1.0_f32, CARD_STROKE))
            .show(ui, |ui| {
                ui.label(egui::RichText::new("Target").strong().size(14.0).color(ACCENT));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui
                        .selectable_label(self.form.core == Some(false), "Standard NRO")
                        .on_hover_text("Create a standard NRO forwarder")
                        .clicked()
                    {
                        self.form.core = Some(false);
                        self.form.nro_path = "/switch/App-Dir/App.nro".into();
                    }
                    if ui
                        .selectable_label(self.form.core == Some(true), "RetroArch core")
                        .on_hover_text("Create a RetroArch core forwarder")
                        .clicked()
                    {
                        self.form.core = Some(true);
                        self.form.nro_path = "/retroarch/cores/core.nro".into();
                    }
                });
                ui.horizontal(|ui| {
                    ui.label(if self.form.core == Some(true) { "Core path" } else { "NRO path" });
                    ui.add(
                        egui::TextEdit::singleline(&mut self.form.nro_path).desired_width(380.0),
                    );
                });
                if self.form.core == Some(true) {
                    ui.horizontal(|ui| {
                        ui.label("Roms path");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.form.rom_path)
                                .desired_width(380.0),
                        );
                    });
                }
            });
        ui.add_space(8.0);
        let ready = self.form.core.is_some();
        if self.run_row(ui, "Create forwarder", ready) {
            let opts = serde_json::json!({
                "appname": self.form.appname,
                "titleid": self.form.titleid,
                "author": self.form.author,
                "version": self.form.version,
                "nologo": self.form.nologo,
                "screenshots": self.form.shots,
                "videocapture": self.form.video,
                "mode": if self.form.core == Some(true) { "core" } else { "nro" },
                "path": self.form.nro_path,
                "rompath": self.form.rom_path,
            });
            let icon = self.form.icon.clone().unwrap_or_default();
            self.start("nro-forwarder", vec![icon], opts);
        }
    }
}

// ---------- shared widgets ----------

fn combo(ui: &mut egui::Ui, id: &str, items: &[String], sel: &mut usize) {
    egui::ComboBox::from_id_salt(id)
        .selected_text(items.get(*sel).cloned().unwrap_or_default())
        .width(140.0)
        .show_ui(ui, |ui| {
            for (i, it) in items.iter().enumerate() {
                ui.selectable_value(sel, i, it);
            }
        });
}

fn pick(filter: (&str, &[&str]), multiple: bool) -> Vec<String> {
    let fd = rfd::FileDialog::new().add_filter(filter.0, filter.1);
    let res = if multiple {
        fd.pick_files()
    } else {
        fd.pick_file().map(|p| vec![p])
    };
    res.map(|v| v.into_iter().map(|p| p.to_string_lossy().into_owned()).collect())
        .unwrap_or_default()
}

fn short_path(p: &str) -> String {
    let name = std::path::Path::new(p).file_name().map(|n| n.to_string_lossy().into_owned());
    name.unwrap_or_else(|| p.to_string())
}

fn file_size(path: &str) -> Option<String> {
    let bytes = std::fs::metadata(path).ok()?.len();
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    if bytes as f64 >= GB {
        Some(format!("{:.2} GB", bytes as f64 / GB))
    } else {
        Some(format!("{:.1} MB", bytes as f64 / MB))
    }
}

fn fmt_elapsed(d: Duration) -> String {
    let millis = d.as_millis();
    if millis < 1_000 {
        format!("{} ms", millis)
    } else {
        let secs = (millis / 1_000) as u64;
        if secs >= 60 {
            format!("{} minutes {} seconds", secs / 60, secs % 60)
        } else {
            // Keep one decimal for short runs so sub-second-cost ops don't read "0 seconds".
            if millis < 10_000 {
                format!("{:.1} seconds", millis as f64 / 1_000.0)
            } else {
                format!("{} seconds", secs)
            }
        }
    }
}
