//! The window: Update / Check / Restore / Set up, a large pane with progress and a plain
//! description of what is saved, and the Set up page with the scan results.
use crate::lang::fmt_size;
use crate::out::{Msg, Out};
use crate::scan::ScanResult;
use crate::settings::{app_dir, item_title, settings_path, Dest, Item, Settings, GROUPS, INV_IDS, KIT_FOLDER};
use crate::{check, lang, restore, scan, sys, tr, update};
use chrono::{DateTime, Local};
use eframe::egui::{self, Align, Color32, FontFamily, FontId, Layout, Margin, RichText, Rounding, Stroke, TextStyle};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::collections::HashMap;
use std::path::Path;
use std::sync::mpsc::{channel, Receiver};

const ACCENT: Color32 = Color32::from_rgb(127, 212, 255);
const TEXT: Color32 = Color32::from_rgb(242, 245, 248);
const DIM: Color32 = Color32::from_rgb(178, 186, 196);
const WARN: Color32 = Color32::from_rgb(255, 201, 138);
const GOOD: Color32 = Color32::from_rgb(142, 230, 168);

#[derive(PartialEq)]
enum Page {
    Main,
    Wizard,
}

struct Row {
    group: String,
    item: Option<Item>,
    inv_id: Option<String>,
    title: String,
    sub: String,
    on: bool,
    enabled: bool,
    bytes: u64,
    partial: bool,
}

struct RestoreDlg {
    kit: String,
    on: [bool; 10],
}

struct App {
    settings: Option<Settings>,
    own_kit: Option<String>,
    glass: bool,
    page: Page,
    title: String,
    sub: String,
    // right pane
    show_log: bool,
    pane_title: String,
    log: String,
    about: String,
    running: bool,
    prog: (usize, usize),
    prog_text: String,
    rx: Option<Receiver<Msg>>,
    // set up page
    scanning: bool,
    scan_failed: bool,
    wiz_status: String,
    rows: Vec<Row>,
    drives: HashMap<String, sys::Drive>,
    projects_inside: usize,
    dest1: String,
    dest2: String,
    secrets_enc: bool,
    days: String,
    time: String,
    schedule: bool,
    notify: bool,
    wiz_lang: String,
    restore_dlg: Option<RestoreDlg>,
    // tests
    page_arg: String,
    selftest: Option<String>,
    first_frame: bool,
}

pub fn run(args: Vec<String>) {
    let mut vp = egui::ViewportBuilder::default()
        .with_title("Reinstall Kit")
        .with_inner_size([1180.0, 760.0])
        .with_min_inner_size([980.0, 640.0])
        .with_transparent(true);
    if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png")) {
        vp = vp.with_icon(icon);
    }
    let opts = eframe::NativeOptions { viewport: vp, centered: true, ..Default::default() };
    let _ = eframe::run_native("Reinstall Kit", opts, Box::new(move |cc| Ok(Box::new(App::new(cc, args)))));
}

/// Plain translucent white:  out of 255 is how much lighter the surface gets (egui's own helpers brighten far more).
fn white(a: u8) -> Color32 {
    Color32::from_rgba_premultiplied(a, a, a, a)
}

fn family(name: &str) -> FontFamily {
    FontFamily::Name(name.into())
}

/// Uses the fonts Windows itself uses, so the window looks at home: Segoe UI, Segoe Fluent Icons, Cascadia Mono.
fn setup_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let dir = Path::new(&std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into())).join("Fonts");
    let mut load = |name: &str, files: &[&str]| -> bool {
        for f in files {
            if let Ok(bytes) = std::fs::read(dir.join(f)) {
                fonts.font_data.insert(name.to_string(), egui::FontData::from_owned(bytes));
                return true;
            }
        }
        false
    };
    let ui_font = load("ui", &["segoeui.ttf"]);
    let semi = load("semi", &["seguisb.ttf", "segoeuib.ttf"]);
    let icons = load("icons", &["SegoeIcons.ttf", "segmdl2.ttf"]);
    let mono = load("mono", &["CascadiaMono.ttf", "consola.ttf"]);
    let fallback: Vec<String> = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let with = |first: Option<&str>| {
        let mut v: Vec<String> = first.map(|s| vec![s.to_string()]).unwrap_or_default();
        if ui_font {
            v.push("ui".into());
        }
        v.extend(fallback.iter().cloned());
        v
    };
    fonts.families.insert(FontFamily::Proportional, with(None));
    fonts.families.insert(family("semi"), with(if semi { Some("semi") } else { None }));
    fonts.families.insert(family("icons"), with(if icons { Some("icons") } else { None }));
    let mut m = with(if mono { Some("mono") } else { None });
    m.extend(fonts.families.get(&FontFamily::Monospace).cloned().unwrap_or_default());
    fonts.families.insert(FontFamily::Monospace, m);
    ctx.set_fonts(fonts);
}

fn setup_style(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    style.text_styles = [
        (TextStyle::Heading, FontId::new(28.0, family("semi"))),
        (TextStyle::Body, FontId::new(14.5, FontFamily::Proportional)),
        (TextStyle::Button, FontId::new(14.0, FontFamily::Proportional)),
        (TextStyle::Monospace, FontId::new(13.0, FontFamily::Monospace)),
        (TextStyle::Small, FontId::new(12.0, FontFamily::Proportional)),
    ]
    .into();
    style.interaction.selectable_labels = false;
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(14.0, 7.0);
    style.spacing.icon_width = 20.0;
    style.spacing.icon_spacing = 10.0;
    style.spacing.scroll.bar_width = 8.0;
    let v = &mut style.visuals;
    *v = egui::Visuals::dark();
    v.override_text_color = Some(TEXT);
    v.panel_fill = Color32::TRANSPARENT;
    v.window_fill = Color32::from_rgb(27, 34, 43);
    v.window_rounding = Rounding::same(14.0);
    v.window_stroke = Stroke::new(1.0_f32, white(40));
    v.extreme_bg_color = Color32::from_black_alpha(90);
    v.selection.bg_fill = Color32::from_rgb(60, 130, 170);
    v.selection.stroke = Stroke::new(1.0_f32, Color32::from_rgb(8, 19, 28));
    for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.noninteractive, &mut v.widgets.open] {
        w.rounding = Rounding::same(8.0);
    }
    v.widgets.inactive.bg_fill = white(28);
    v.widgets.inactive.weak_bg_fill = white(28);
    v.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, white(60));
    v.widgets.hovered.bg_fill = white(56);
    v.widgets.hovered.weak_bg_fill = white(56);
    v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, white(140));
    v.widgets.active.bg_fill = white(80);
    v.widgets.active.weak_bg_fill = white(80);
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, white(34));
    ctx.set_style(style);
}

fn semi(text: impl Into<String>, size: f32) -> RichText {
    RichText::new(text).font(FontId::new(size, family("semi")))
}

fn panel_frame() -> egui::Frame {
    egui::Frame::none()
        .fill(Color32::from_black_alpha(58))
        .stroke(Stroke::new(1.0_f32, white(34)))
        .rounding(Rounding::same(16.0))
        .inner_margin(Margin { left: 20.0, right: 12.0, top: 14.0, bottom: 14.0 })
}

/// A rounded surface whose light fades from the top-left corner to the bottom-right one, like a sheet of glass.
fn glass(rect: egui::Rect, radius: f32, from: u8, to: u8) -> egui::Shape {
    let shade = |p: egui::Pos2| {
        let t = (((p.x - rect.left()) / rect.width()) + ((p.y - rect.top()) / rect.height())) * 0.5;
        white((from as f32 + (to as f32 - from as f32) * t.clamp(0.0, 1.0)) as u8)
    };
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(rect.center(), shade(rect.center()));
    let corners = [
        (rect.right() - radius, rect.top() + radius, -90.0_f32),
        (rect.right() - radius, rect.bottom() - radius, 0.0),
        (rect.left() + radius, rect.bottom() - radius, 90.0),
        (rect.left() + radius, rect.top() + radius, 180.0),
    ];
    let mut n = 0u32;
    for (cx, cy, start) in corners {
        for i in 0..=8 {
            let ang = (start + 90.0 * i as f32 / 8.0).to_radians();
            let p = egui::pos2(cx + radius * ang.cos(), cy + radius * ang.sin());
            mesh.colored_vertex(p, shade(p));
            n += 1;
        }
    }
    for i in 0..n {
        mesh.add_triangle(0, 1 + i, 1 + (i + 1) % n);
    }
    egui::Shape::mesh(mesh)
}

/// A large glass button with an icon, a title and an explanation. Lights up smoothly under the mouse.
fn card(ui: &mut egui::Ui, enabled: bool, icon: char, icon_color: Color32, title: &str, desc: &str) -> bool {
    let slot = ui.painter().add(egui::Shape::Noop);
    let inner = ui.add_enabled_ui(enabled, |ui| {
        egui::Frame::none()
            .inner_margin(Margin::symmetric(18.0, 14.0))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icon.to_string()).font(FontId::new(26.0, family("icons"))).color(icon_color));
                    ui.add_space(10.0);
                    ui.vertical(|ui| {
                        ui.label(semi(title, 17.0));
                        ui.label(RichText::new(desc).size(13.0).color(DIM));
                    });
                });
            })
            .response
    });
    let resp = inner.inner.interact(egui::Sense::click());
    let lit = ui.ctx().animate_bool_with_time(resp.id.with("lit"), enabled && resp.hovered(), 0.18);
    let k = if enabled { 1.0 } else { 0.45 };
    let mix = |a: f32, b: f32| ((a + (b - a) * lit) * k) as u8;
    let press = if enabled && resp.is_pointer_button_down_on() { 0.8 } else { 1.0 };
    let shapes = vec![
        glass(resp.rect, 16.0, (mix(46.0, 78.0) as f32 * press) as u8, (mix(16.0, 36.0) as f32 * press) as u8),
        egui::Shape::rect_stroke(resp.rect, Rounding::same(16.0), Stroke::new(1.0_f32, white(mix(52.0, 140.0)))),
    ];
    ui.painter().set(slot, egui::Shape::Vec(shapes));
    if enabled && resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    ui.add_space(6.0);
    enabled && resp.clicked()
}
/// A check box that is easy to see on the dark glass: a filled accent square with a dark tick.
fn check(ui: &mut egui::Ui, on: &mut bool) -> egui::Response {
    let (rect, mut resp) = ui.allocate_exact_size(egui::vec2(20.0, 20.0), egui::Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let fade = if ui.is_enabled() { 1.0 } else { 0.4 };
    let p = ui.painter();
    if *on {
        p.rect_filled(rect, Rounding::same(5.0), ACCENT.gamma_multiply(fade));
        let c = rect.center();
        let dark = Stroke::new(2.0_f32, Color32::from_rgb(8, 19, 28));
        p.line_segment([egui::pos2(c.x - 5.0, c.y), egui::pos2(c.x - 1.5, c.y + 3.5)], dark);
        p.line_segment([egui::pos2(c.x - 1.5, c.y + 3.5), egui::pos2(c.x + 5.0, c.y - 3.5)], dark);
    } else {
        let edge = if resp.hovered() { 255 } else { 136 };
        p.rect(rect.shrink(0.75), Rounding::same(5.0), white(20), Stroke::new(1.5_f32, white(edge).gamma_multiply(fade)));
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}
fn check_text(ui: &mut egui::Ui, on: &mut bool, text: &str) {
    ui.horizontal(|ui| {
        check(ui, on);
        if ui.add(egui::Label::new(RichText::new(text).size(14.0)).wrap().sense(egui::Sense::click())).clicked() {
            *on = !*on;
        }
    });
}

fn accent_button(ui: &mut egui::Ui, text: &str, enabled: bool) -> bool {
    let b = egui::Button::new(semi(text, 14.5).color(Color32::from_rgb(8, 19, 28))).fill(ACCENT).rounding(Rounding::same(10.0)).min_size(egui::vec2(0.0, 36.0));
    ui.add_enabled(enabled, b).clicked()
}

fn ago(t: std::time::SystemTime) -> String {
    let hours = t.elapsed().map(|e| e.as_secs() / 3600).unwrap_or(0);
    let a = if hours < 1 { tr!("ago.now") } else if hours < 24 { tr!("ago.h", hours) } else { tr!("ago.d", hours / 24) };
    let d = DateTime::<Local>::from(t);
    tr!("ago.fmt", d.format("%d.%m.%Y"), d.format("%H:%M"), a)
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, args: Vec<String>) -> App {
        setup_fonts(&cc.egui_ctx);
        setup_style(&cc.egui_ctx);
        let glass = match cc.window_handle().map(|h| h.as_raw()) {
            Ok(RawWindowHandle::Win32(h)) => sys::apply_glass(h.hwnd.get()),
            _ => false,
        };
        let dir = app_dir();
        let own_kit = match (dir.file_name().map(|n| n.to_string_lossy().to_lowercase()), dir.parent()) {
            (Some(n), Some(up)) if n == "_kit" && up.join("files").is_dir() => Some(up.to_string_lossy().into_owned()),
            _ => None,
        };
        let get = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
        let mut app = App {
            settings: Settings::load(&settings_path()),
            own_kit,
            glass,
            page: Page::Main,
            title: String::new(),
            sub: String::new(),
            show_log: false,
            pane_title: String::new(),
            log: String::new(),
            about: String::new(),
            running: false,
            prog: (0, 0),
            prog_text: String::new(),
            rx: None,
            scanning: false,
            scan_failed: false,
            wiz_status: String::new(),
            rows: vec![],
            drives: HashMap::new(),
            projects_inside: 0,
            dest1: String::new(),
            dest2: String::new(),
            secrets_enc: true,
            days: "30".into(),
            time: "13:00".into(),
            schedule: true,
            notify: true,
            wiz_lang: "auto".into(),
            restore_dlg: None,
            page_arg: get("--page").unwrap_or_default(),
            selftest: get("--selftest"),
            first_frame: true,
        };
        app.refresh();
        app
    }

    // ---- Main page state ---------------------------------------------------------
    fn refresh(&mut self) {
        self.about = self.about_text();
        if let Some(k) = &self.own_kit {
            self.title = tr!("st.kitmode.t");
            self.sub = tr!("st.kitmode.s", k);
            return;
        }
        let dests = self.settings.as_ref().map(|s| s.dests()).unwrap_or_default();
        if dests.is_empty() {
            self.title = tr!("st.none.t");
            self.sub = tr!("st.none.s");
            return;
        }
        let mut fresh = false;
        let mut lines = vec![];
        for d in dests {
            let st = Path::new(&d).join(KIT_FOLDER).join("_updated.txt");
            if !sys::drive_there(&d) {
                lines.push(tr!("st.dest.offline", d));
            } else if let Ok(w) = std::fs::metadata(&st).and_then(|m| m.modified()) {
                if w.elapsed().map(|e| e.as_secs() < 2 * 86400).unwrap_or(true) {
                    fresh = true;
                }
                lines.push(tr!("st.dest.updated", d, ago(w)));
            } else {
                lines.push(tr!("st.dest.never", d));
            }
        }
        self.title = if fresh { tr!("st.fresh.t") } else { tr!("st.stale.t") };
        self.sub = lines.join("\n");
    }

    /// "What is saved": a plain description assembled from the settings file.
    fn about_text(&self) -> String {
        if self.own_kit.is_some() {
            return tr!("about.kitmode");
        }
        let Some(s) = &self.settings else { return tr!("about.first") };
        let mut t = format!("{}\n\n{}\n", tr!("about.intro"), tr!("about.h.what"));
        for g in GROUPS {
            let names: Vec<String> = s.items.iter().filter(|m| m.enabled && m.group == g).map(item_title).collect();
            if names.is_empty() {
                continue;
            }
            t += &format!("•  {}: {}.", tr!(&format!("grp.{}", g)), names.join(", "));
            if (g == "secrets" || g == "browsers") && s.secrets_only_encrypted {
                t += &format!(" {}", tr!("about.secrets.enc"));
            }
            t += "\n";
        }
        let inv: Vec<String> = INV_IDS.iter().filter(|id| s.inventory.get(id)).map(|id| tr!(&format!("inv.{}", id))).collect();
        if !inv.is_empty() {
            t += &format!("•  {}: {}.\n", tr!("grp.system"), inv.join(", "));
        }
        t += &format!("\n{}\n", tr!("about.h.where"));
        for d in s.dests() {
            t += &format!("•  {}\n", Path::new(&d).join(KIT_FOLDER).display());
        }
        t += &format!("\n{}\n{}\n", tr!("about.h.how"), tr!("about.how", s.keep_days()));
        t += &format!("\n{}\n{}\n", tr!("about.h.when"), if s.schedule.enabled { tr!("about.when.on", s.schedule.time) } else { tr!("about.when.off") });
        t += &format!("\n{}\n{}\n", tr!("about.h.not"), tr!("about.not"));
        t += &format!("\n{}", tr!("about.file", settings_path().display()));
        t
    }

    fn start(&mut self, ctx: &egui::Context, job: impl FnOnce(Out) + Send + 'static) {
        let (tx, rx) = channel();
        self.rx = Some(rx);
        self.running = true;
        let out = Out::new(tx, Some(ctx.clone()));
        std::thread::spawn(move || {
            sys::background_thread();
            job(out);
        });
    }
    fn run_job(&mut self, ctx: &egui::Context, title: String, job: impl FnOnce(Out) + Send + 'static) {
        if self.running {
            return;
        }
        self.show_log = true;
        self.pane_title = title;
        self.log.clear();
        self.prog = (0, 0);
        self.prog_text = tr!("run.working");
        self.start(ctx, job);
    }

    fn poll(&mut self, ctx: &egui::Context) {
        let msgs: Vec<Msg> = match &self.rx {
            Some(rx) => rx.try_iter().collect(),
            None => return,
        };
        for m in msgs {
            match m {
                Msg::Line(s) => {
                    self.log.push_str(&s);
                    self.log.push('\n');
                }
                Msg::Progress(i, n, label) => {
                    self.prog = (i.saturating_sub(1), n);
                    self.prog_text = format!("{}   ·   {} / {}", label, i, n);
                }
                Msg::Status(s) => self.wiz_status = format!("{}  {}", tr!("wiz.scanning"), s),
                Msg::Scan(r) => self.build_wizard(*r),
                Msg::Toast(..) => {}
                Msg::Done(code) => {
                    self.running = false;
                    self.rx = None;
                    if self.scanning {
                        self.scanning = false;
                        if self.rows.is_empty() {
                            self.scan_failed = true;
                            self.wiz_status = tr!("wiz.scanfail");
                        } else {
                            self.wiz_status = tr!("wiz.ready");
                        }
                        if let Some(spec) = self.selftest.take() {
                            self.apply_selftest(&spec);
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    } else {
                        self.prog = (1, 1);
                        self.prog_text = if code == 0 { tr!("run.done") } else { tr!("run.done.err") };
                        self.refresh();
                    }
                    break;
                }
            }
        }
    }

    // ---- Restore -----------------------------------------------------------------
    fn find_kit(&self) -> Option<String> {
        if let Some(k) = &self.own_kit {
            return Some(k.clone());
        }
        self.settings.as_ref()?.dests().into_iter().map(|d| Path::new(&d).join(KIT_FOLDER)).find(|k| k.join("files").is_dir()).map(|k| k.to_string_lossy().into_owned())
    }
    fn open_restore(&mut self) {
        let kit = self.find_kit().or_else(|| {
            let mut p = rfd::FileDialog::new().set_title(tr!("rst.pick.kit")).pick_folder()?;
            if !p.join("files").is_dir() && p.join(KIT_FOLDER).join("files").is_dir() {
                p = p.join(KIT_FOLDER);
            }
            if !p.join("files").is_dir() {
                rfd::MessageDialog::new().set_level(rfd::MessageLevel::Warning).set_title("Reinstall Kit").set_description(tr!("rst.err.nokit", p.display())).show();
                return None;
            }
            Some(p.to_string_lossy().into_owned())
        });
        if let Some(kit) = kit {
            let mut on = [true; 10];
            on[8] = false; // Wi-Fi and drivers are optional: off unless asked for
            on[9] = false;
            self.restore_dlg = Some(RestoreDlg { kit, on });
        }
    }
    fn restore_window(&mut self, ctx: &egui::Context) {
        let Some(dlg) = &mut self.restore_dlg else { return };
        let mut action = 0;
        egui::Window::new(semi(tr!("main.restore.h"), 18.0))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .frame(egui::Frame::window(&ctx.style()).inner_margin(Margin::same(22.0)))
            .show(ctx, |ui| {
                ui.set_width(520.0);
                ui.label(RichText::new(tr!("rst.dlg.text")).size(14.0));
                ui.add_space(4.0);
                ui.label(RichText::new(tr!("rst.dlg.kit", dlg.kit)).size(13.0).color(DIM));
                ui.add_space(8.0);
                for (i, s) in restore::STEPS.iter().enumerate() {
                    check_text(ui, &mut dlg.on[i], &tr!(&format!("rst.step.{}", s)));
                }
                ui.add_space(12.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if accent_button(ui, &tr!("rst.dlg.dry"), true) {
                        action = 1;
                    }
                    if ui.button(tr!("rst.dlg.real")).clicked() {
                        action = 2;
                    }
                    if ui.button(tr!("wiz.cancel")).clicked() {
                        action = 3;
                    }
                });
            });
        if action == 2 {
            let yes = rfd::MessageDialog::new().set_level(rfd::MessageLevel::Warning).set_title("Reinstall Kit").set_description(tr!("rst.dlg.confirm")).set_buttons(rfd::MessageButtons::YesNo).show();
            if yes != rfd::MessageDialogResult::Yes {
                action = 0;
            }
        }
        if action == 0 {
            return;
        }
        let dlg = self.restore_dlg.take().unwrap();
        if action == 3 {
            return;
        }
        let steps: Vec<String> = restore::STEPS.iter().enumerate().filter(|(i, _)| dlg.on[*i]).map(|(_, s)| s.to_string()).collect();
        let dry = action == 1;
        let kit = dlg.kit;
        self.run_job(ctx, if dry { tr!("pane.restore.dry") } else { tr!("pane.restore") }, move |o| {
            restore::run(kit, dry, steps, o);
        });
    }

    // ---- Set up page ---------------------------------------------------------------
    fn open_wizard(&mut self, ctx: &egui::Context) {
        if self.running || self.own_kit.is_some() {
            return;
        }
        self.page = Page::Wizard;
        self.scanning = true;
        self.scan_failed = false;
        self.rows.clear();
        self.wiz_status = tr!("wiz.scanning");
        let s = self.settings.clone();
        self.start(ctx, move |o| scan::run(s, o));
    }

    fn row_sub(full: &str, files: &[String], bytes: Option<(u64, bool)>, count: usize, secret: bool, gone: bool) -> String {
        let mut parts = vec![if files.is_empty() { full.to_string() } else { format!(r"{}\{{{}}}", full, files.join(", ")) }];
        if let Some((b, partial)) = bytes {
            parts.push(format!("{}{}", if partial { format!("{} ", tr!("wiz.atleast")) } else { String::new() }, fmt_size(b)));
        }
        if count > 1 {
            parts.push(tr!("wiz.projects.count", count));
        }
        if secret {
            parts.push(tr!("wiz.secret.mark"));
        }
        if gone {
            parts.push(tr!("wiz.notfound"));
        }
        parts.join("   ·   ")
    }

    fn build_wizard(&mut self, scan: ScanResult) {
        self.rows.clear();
        self.drives = scan.drives.iter().map(|d| (d.root.to_uppercase(), d.clone())).collect();
        self.projects_inside = scan.projects_inside;
        let first = self.settings.is_none();
        let have: HashMap<String, Item> = self.settings.as_ref().map(|s| s.items.iter().filter(|i| i.enabled).map(|i| (i.id.to_lowercase(), i.clone())).collect()).unwrap_or_default();
        let found_ids: Vec<String> = scan.items.iter().map(|f| f.item.id.to_lowercase()).collect();
        for f in &scan.items {
            let saved = have.get(&f.item.id.to_lowercase());
            // Keep hand edits of an already saved item; the size comes fresh from the scan.
            let item = saved.cloned().unwrap_or_else(|| f.item.clone());
            self.rows.push(Row {
                group: f.item.group.clone(),
                title: item_title(&item),
                sub: Self::row_sub(&f.full, &item.files, Some((f.bytes, f.partial)), f.count, item.secret, false),
                on: if first { f.def } else { saved.is_some() },
                enabled: true,
                bytes: f.bytes,
                partial: f.partial,
                item: Some(item),
                inv_id: None,
            });
        }
        // Items from the settings file that the scan did not find again (own folders, hand-written entries) stay in the list.
        if let Some(s) = &self.settings {
            for it in s.items.iter().filter(|i| i.enabled && !found_ids.contains(&i.id.to_lowercase())) {
                let full = sys::item_path(it);
                let gone = !Path::new(&full).is_dir();
                self.rows.push(Row {
                    group: "custom".into(),
                    title: item_title(it),
                    sub: Self::row_sub(&full, &it.files, None, 0, it.secret, gone),
                    on: true,
                    enabled: true,
                    bytes: 0,
                    partial: false,
                    item: Some(it.clone()),
                    inv_id: None,
                });
            }
        }
        for i in &scan.inventory {
            let on = if first { i.def } else { self.settings.as_ref().map(|s| s.inventory.get(&i.id)).unwrap_or(false) };
            let mut sub = tr!(&format!("inv.desc.{}", i.id));
            if !i.note.is_empty() {
                sub = format!("{}   ·   {}", sub, i.note);
            }
            self.rows.push(Row { group: "system".into(), item: None, inv_id: Some(i.id.clone()), title: tr!(&format!("inv.{}", i.id)), sub, on: on && i.available, enabled: i.available, bytes: 0, partial: false });
        }
        // Where, how and when.
        let s = self.settings.clone().unwrap_or_default();
        let mut dests = s.dests();
        if first {
            // Suggest the non-system drive with the most free space.
            if let Some(best) = scan.drives.iter().filter(|d| !d.system).max_by_key(|d| d.free) {
                dests.push(format!("{}Backups", best.root));
            }
        }
        self.dest1 = dests.first().cloned().unwrap_or_default();
        self.dest2 = dests.get(1).cloned().unwrap_or_default();
        self.secrets_enc = s.secrets_only_encrypted;
        self.days = s.keep_days().to_string();
        self.time = s.schedule.time.clone();
        self.schedule = s.schedule.enabled;
        self.notify = s.notify;
        self.wiz_lang = s.language.clone();
    }

    fn drive_of(&self, path: &str) -> Option<&sys::Drive> {
        self.drives.get(&sys::root_of(path.trim())?.to_uppercase())
    }
    fn enc_label(&self, ui: &mut egui::Ui, path: &str) {
        if path.trim().is_empty() {
            ui.add_space(4.0);
            return;
        }
        match self.drive_of(path) {
            None => {
                ui.label(RichText::new(tr!("wiz.enc.unknown")).size(12.5).color(DIM));
            }
            Some(d) => {
                let text = format!("{}   ·   {}", if d.enc { tr!("wiz.enc.yes") } else { tr!("wiz.enc.no") }, tr!("wiz.free", fmt_size(d.free)));
                ui.label(RichText::new(text).size(12.5).color(if d.enc { GOOD } else { DIM }));
                if d.system {
                    ui.label(RichText::new(tr!("wiz.dest.system")).size(12.5).color(WARN));
                }
            }
        }
        ui.add_space(6.0);
    }

    fn add_custom(&mut self, path: &str, installers: bool) {
        let path = path.trim_end_matches('\\').to_string();
        if path.len() < 3 {
            return; // a whole drive is not a folder to back up
        }
        let id = format!("custom:{}", path);
        if let Some(r) = self.rows.iter_mut().find(|r| r.item.as_ref().map(|i| i.id.eq_ignore_ascii_case(&id)).unwrap_or(false)) {
            r.on = true;
            return;
        }
        let leaf = Path::new(&path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "folder".into());
        let used: Vec<String> = self.rows.iter().filter_map(|r| r.item.as_ref().map(|i| i.dest.to_lowercase())).collect();
        let mut dn = leaf.clone();
        let mut n = 2;
        while used.contains(&format!("files\\folders\\{}", dn).to_lowercase()) {
            dn = format!("{} ({})", leaf, n);
            n += 1;
        }
        let title = if installers { format!("{}: {}", tr!("item.installers"), leaf) } else { leaf };
        let item = Item { id, group: "custom".into(), title: title.clone(), path: sys::tokenize(&path), dest: format!("files\\folders\\{}", dn), enabled: true, ..Default::default() };
        self.rows.push(Row { group: "custom".into(), title, sub: Self::row_sub(&path, &[], None, 0, false, false), on: true, enabled: true, bytes: 0, partial: false, item: Some(item), inv_id: None });
    }

    fn save_wizard(&mut self, schedule_too: bool) -> bool {
        let warn = |text: String| {
            rfd::MessageDialog::new().set_level(rfd::MessageLevel::Warning).set_title("Reinstall Kit").set_description(text).show();
        };
        let mut dests = vec![];
        for p in [&self.dest1, &self.dest2] {
            let mut p = p.trim().trim_end_matches('\\').to_string();
            if p.is_empty() {
                continue;
            }
            if p.len() == 2 && p.ends_with(':') {
                p.push('\\');
            }
            if sys::root_of(&p).is_none() {
                warn(tr!("wiz.err.badpath", p));
                return false;
            }
            dests.push(Dest { path: p });
        }
        if dests.is_empty() {
            warn(tr!("wiz.err.nodest"));
            return false;
        }
        let mut s = self.settings.clone().unwrap_or_default();
        s.help = Settings::default().help;
        s.version = 1;
        s.language = self.wiz_lang.clone();
        s.destinations = dests;
        s.secrets_only_encrypted = self.secrets_enc;
        s.keep_versions_days = self.days.trim().parse::<u32>().ok().filter(|d| (1..=3650).contains(d)).unwrap_or(30);
        let t = self.time.trim();
        let ok_time = t.split_once(':').map(|(h, m)| h.parse::<u32>().map(|h| h < 24).unwrap_or(false) && m.len() == 2 && m.parse::<u32>().map(|m| m < 60).unwrap_or(false)).unwrap_or(false);
        s.schedule.time = if ok_time { t.to_string() } else { "13:00".into() };
        s.schedule.enabled = self.schedule;
        s.notify = self.notify;
        s.items = self.rows.iter().filter(|r| r.on).filter_map(|r| r.item.clone()).map(|mut i| {
            i.enabled = true;
            i
        }).collect();
        for r in self.rows.iter().filter(|r| r.inv_id.is_some()) {
            s.inventory.set(r.inv_id.as_deref().unwrap(), r.on);
        }
        if let Err(e) = s.save(&settings_path()) {
            warn(tr!("wiz.err.save", e));
            return false;
        }
        lang::init(&s.language);
        if schedule_too {
            if s.schedule.enabled {
                let _ = sys::schedule_daily(&s.schedule.time);
            } else {
                sys::task_delete(sys::TASK_NAME);
            }
        }
        self.settings = Some(s);
        self.page = Page::Main;
        self.show_log = false;
        self.refresh();
        true
    }

    /// Automated test of the Set up page: "<destination>|<own folder>|<id>,<id>,..." - keeps only the
    /// listed items ticked, adds the folder and saves. Never touches Task Scheduler.
    fn apply_selftest(&mut self, spec: &str) {
        let p: Vec<&str> = spec.split('|').collect();
        let keep: Vec<&str> = p.get(2).map(|s| s.split(',').collect()).unwrap_or_default();
        for r in &mut self.rows {
            let id = r.inv_id.clone().or_else(|| r.item.as_ref().map(|i| i.id.clone())).unwrap_or_default();
            r.on = keep.contains(&id.as_str());
        }
        if let Some(f) = p.get(1).filter(|f| !f.is_empty()) {
            self.add_custom(f, false);
        }
        self.dest1 = p.first().unwrap_or(&"").to_string();
        self.dest2.clear();
        self.save_wizard(false);
    }

    // ---- Drawing -------------------------------------------------------------------
    fn main_page(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        ui.label(RichText::new(&self.title).heading());
        ui.label(RichText::new(&self.sub).size(14.0).color(DIM));
        ui.add_space(12.0);
        let idle = !self.running;
        let kit_mode = self.own_kit.is_some();
        let ready = self.settings.as_ref().map(|s| !s.dests().is_empty()).unwrap_or(false);
        let h = ui.available_height();
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(egui::vec2(390.0, h), Layout::top_down(Align::Min), |ui| {
                if card(ui, idle && ready && !kit_mode, '\u{E895}', ACCENT, &tr!("main.update.h"), &tr!("main.update.s")) {
                    self.run_job(&ctx, tr!("pane.update"), |o| {
                        update::run(false, false, o);
                    });
                }
                if card(ui, idle && ready && !kit_mode, '\u{E73E}', GOOD, &tr!("main.check.h"), &tr!("main.check.s")) {
                    self.run_job(&ctx, tr!("pane.check"), |o| {
                        check::run(o);
                    });
                }
                if card(ui, idle, '\u{E777}', WARN, &tr!("main.restore.h"), &tr!("main.restore.s")) {
                    self.open_restore();
                }
                if card(ui, idle && !kit_mode, '\u{E713}', Color32::from_rgb(214, 200, 255), &tr!("main.setup.h"), &tr!("main.setup.s")) {
                    self.open_wizard(&ctx);
                }
            });
            ui.add_space(10.0);
            ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), h), Layout::top_down(Align::Min), |ui| {
                panel_frame().show(ui, |ui| {
                    ui.set_min_size(ui.available_size());
                    ui.horizontal(|ui| {
                        ui.label(semi(if self.show_log { self.pane_title.clone() } else { tr!("pane.about") }, 18.0));
                        if self.show_log {
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.add_space(8.0);
                                if ui.button(tr!("pane.about")).clicked() {
                                    self.show_log = false;
                                }
                            });
                        }
                    });
                    ui.add_space(4.0);
                    if self.show_log {
                        ui.label(RichText::new(&self.prog_text).size(15.0));
                        let frac = if self.prog.1 == 0 { 0.0 } else { self.prog.0 as f32 / self.prog.1 as f32 };
                        let mut bar = egui::ProgressBar::new(frac).desired_height(8.0).fill(ACCENT).desired_width(ui.available_width() - 8.0);
                        if self.running && self.prog.1 == 0 {
                            bar = bar.animate(true);
                        }
                        ui.add(bar);
                        ui.add_space(8.0);
                        egui::ScrollArea::both().auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
                            ui.add(egui::Label::new(RichText::new(&self.log).monospace()).wrap_mode(egui::TextWrapMode::Extend));
                        });
                    } else {
                        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                            ui.add(egui::Label::new(RichText::new(&self.about).size(14.5)).wrap());
                        });
                    }
                });
            });
        });
    }

    fn wizard_page(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new(tr!("wiz.title")).heading());
        ui.horizontal(|ui| {
            if self.scanning {
                ui.spinner();
            }
            ui.label(RichText::new(&self.wiz_status).size(14.0).color(DIM));
        });
        ui.add_space(10.0);
        let h = (ui.available_height() - 52.0).max(200.0);
        let ready = !self.scanning && !self.scan_failed;
        ui.horizontal_top(|ui| {
            if !ready {
                ui.allocate_space(egui::vec2(ui.available_width(), h));
                return;
            }
            let left_w = ui.available_width() - 420.0;
            ui.allocate_ui_with_layout(egui::vec2(left_w, h), Layout::top_down(Align::Min), |ui| {
                panel_frame().show(ui, |ui| {
                    ui.set_min_size(ui.available_size());
                    ui.label(semi(tr!("wiz.what"), 18.0));
                    let list_h = ui.available_height() - 44.0;
                    egui::ScrollArea::vertical().auto_shrink([false, false]).max_height(list_h).show(ui, |ui| {
                        let groups: Vec<&str> = GROUPS.iter().copied().chain(std::iter::once("system")).collect();
                        for g in groups {
                            if g != "custom" && !self.rows.iter().any(|r| r.group == g) {
                                continue;
                            }
                            ui.add_space(8.0);
                            ui.label(semi(tr!(&format!("grp.{}", g)), 15.5).color(ACCENT));
                            let hint_key = format!("grp.hint.{}", g);
                            if g == "projects" && self.projects_inside > 0 {
                                ui.label(RichText::new(tr!("grp.hint.projects.inside", self.projects_inside)).size(12.5).color(DIM));
                            } else if lang::has(&hint_key) {
                                ui.label(RichText::new(tr!(&hint_key)).size(12.5).color(DIM));
                            }
                            for r in self.rows.iter_mut().filter(|r| r.group == g) {
                                ui.add_enabled_ui(r.enabled, |ui| {
                                    ui.horizontal(|ui| {
                                        check(ui, &mut r.on);
                                        ui.vertical(|ui| {
                                            ui.spacing_mut().item_spacing.y = 1.0;
                                            ui.label(RichText::new(&r.title).size(14.0));
                                            ui.label(RichText::new(&r.sub).size(12.0).color(DIM));
                                        });
                                    });
                                });
                            }
                        }
                        if self.projects_inside > 0 && !self.rows.iter().any(|r| r.group == "projects") {
                            ui.add_space(8.0);
                            ui.label(RichText::new(tr!("grp.hint.projects.inside", self.projects_inside)).size(12.5).color(DIM));
                        }
                    });
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.button(tr!("wiz.add")).clicked() {
                            if let Some(p) = rfd::FileDialog::new().set_title(tr!("wiz.pick.folder")).pick_folder() {
                                self.add_custom(&p.to_string_lossy(), false);
                            }
                        }
                        if ui.button(tr!("wiz.addinst")).clicked() {
                            if let Some(p) = rfd::FileDialog::new().set_title(tr!("wiz.pick.installers")).pick_folder() {
                                self.add_custom(&p.to_string_lossy(), true);
                            }
                        }
                    });
                });
            });
            ui.add_space(10.0);
            ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), h), Layout::top_down(Align::Min), |ui| {
                panel_frame().show(ui, |ui| {
                    ui.set_min_size(ui.available_size());
                    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                        ui.set_width(ui.available_width() - 10.0);
                        ui.label(semi(tr!("wiz.where"), 18.0));
                        ui.label(RichText::new(tr!("wiz.where.s")).size(13.0).color(DIM));
                        ui.add_space(6.0);
                        for n in 0..2 {
                            ui.label(RichText::new(tr!(if n == 0 { "wiz.dest1" } else { "wiz.dest2" })).size(13.5));
                            let mut pick = false;
                            ui.horizontal(|ui| {
                                let w = ui.available_width() - 110.0;
                                let field = if n == 0 { &mut self.dest1 } else { &mut self.dest2 };
                                ui.add(egui::TextEdit::singleline(field).desired_width(w).margin(egui::vec2(8.0, 6.0)));
                                pick = ui.button(tr!("wiz.browse")).clicked();
                            });
                            if pick {
                                if let Some(p) = rfd::FileDialog::new().set_title(tr!("wiz.pick.dest")).pick_folder() {
                                    let p = p.to_string_lossy().into_owned();
                                    if n == 0 {
                                        self.dest1 = p;
                                    } else {
                                        self.dest2 = p;
                                    }
                                }
                            }
                            let path = if n == 0 { self.dest1.clone() } else { self.dest2.clone() };
                            self.enc_label(ui, &path);
                        }
                        check_text(ui, &mut self.secrets_enc, &tr!("wiz.secretsenc"));
                        let any_secret = self.rows.iter().any(|r| r.on && r.item.as_ref().map(|i| i.secret).unwrap_or(false));
                        let enc_somewhere = [&self.dest1, &self.dest2].iter().any(|d| self.drive_of(d).map(|x| x.enc).unwrap_or(false));
                        if any_secret && self.secrets_enc && !enc_somewhere {
                            ui.label(RichText::new(tr!("wiz.secrets.warn")).size(12.5).color(WARN));
                        }
                        ui.add_space(14.0);
                        ui.label(semi(tr!("wiz.how"), 18.0));
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut self.days).desired_width(44.0).margin(egui::vec2(8.0, 6.0)).horizontal_align(Align::Center));
                            ui.label(RichText::new(tr!("wiz.days")).size(13.5));
                        });
                        ui.horizontal(|ui| {
                            check_text(ui, &mut self.schedule, &tr!("wiz.schedule"));
                            ui.add(egui::TextEdit::singleline(&mut self.time).desired_width(52.0).margin(egui::vec2(8.0, 6.0)).horizontal_align(Align::Center));
                        });
                        check_text(ui, &mut self.notify, &tr!("wiz.notify"));
                        ui.add_space(8.0);
                        let name = match self.wiz_lang.as_str() {
                            "ru" => "Русский".to_string(),
                            "en" => "English".to_string(),
                            _ => tr!("wiz.lang.auto"),
                        };
                        if ui.button(tr!("wiz.lang", name)).clicked() {
                            self.wiz_lang = match self.wiz_lang.as_str() {
                                "auto" => "ru",
                                "ru" => "en",
                                _ => "auto",
                            }
                            .into();
                        }
                    });
                });
            });
        });
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if ready {
                let on: Vec<&Row> = self.rows.iter().filter(|r| r.on).collect();
                let bytes: u64 = on.iter().map(|r| r.bytes).sum();
                let partial = on.iter().any(|r| r.partial);
                ui.label(RichText::new(tr!("wiz.total", on.len(), format!("{}{}", if partial { format!("{} ", tr!("wiz.atleast")) } else { String::new() }, fmt_size(bytes)))).size(14.0));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if accent_button(ui, &tr!("wiz.save"), ready) {
                    self.save_wizard(true);
                }
                if ui.add(egui::Button::new(tr!("wiz.cancel")).min_size(egui::vec2(0.0, 36.0))).clicked() && !self.scanning {
                    self.page = Page::Main;
                    self.refresh();
                }
            });
        });
    }
}

impl eframe::App for App {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        if self.glass {
            [0.0, 0.0, 0.0, 0.0]
        } else {
            [22.0 / 255.0, 28.0 / 255.0, 36.0 / 255.0, 1.0] // Windows 10: plain dark background
        }
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll(ctx);
        if self.first_frame {
            self.first_frame = false;
            let page = self.page_arg.clone();
            if self.selftest.is_some() || page == "wizard" || (self.settings.is_none() && self.own_kit.is_none() && page.is_empty()) {
                self.open_wizard(ctx);
            } else if page == "update" {
                self.run_job(ctx, tr!("pane.update"), |o| {
                    update::run(false, false, o);
                });
            } else if page == "check" {
                self.run_job(ctx, tr!("pane.check"), |o| {
                    check::run(o);
                });
            } else if page == "restore" {
                self.open_restore();
            }
        }
        if ctx.input(|i| i.viewport().close_requested()) && self.running && !self.scanning {
            let yes = rfd::MessageDialog::new().set_level(rfd::MessageLevel::Warning).set_title("Reinstall Kit").set_description(tr!("close.confirm")).set_buttons(rfd::MessageButtons::YesNo).show();
            if yes != rfd::MessageDialogResult::Yes {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            }
        }
        let tint = if self.glass { Color32::from_rgba_unmultiplied(16, 21, 28, 96) } else { Color32::from_rgb(22, 28, 36) };
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(tint).inner_margin(Margin { left: 28.0, right: 28.0, top: 14.0, bottom: 22.0 }))
            .show(ctx, |ui| {
                ui.add_enabled_ui(self.restore_dlg.is_none(), |ui| match self.page {
                    Page::Main => self.main_page(ui),
                    Page::Wizard => self.wizard_page(ui),
                });
            });
        self.restore_window(ctx);
    }
}
