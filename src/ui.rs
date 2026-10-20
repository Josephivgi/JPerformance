use crate::insights::{Insight, Severity};
use crate::theme::{self, Tokens};
use crate::{autostart, collector, db, describe, fmt, insights, paths, procs};
use eframe::egui::{self, Color32, RichText, Sense, Stroke, Ui, vec2};
use egui_plot::{Line, Plot, PlotPoints, Points, VLine};
use rusqlite::Connection;
use std::collections::HashMap;
use std::time::{Duration, Instant};

pub fn run() -> Result<(), String> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("JPerformance")
            .with_inner_size([1200.0, 800.0])
            .with_min_inner_size([900.0, 600.0]),
        ..Default::default()
    };
    eframe::run_native("JPerformance", options, Box::new(|cc| Ok(Box::new(App::new(cc)?))))
        .map_err(|e| e.to_string())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Now,
    History,
    Apps,
    Insights,
    Running,
    Settings,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Range {
    H1,
    H6,
    H24,
    D7,
    D30,
}

impl Range {
    const ALL: [Range; 5] = [Range::H1, Range::H6, Range::H24, Range::D7, Range::D30];
    fn secs(self) -> i64 {
        match self {
            Range::H1 => 3600,
            Range::H6 => 6 * 3600,
            Range::H24 => 86_400,
            Range::D7 => 7 * 86_400,
            Range::D30 => 30 * 86_400,
        }
    }
    fn label(self) -> &'static str {
        match self {
            Range::H1 => "1 hour",
            Range::H6 => "6 hours",
            Range::H24 => "24 hours",
            Range::D7 => "7 days",
            Range::D30 => "30 days",
        }
    }
    /// Bucket size giving roughly 360 points across the range.
    fn bucket(self) -> i64 {
        (self.secs() / 360).max(5)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RunSort {
    Memory,
    Cpu,
    Name,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AppSort {
    Cpu,
    Memory,
    Io,
}

/// A clicked point in time and what was happening then.
struct Moment {
    from: i64,
    to: i64,
    sys: Option<db::SysPoint>,
    apps: Vec<db::AppSummary>,
    minutes: i64,
}

impl Moment {
    fn len(&self) -> i64 {
        self.to - self.from + 1
    }
    fn center(&self) -> i64 {
        self.from + self.len() / 2
    }
    fn label(&self) -> String {
        if self.len() <= 60 {
            fmt::day_time(self.from)
        } else {
            format!("{} to {}", fmt::day_time(self.from), fmt::local(self.to + 1).format("%H:%M"))
        }
    }
}

struct App {
    conn: Connection,
    page: Page,
    range: Range,
    moment: Option<Moment>,

    last_live: Option<Instant>,
    last_range: Option<Instant>,
    range_dirty: bool,

    latest: Option<(db::SysPoint, u64)>,
    recent: Vec<db::SysPoint>,
    live: Vec<db::LiveRow>,
    live_sort: AppSort,
    collector_running: bool,
    status: Option<db::CollectorStatus>,

    history: Vec<db::SysPoint>,
    heat: [[Option<f64>; 24]; 7],
    apps: Vec<db::AppSummary>,
    app_sort: AppSort,
    app_filter: String,
    selected_app: Option<String>,
    app_series: Vec<db::AppPoint>,
    insights: Vec<Insight>,
    tips: TipCache,
    run_filter: String,
    run_sort: RunSort,
    run_notice: Option<(String, bool)>,
    /// When each app was last asked to stop.
    stops: HashMap<String, Instant>,
    confirm_force: Option<String>,

    tracking: bool,
    autostart: bool,
    interval: i64,
    retention: i64,
    confirm_clear: bool,
    notice: Option<(String, bool)>,
    error: Option<String>,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        theme::install(&cc.egui_ctx);
        let conn = db::open(&paths::db_path())?;
        let tracking = db::tracking_enabled(&conn);
        if tracking {
            if let Err(e) = collector::spawn_detached() {
                collector::log(&format!("could not start collector: {e}"));
            }
        }
        autostart::refresh_path();
        Ok(Self {
            interval: db::setting_i64(&conn, db::KEY_INTERVAL, db::DEFAULT_INTERVAL),
            retention: db::setting_i64(&conn, db::KEY_RETENTION, db::DEFAULT_RETENTION),
            conn,
            page: Page::Now,
            range: Range::H24,
            moment: None,
            last_live: None,
            last_range: None,
            range_dirty: true,
            latest: None,
            recent: Vec::new(),
            live: Vec::new(),
            live_sort: AppSort::Cpu,
            collector_running: false,
            status: None,
            history: Vec::new(),
            heat: [[None; 24]; 7],
            apps: Vec::new(),
            app_sort: AppSort::Cpu,
            app_filter: String::new(),
            selected_app: None,
            app_series: Vec::new(),
            insights: Vec::new(),
            tips: TipCache::default(),
            run_filter: String::new(),
            run_sort: RunSort::Memory,
            run_notice: None,
            stops: HashMap::new(),
            confirm_force: None,
            tracking,
            autostart: autostart::is_enabled(),
            confirm_clear: false,
            notice: None,
            error: None,
        })
    }

    fn refresh(&mut self) {
        let now = chrono::Utc::now().timestamp();
        if self.last_live.is_none_or(|t| t.elapsed() >= Duration::from_secs(2)) {
            self.last_live = Some(Instant::now());
            let r: db::Result<()> = (|| {
                self.latest = db::latest_sample(&self.conn)?;
                self.recent = db::system_series(&self.conn, now - 15 * 60, now, 1)?;
                self.live = db::live_apps(&self.conn)?;
                self.status = db::collector_status(&self.conn)?;
                self.tips.map.clear();
                Ok(())
            })();
            self.collector_running = collector::is_running();
            self.error = r.err().map(|e| format!("Could not read data: {e}"));
        }
        if self.range_dirty || self.last_range.is_none_or(|t| t.elapsed() >= Duration::from_secs(20)) {
            self.range_dirty = false;
            self.last_range = Some(Instant::now());
            let from = now - self.range.secs();
            let r: db::Result<()> = (|| {
                self.history = db::system_series(&self.conn, from, now, self.range.bucket())?;
                self.heat = db::weekday_hour_cpu(&self.conn, from, now)?;
                self.apps = db::app_summaries(&self.conn, from, now)?;
                self.insights = insights::compute(&self.conn, from, now)?;
                self.app_series = match &self.selected_app {
                    Some(a) => db::app_series(&self.conn, a, from, now, self.range.bucket())?,
                    None => Vec::new(),
                };
                Ok(())
            })();
            if let Err(e) = r {
                self.error = Some(format!("Could not read data: {e}"));
            }
        }
    }

    fn set_tracking(&mut self, on: bool) {
        self.tracking = on;
        if let Err(e) = db::set_setting(&self.conn, db::KEY_TRACKING, if on { "1" } else { "0" }) {
            self.notice = Some((format!("Could not save setting: {e}"), true));
            return;
        }
        if on {
            match collector::spawn_detached() {
                Ok(()) => self.notice = Some(("Background tracking started.".into(), false)),
                Err(e) => self.notice = Some((format!("Could not start tracking: {e}"), true)),
            }
        } else {
            self.notice = Some(("Background tracking will stop within a few seconds.".into(), false));
        }
        self.last_live = None;
    }
}

impl App {
    /// Selects the `bucket`-second window containing `ts` (at least one minute).
    fn select_at(&mut self, ts: i64, bucket: i64) {
        let bucket = bucket.max(60);
        let from = ts - ts.rem_euclid(bucket);
        self.select_window(from, from + bucket - 1);
    }

    fn select_window(&mut self, from: i64, to: i64) {
        let r: db::Result<Moment> = (|| {
            let sys = db::system_series(&self.conn, from, to, to - from + 1)?.into_iter().next();
            let mut apps = db::app_summaries(&self.conn, from - from.rem_euclid(60), to)?;
            apps.sort_by(|a, b| b.cpu_load.total_cmp(&a.cpu_load));
            let minutes = db::minutes_recorded(&self.conn, from - from.rem_euclid(60), to)?;
            Ok(Moment { from, to, sys, apps, minutes })
        })();
        match r {
            Ok(m) => self.moment = Some(m),
            Err(e) => self.error = Some(format!("Could not read data: {e}")),
        }
    }

    fn moment_panel(&mut self, ui: &mut Ui, t: &Tokens) {
        let Some(m) = &self.moment else { return };
        let (from, len) = (m.from, m.len());
        let mut close = false;
        let mut shift = 0i64;
        let mut open_app = None;
        ui.horizontal(|ui| {
            ui.label(RichText::new("At this time").size(16.0).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Close").clicked() {
                    close = true;
                }
            });
        });
        ui.label(RichText::new(m.label()).color(t.text_2));
        ui.horizontal(|ui| {
            if ui.button("Earlier").clicked() {
                shift = -len;
            }
            let later_ok = from + len <= chrono::Utc::now().timestamp();
            if ui.add_enabled(later_ok, egui::Button::new("Later")).clicked() {
                shift = len;
            }
        });
        ui.add_space(theme::GAP);

        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            let Some(p) = &m.sys else {
                ui.label(RichText::new("Nothing was recorded at this time. Background tracking may have been off.").color(t.muted));
                return;
            };
            egui::Grid::new("moment_stats").num_columns(2).spacing(vec2(16.0, 6.0)).show(ui, |ui| {
                ui.label(RichText::new("CPU").color(t.text_2));
                ui.label(if len > 60 {
                    format!("{} average, {} peak", fmt::pct(p.cpu), fmt::pct(p.cpu_max))
                } else {
                    format!("{} average", fmt::pct(p.cpu))
                });
                ui.end_row();
                ui.label(RichText::new("Memory").color(t.text_2));
                ui.label(format!("{} ({})", fmt::pct(p.mem_pct), fmt::bytes(p.mem_used)));
                ui.end_row();
                ui.label(RichText::new("Disk and file I/O").color(t.text_2));
                ui.label(fmt::rate(p.io_rate));
                ui.end_row();
                ui.label(RichText::new("Network").color(t.text_2));
                ui.label(fmt::rate(p.net_rate));
                ui.end_row();
            });

            if m.apps.is_empty() {
                ui.add_space(theme::GAP);
                ui.label(RichText::new("No per-app data for this time. Apps are saved once a minute.").color(t.muted));
                return;
            }
            let minutes = m.minutes.max(1) as f64;
            let max_cpu = m.apps.first().map(|a| a.cpu_load).unwrap_or(0.0).max(f64::EPSILON);

            section(ui, t, "Using the most CPU");
            for a in m.apps.iter().filter(|a| a.cpu_load * 100.0 / minutes >= 0.1).take(8) {
                let share = a.cpu_load * 100.0 / minutes;
                if moment_row(ui, t, &a.app, a.description.as_deref(), &fmt::pct(share), a.cpu_load / max_cpu) {
                    open_app = Some(a.app.clone());
                }
            }

            let mut by_mem: Vec<&db::AppSummary> = m.apps.iter().collect();
            by_mem.sort_by(|a, b| b.mem_avg.total_cmp(&a.mem_avg));
            let max_mem = by_mem.first().map(|a| a.mem_avg).unwrap_or(0.0).max(f64::EPSILON);
            section(ui, t, "Using the most memory");
            for a in by_mem.into_iter().take(6) {
                if moment_row(ui, t, &a.app, a.description.as_deref(), &fmt::bytes(a.mem_avg), a.mem_avg / max_mem) {
                    open_app = Some(a.app.clone());
                }
            }
        });

        if close {
            self.moment = None;
        } else if shift != 0 {
            self.select_window(from + shift, from + shift + len - 1);
        }
        if let Some(app) = open_app {
            self.selected_app = Some(app);
            self.page = Page::Apps;
            self.range_dirty = true;
        }
    }
}

/// One app line in the moment panel. Returns true when clicked.
fn moment_row(ui: &mut Ui, t: &Tokens, app: &str, desc: Option<&str>, value: &str, frac: f64) -> bool {
    let resp = ui
        .vertical(|ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(describe::display_name(app, desc)).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(value);
                });
            });
            let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 6.0), Sense::hover());
            ui.painter().rect_filled(rect, 3.0, t.surface_alt);
            let mut fill = rect;
            fill.set_width((rect.width() * frac as f32).max(2.0));
            ui.painter().rect_filled(fill, 3.0, t.accent);
            ui.label(RichText::new(describe::explain(app, desc)).small().color(t.text_2));
            ui.add_space(4.0);
        })
        .response
        .interact(Sense::click())
        .on_hover_text("Open in Apps");
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp.clicked()
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.refresh();
        ui.ctx().request_repaint_after(Duration::from_secs(2));
        let t = theme::tokens(ui.ctx());

        egui::Panel::left("nav")
            .exact_size(200.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(t.surface).inner_margin(egui::Margin::same(14)).stroke(Stroke::new(1.0, t.border)))
            .show(ui, |ui| self.nav(ui, &t));

        if self.moment.is_some() {
            egui::Panel::right("moment")
                .exact_size(340.0)
                .resizable(false)
                .frame(egui::Frame::new().fill(t.surface).inner_margin(egui::Margin::same(16)).stroke(Stroke::new(1.0, t.border)))
                .show(ui, |ui| self.moment_panel(ui, &t));
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(t.bg).inner_margin(egui::Margin::symmetric(24, 18)))
            .show(ui, |ui| {
                if let Some(err) = &self.error {
                    ui.colored_label(t.critical, err);
                }
                egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| match self.page {
                    Page::Now => self.page_now(ui, &t),
                    Page::History => self.page_history(ui, &t),
                    Page::Apps => self.page_apps(ui, &t),
                    Page::Insights => self.page_insights(ui, &t),
                    Page::Running => self.page_running(ui, &t),
                    Page::Settings => self.page_settings(ui, &t),
                });
            });

        if self.confirm_clear {
            self.clear_dialog(ui, &t);
        }
        if self.confirm_force.is_some() {
            self.force_dialog(ui, &t);
        }
    }
}

// ---------- shared widgets ----------

fn heading(ui: &mut Ui, t: &Tokens, title: &str, sub: &str) {
    ui.label(RichText::new(title).size(22.0).strong().color(t.text));
    if !sub.is_empty() {
        ui.label(RichText::new(sub).color(t.text_2));
    }
    ui.add_space(theme::GAP);
}

fn section(ui: &mut Ui, t: &Tokens, title: &str) {
    ui.add_space(theme::GAP);
    ui.label(RichText::new(title).size(15.0).strong().color(t.text));
    ui.add_space(4.0);
}

fn status_dot(ui: &mut Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.0, color);
}

impl App {
    fn range_picker(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.label("Time range");
            for r in Range::ALL {
                if ui.selectable_label(self.range == r, r.label()).clicked() && self.range != r {
                    self.range = r;
                    self.range_dirty = true;
                    self.moment = None;
                }
            }
        });
        ui.add_space(theme::GAP);
    }
}

/// Splits points into segments wherever there is a gap (tracking was off).
fn segments(points: &[(i64, f64)], max_gap: i64) -> Vec<Vec<[f64; 2]>> {
    let mut out: Vec<Vec<[f64; 2]>> = Vec::new();
    let mut prev: Option<i64> = None;
    for &(ts, v) in points {
        if prev.is_none_or(|p| ts - p > max_gap) {
            out.push(Vec::new());
        }
        out.last_mut().unwrap().push([ts as f64, v]);
        prev = Some(ts);
    }
    out
}

/// What a chart's hover tooltip breaks down by app.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tip {
    None,
    Cpu,
    Mem,
    Io,
    Net,
    Procs,
}

struct ChartSpec<'a> {
    id: &'a str,
    title: &'a str,
    value_fmt: fn(f64) -> String,
    y_max: Option<f64>,
    height: f32,
    gap: i64,
    x_range: Option<(i64, i64)>,
    marker: Option<i64>,
    tip: Tip,
    /// Seconds each point covers.
    bucket: i64,
}

/// Per-app data for hovered time windows, so hovering does not query every frame.
#[derive(Default)]
struct TipCache {
    map: HashMap<(i64, i64), (Vec<db::AppSummary>, i64)>,
}

impl TipCache {
    fn get(&mut self, conn: &Connection, from: i64, to: i64) -> &(Vec<db::AppSummary>, i64) {
        if self.map.len() > 400 {
            self.map.clear();
        }
        self.map.entry((from, to)).or_insert_with(|| {
            let apps = db::app_summaries(conn, from, to).unwrap_or_default();
            let minutes = db::minutes_recorded(conn, from, to).unwrap_or(0);
            (apps, minutes)
        })
    }
}

/// Point nearest to `x`, if it is close enough to belong to the hovered spot.
fn nearest(points: &[(i64, f64)], x: f64, max_dist: i64) -> Option<(i64, f64)> {
    let i = points.partition_point(|(ts, _)| (*ts as f64) < x);
    let candidates = [i.checked_sub(1), Some(i)];
    candidates
        .into_iter()
        .flatten()
        .filter_map(|i| points.get(i).copied())
        .min_by(|a, b| (a.0 as f64 - x).abs().total_cmp(&(b.0 as f64 - x).abs()))
        .filter(|(ts, _)| (*ts as f64 - x).abs() <= max_dist as f64)
}

/// A single-series time chart: one line, one y axis, hover breakdown by app.
/// Returns the time the user clicked, if any.
fn time_chart(
    ui: &mut Ui,
    t: &Tokens,
    spec: ChartSpec,
    points: &[(i64, f64)],
    tips: &mut TipCache,
    conn: &Connection,
) -> Option<i64> {
    theme::card(t).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new(spec.title).strong());
            if let Some((_, v)) = points.last() {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(format!("latest {}", (spec.value_fmt)(*v))).color(t.text_2));
                });
            }
        });
        if points.is_empty() {
            ui.add_space(8.0);
            ui.label(RichText::new("No samples in this range yet.").color(t.muted));
            ui.add_space(8.0);
            return None;
        }
        let value_fmt = spec.value_fmt;
        let mut plot = Plot::new(spec.id)
            .height(spec.height)
            .allow_drag(false)
            .allow_zoom(false)
            .allow_scroll(false)
            .allow_boxed_zoom(false)
            .show_x(false)
            .show_y(false)
            .include_y(0.0)
            .set_margin_fraction(vec2(0.0, 0.08))
            .x_axis_formatter(|m, r| fmt::axis(m.value as i64, r.end() - r.start()))
            .y_axis_formatter(move |m, _| value_fmt(m.value))
            .label_formatter(|_| None);
        if let Some(max) = spec.y_max {
            plot = plot.include_y(max);
        }
        if let Some((a, b)) = spec.x_range {
            plot = plot.include_x(a as f64).include_x(b as f64);
        }
        let resp = plot.show(ui, |p| {
            for seg in segments(points, spec.gap) {
                p.line(
                    Line::new(spec.title, PlotPoints::from(seg))
                        .color(t.accent)
                        .width(2.0)
                        .fill(0.0)
                        .fill_alpha(0.10),
                );
            }
            if let Some(m) = spec.marker {
                p.vline(VLine::new("marker", m as f64).color(t.text_2).width(1.5));
            }
            let hover = if p.response().hovered() {
                p.pointer_coordinate().and_then(|c| nearest(points, c.x, spec.gap))
            } else {
                None
            };
            if let Some((ts, v)) = hover {
                p.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                p.vline(VLine::new("hover", ts as f64).color(t.muted).width(1.0));
                p.points(Points::new("hover point", vec![[ts as f64, v]]).radius(4.5).color(t.accent).filled(true));
            }
            let clicked = if p.response().clicked() { p.pointer_coordinate().map(|c| c.x as i64) } else { None };
            (clicked, hover)
        });
        let (clicked, hover) = resp.inner;
        if let Some((ts, v)) = hover {
            resp.response.on_hover_ui_at_pointer(|ui| tip_ui(ui, t, &spec, ts, v, tips, conn));
        }
        clicked
    })
    .inner
}

fn tip_ui(ui: &mut Ui, t: &Tokens, spec: &ChartSpec, ts: i64, v: f64, tips: &mut TipCache, conn: &Connection) {
    ui.set_min_width(240.0);
    let bucket = spec.bucket.max(1);
    let from = ts - ts.rem_euclid(bucket);
    let when = if bucket < 60 {
        fmt::day_time(ts)
    } else if bucket == 60 {
        format!("{} (1 min)", fmt::day_time(from))
    } else {
        format!("{} to {}", fmt::day_time(from), fmt::local(from + bucket).format("%H:%M"))
    };
    ui.label(RichText::new(when).small().color(t.text_2));
    ui.label(RichText::new(format!("{}  {}", spec.title, (spec.value_fmt)(v))).strong());

    let rank = match spec.tip {
        Tip::None => return,
        Tip::Net => {
            ui.label(RichText::new("Windows does not report network use per app.").small().color(t.muted));
            return;
        }
        other => other,
    };
    // App data is stored per minute; use the minutes this point covers.
    let a_from = from - from.rem_euclid(60);
    let a_to = (from + bucket).max(a_from + 60) - 1;
    let (apps, minutes) = tips.get(conn, a_from, a_to);
    if *minutes == 0 || apps.is_empty() {
        ui.label(RichText::new("App details are saved once a minute and are not ready for this moment yet.").small().color(t.muted));
        return;
    }
    let minutes = *minutes as f64;
    let mut rows: Vec<(f64, String, &db::AppSummary)> = apps
        .iter()
        .map(|a| match rank {
            Tip::Cpu => {
                let v = a.cpu_load * 100.0 / minutes;
                (v, fmt::pct(v), a)
            }
            Tip::Mem => (a.mem_avg, fmt::bytes(a.mem_avg), a),
            Tip::Io => {
                let v = a.io_total as f64 / (minutes * 60.0);
                (v, fmt::rate(v), a)
            }
            _ => (a.procs, format!("{:.0}", a.procs), a),
        })
        .filter(|(v, _, _)| *v > 0.0)
        .collect();
    rows.sort_by(|a, b| b.0.total_cmp(&a.0));
    ui.add_space(4.0);
    ui.label(RichText::new(if rank == Tip::Procs { "Apps with the most processes" } else { "Top apps" }).small().color(t.muted));
    egui::Grid::new(("tip", spec.id)).num_columns(2).spacing(vec2(16.0, 2.0)).show(ui, |ui| {
        for (_, label, a) in rows.iter().take(6) {
            ui.label(describe::display_name(&a.app, a.description.as_deref()));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new(label).strong());
            });
            ui.end_row();
        }
    });
    ui.label(RichText::new("Click to keep this moment open.").small().color(t.muted));
}

fn pct_fmt(v: f64) -> String {
    format!("{v:.0}%")
}
fn rate_fmt(v: f64) -> String {
    fmt::rate(v)
}
fn count_fmt(v: f64) -> String {
    format!("{v:.0}")
}
fn bytes_fmt(v: f64) -> String {
    fmt::bytes(v)
}

// ---------- pages ----------

impl App {
    fn nav(&mut self, ui: &mut Ui, t: &Tokens) {
        ui.add_space(4.0);
        ui.label(RichText::new("JPerformance").size(18.0).strong().color(t.text));
        ui.label(RichText::new("App performance over time").small().color(t.muted));
        ui.add_space(18.0);
        for (page, label) in [
            (Page::Now, "Right now"),
            (Page::Running, "Running apps"),
            (Page::History, "History"),
            (Page::Apps, "Apps"),
            (Page::Insights, "Insights"),
            (Page::Settings, "Settings"),
        ] {
            let selected = self.page == page;
            let text = if selected { RichText::new(label).strong() } else { RichText::new(label) };
            let resp = ui.add_sized([ui.available_width(), 32.0], egui::Button::selectable(selected, text));
            if resp.clicked() {
                self.page = page;
            }
        }

        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            let (color, text) = if self.collector_running {
                (t.good, "Tracking in background")
            } else if self.tracking {
                (t.warning, "Tracking is starting")
            } else {
                (t.muted, "Tracking is off")
            };
            if let Some(s) = &self.status {
                if self.collector_running {
                    ui.label(RichText::new(format!("Last sample {}", fmt::ago(s.heartbeat))).small().color(t.muted));
                }
            }
            ui.horizontal(|ui| {
                status_dot(ui, color);
                ui.label(RichText::new(text).small().color(t.text_2));
            });
        });
    }

    fn page_now(&mut self, ui: &mut Ui, t: &Tokens) {
        heading(ui, t, "Right now", "Live readings from the background tracker, updated every few seconds.");
        if !self.collector_running && self.latest.as_ref().is_none_or(|(p, _)| chrono::Utc::now().timestamp() - p.ts > 60) {
            theme::card(t).show(ui, |ui| {
                ui.label(RichText::new("Background tracking is not running").strong());
                ui.label(RichText::new("Turn it on to record performance over time.").color(t.text_2));
                if ui.button("Start tracking").clicked() {
                    self.set_tracking(true);
                }
            });
            ui.add_space(theme::GAP);
        }

        if let Some((p, total)) = &self.latest {
            let tiles = [
                ("CPU", fmt::pct(p.cpu), String::new()),
                ("Memory", fmt::pct(p.mem_pct), format!("{} of {}", fmt::bytes(p.mem_used), fmt::bytes(*total as f64))),
                ("Disk and file I/O", fmt::rate(p.io_rate), String::new()),
                ("Network", fmt::rate(p.net_rate), String::new()),
                (
                    "Running now",
                    if p.procs > 0.0 { format!("{:.0} processes", p.procs) } else { "Counting".into() },
                    if p.apps > 0.0 { format!("from {:.0} apps", p.apps) } else { String::new() },
                ),
            ];
            ui.columns(5, |cols| {
                for (col, (label, value, sub)) in cols.iter_mut().zip(tiles) {
                    theme::card(t).show(col, |ui| {
                        ui.set_min_width(ui.available_width());
                        ui.label(RichText::new(label).color(t.text_2));
                        ui.label(RichText::new(value).size(26.0).strong().color(t.text));
                        ui.label(RichText::new(if sub.is_empty() { " ".to_string() } else { sub }).small().color(t.muted));
                    });
                }
            });
        }

        let now = chrono::Utc::now().timestamp();
        let cpu: Vec<(i64, f64)> = self.recent.iter().map(|p| (p.ts, p.cpu)).collect();
        let mem: Vec<(i64, f64)> = self.recent.iter().map(|p| (p.ts, p.mem_pct)).collect();
        section(ui, t, "Last 15 minutes");
        ui.label(RichText::new("Hover a chart to see which apps were using it. Click to keep that moment open.").small().color(t.muted));
        let mut clicked = None;
        ui.columns(2, |cols| {
            let spec = |id, title, tip| ChartSpec {
                id,
                title,
                value_fmt: pct_fmt,
                y_max: Some(100.0),
                height: 150.0,
                gap: 3 * self.interval.max(1) + 5,
                x_range: Some((now - 15 * 60, now)),
                marker: self.moment.as_ref().map(Moment::center),
                tip,
                bucket: 1,
            };
            let a = time_chart(&mut cols[0], t, spec("now_cpu", "CPU", Tip::Cpu), &cpu, &mut self.tips, &self.conn);
            let b = time_chart(&mut cols[1], t, spec("now_mem", "Memory", Tip::Mem), &mem, &mut self.tips, &self.conn);
            clicked = a.or(b);
        });
        if let Some(ts) = clicked {
            self.select_at(ts, 60);
        }

        section(ui, t, "What is using your PC");
        ui.horizontal(|ui| {
            ui.label(RichText::new("Sort by").color(t.text_2));
            ui.selectable_value(&mut self.live_sort, AppSort::Cpu, "CPU");
            ui.selectable_value(&mut self.live_sort, AppSort::Memory, "Memory");
            ui.selectable_value(&mut self.live_sort, AppSort::Io, "I/O");
        });
        let mut rows = self.live.clone();
        match self.live_sort {
            AppSort::Cpu => rows.sort_by(|a, b| b.cpu.total_cmp(&a.cpu)),
            AppSort::Memory => rows.sort_by(|a, b| b.mem.cmp(&a.mem)),
            AppSort::Io => rows.sort_by(|a, b| b.io_rate.total_cmp(&a.io_rate)),
        }
        let total_mem = self.latest.as_ref().map(|(_, m)| *m as f64).unwrap_or(1.0);
        theme::card(t).show(ui, |ui| {
            if rows.is_empty() {
                ui.label(RichText::new("Waiting for the first sample.").color(t.muted));
                return;
            }
            egui::Grid::new("live_grid").num_columns(5).striped(true).spacing(vec2(18.0, 8.0)).min_col_width(70.0).show(ui, |ui| {
                for h in ["App", "CPU", "Memory", "I/O", "Processes"] {
                    ui.label(RichText::new(h).small().color(t.muted));
                }
                ui.end_row();
                for r in rows.iter().take(15) {
                    let name = describe::display_name(&r.app, r.description.as_deref());
                    if ui
                        .vertical(|ui| {
                            ui.set_max_width(520.0);
                            ui.label(RichText::new(&name).strong());
                            ui.label(RichText::new(describe::explain(&r.app, r.description.as_deref())).small().color(t.text_2));
                        })
                        .response
                        .interact(Sense::click())
                        .on_hover_text("Open in Apps")
                        .clicked()
                    {
                        self.selected_app = Some(r.app.clone());
                        self.page = Page::Apps;
                        self.range_dirty = true;
                    }
                    ui.label(fmt::pct(r.cpu));
                    ui.label(format!("{}  ({})", fmt::bytes(r.mem as f64), fmt::pct(100.0 * r.mem as f64 / total_mem)));
                    ui.label(fmt::rate(r.io_rate));
                    ui.label(r.procs.to_string());
                    ui.end_row();
                }
            });
        });
    }

    fn page_history(&mut self, ui: &mut Ui, t: &Tokens) {
        heading(ui, t, "History", "How hard your PC worked over time. Hover a chart to see which apps were behind it, click to keep that moment open.");
        self.range_picker(ui);
        let now = chrono::Utc::now().timestamp();
        let x_range = Some((now - self.range.secs(), now));
        let gap = (3 * self.range.bucket()).max(60);
        let series = |f: fn(&db::SysPoint) -> f64| self.history.iter().map(|p| (p.ts, f(p))).collect::<Vec<_>>();
        let procs: Vec<(i64, f64)> = self.history.iter().filter(|p| p.procs > 0.0).map(|p| (p.ts, p.procs)).collect();
        let charts: [(&str, &str, fn(f64) -> String, Option<f64>, Tip, Vec<(i64, f64)>); 5] = [
            ("h_cpu", "CPU", pct_fmt, Some(100.0), Tip::Cpu, series(|p| p.cpu)),
            ("h_mem", "Memory", pct_fmt, Some(100.0), Tip::Mem, series(|p| p.mem_pct)),
            ("h_io", "Disk and file I/O", rate_fmt, None, Tip::Io, series(|p| p.io_rate)),
            ("h_net", "Network", rate_fmt, None, Tip::Net, series(|p| p.net_rate)),
            ("h_procs", "Processes running", count_fmt, None, Tip::Procs, procs),
        ];
        let marker = self.moment.as_ref().map(Moment::center);
        let mut clicked = None;
        let bucket = self.range.bucket();
        for (id, title, value_fmt, y_max, tip, pts) in charts {
            let spec = ChartSpec { id, title, value_fmt, y_max, height: 150.0, gap, x_range, marker, tip, bucket };
            let c = time_chart(ui, t, spec, &pts, &mut self.tips, &self.conn);
            clicked = clicked.or(c);
            ui.add_space(theme::GAP / 2.0);
        }
        if let Some(ts) = clicked {
            self.select_at(ts, self.range.bucket());
        }

        section(ui, t, "Busy times by day and hour");
        ui.label(
            RichText::new("Average CPU for each hour of each weekday in this range. Darker means busier. Click a square to see the most recent time it covers.")
                .color(t.text_2),
        );
        ui.add_space(6.0);
        if let Some((day, hour)) = theme::card(t).show(ui, |ui| heatmap(ui, t, &self.heat)).inner {
            if let Some(from) = latest_weekday_hour(day, hour, self.range.secs()) {
                self.select_window(from, from + 3599);
            }
        }
    }

    fn page_apps(&mut self, ui: &mut Ui, t: &Tokens) {
        heading(ui, t, "Apps", "Which apps used the most resources. Click an app to see its usage over time.");
        self.range_picker(ui);

        if let Some(app) = self.selected_app.clone() {
            let summary = self.apps.iter().find(|a| a.app == app).cloned();
            let desc = summary.as_ref().and_then(|s| s.description.clone());
            theme::card(t).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(describe::display_name(&app, desc.as_deref())).size(17.0).strong());
                    ui.label(RichText::new(&app).color(t.muted));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Close").clicked() {
                            self.selected_app = None;
                        }
                    });
                });
                ui.label(RichText::new(describe::explain(&app, desc.as_deref())).color(t.text_2));
                if let Some(s) = &summary {
                    ui.add_space(4.0);
                    ui.label(format!(
                        "Active {} in this range. CPU averaged {} (peak {}). Memory averaged {} (peak {}). I/O total {}.",
                        fmt::duration(s.minutes * 60),
                        fmt::pct(s.cpu_avg),
                        fmt::pct(s.cpu_peak),
                        fmt::bytes(s.mem_avg),
                        fmt::bytes(s.mem_peak as f64),
                        fmt::bytes(s.io_total as f64)
                    ));
                } else {
                    ui.label(RichText::new("Not recorded in this time range.").color(t.muted));
                }
            });
            let now = chrono::Utc::now().timestamp();
            let gap = (3 * self.range.bucket()).max(180);
            let x_range = Some((now - self.range.secs(), now));
            let cpu: Vec<(i64, f64)> = self.app_series.iter().map(|p| (p.ts, p.cpu)).collect();
            let mem: Vec<(i64, f64)> = self.app_series.iter().map(|p| (p.ts, p.mem)).collect();
            ui.add_space(theme::GAP / 2.0);
            let mut clicked = None;
            ui.columns(2, |cols| {
                let marker = self.moment.as_ref().map(Moment::center);
                let bucket = self.range.bucket().max(60);
                let cpu_spec = ChartSpec { id: "app_cpu", title: "CPU", value_fmt: pct_fmt, y_max: None, height: 140.0, gap, x_range, marker, tip: Tip::None, bucket };
                let mem_spec = ChartSpec { id: "app_mem", title: "Memory", value_fmt: bytes_fmt, y_max: None, height: 140.0, gap, x_range, marker, tip: Tip::None, bucket };
                let a = time_chart(&mut cols[0], t, cpu_spec, &cpu, &mut self.tips, &self.conn);
                let b = time_chart(&mut cols[1], t, mem_spec, &mem, &mut self.tips, &self.conn);
                clicked = a.or(b);
            });
            if let Some(ts) = clicked {
                self.select_at(ts, self.range.bucket());
            }
            ui.add_space(theme::GAP);
        }

        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.app_filter).hint_text("Search apps").desired_width(220.0));
            ui.add_space(12.0);
            ui.label(RichText::new("Rank by").color(t.text_2));
            ui.selectable_value(&mut self.app_sort, AppSort::Cpu, "CPU");
            ui.selectable_value(&mut self.app_sort, AppSort::Memory, "Memory");
            ui.selectable_value(&mut self.app_sort, AppSort::Io, "I/O");
        });
        ui.add_space(6.0);

        let filter = self.app_filter.to_lowercase();
        let mut rows: Vec<db::AppSummary> = self
            .apps
            .iter()
            .filter(|a| {
                filter.is_empty()
                    || a.app.to_lowercase().contains(&filter)
                    || a.description.as_deref().unwrap_or("").to_lowercase().contains(&filter)
            })
            .cloned()
            .collect();
        match self.app_sort {
            AppSort::Cpu => rows.sort_by(|a, b| b.cpu_load.total_cmp(&a.cpu_load)),
            AppSort::Memory => rows.sort_by(|a, b| b.mem_avg.total_cmp(&a.mem_avg)),
            AppSort::Io => rows.sort_by(|a, b| b.io_total.cmp(&a.io_total)),
        }
        let total_load: f64 = self.apps.iter().map(|a| a.cpu_load).sum::<f64>().max(f64::EPSILON);
        let max_bar = match self.app_sort {
            AppSort::Cpu => rows.first().map(|a| a.cpu_load),
            AppSort::Memory => rows.first().map(|a| a.mem_avg),
            AppSort::Io => rows.first().map(|a| a.io_total as f64),
        }
        .unwrap_or(1.0)
        .max(f64::EPSILON);

        theme::card(t).show(ui, |ui| {
            if rows.is_empty() {
                ui.label(RichText::new("No apps recorded in this range yet. Per-app data is saved once a minute.").color(t.muted));
                return;
            }
            egui::Grid::new("apps_grid").num_columns(6).striped(true).spacing(vec2(18.0, 8.0)).show(ui, |ui| {
                for h in ["App", "Share", "CPU avg / peak", "Memory avg / peak", "I/O total", "Active"] {
                    ui.label(RichText::new(h).small().color(t.muted));
                }
                ui.end_row();
                for a in rows.iter().take(60) {
                    let name = describe::display_name(&a.app, a.description.as_deref());
                    let selected = self.selected_app.as_deref() == Some(a.app.as_str());
                    ui.vertical(|ui| {
                        ui.set_max_width(380.0);
                        if ui.selectable_label(selected, RichText::new(&name).strong()).clicked() {
                            self.selected_app = Some(a.app.clone());
                            self.range_dirty = true;
                        }
                        ui.label(RichText::new(describe::explain(&a.app, a.description.as_deref())).small().color(t.text_2));
                    });
                    let v = match self.app_sort {
                        AppSort::Cpu => a.cpu_load,
                        AppSort::Memory => a.mem_avg,
                        AppSort::Io => a.io_total as f64,
                    };
                    ui.horizontal(|ui| {
                        let (rect, _) = ui.allocate_exact_size(vec2(90.0, 8.0), Sense::hover());
                        ui.painter().rect_filled(rect, 4.0, t.surface_alt);
                        let mut fill = rect;
                        fill.set_width((rect.width() * (v / max_bar) as f32).max(2.0));
                        ui.painter().rect_filled(fill, 4.0, t.accent);
                        if self.app_sort == AppSort::Cpu {
                            ui.label(RichText::new(format!("{:.0}%", 100.0 * a.cpu_load / total_load)).color(t.text_2));
                        }
                    });
                    ui.label(format!("{} / {}", fmt::pct(a.cpu_avg), fmt::pct(a.cpu_peak)));
                    ui.label(format!("{} / {}", fmt::bytes(a.mem_avg), fmt::bytes(a.mem_peak as f64)));
                    ui.label(fmt::bytes(a.io_total as f64));
                    ui.label(fmt::duration(a.minutes * 60));
                    ui.end_row();
                }
            });
        });
        ui.add_space(6.0);
        ui.label(
            RichText::new("CPU is shown as a share of your whole processor. Share is each app's part of all CPU used by apps in this range.")
                .small()
                .color(t.muted),
        );
    }

    fn page_insights(&mut self, ui: &mut Ui, t: &Tokens) {
        heading(ui, t, "Insights", "Plain explanations of what slowed your PC down and when.");
        self.range_picker(ui);
        let mut jump = None;
        for i in &self.insights {
            let (color, label) = match i.severity {
                Severity::Info => (t.accent, "Note"),
                Severity::Warning => (t.warning, "Warning"),
                Severity::Critical => (t.critical, "Problem"),
            };
            theme::card(t).show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal(|ui| {
                    status_dot(ui, color);
                    ui.label(RichText::new(label).small().color(t.text_2));
                });
                ui.label(RichText::new(&i.title).size(15.0).strong());
                ui.label(RichText::new(&i.detail).color(t.text_2));
                if let Some(at) = i.at {
                    if ui.link("See what was running then").clicked() {
                        jump = Some(at);
                    }
                }
            });
            ui.add_space(theme::GAP / 2.0);
        }
        if let Some(at) = jump {
            self.select_at(at, self.range.bucket());
            self.page = Page::History;
        }
    }

    fn stop_app(&mut self, app: &str, force: bool) {
        let desc = self.live.iter().find(|r| r.app == app).and_then(|r| r.description.clone());
        let name = describe::display_name(app, desc.as_deref());
        let (msg, is_err) = match procs::stop(app, force) {
            procs::StopResult::AskedToClose => (
                format!("Asked {name} to close. If it has unsaved work, it may ask you to save first."),
                false,
            ),
            procs::StopResult::Ended(n) => {
                (format!("Stopped {name} ({n} process{}).", if n == 1 { "" } else { "es" }), false)
            }
            procs::StopResult::NotRunning => (format!("{name} is not running anymore."), false),
            procs::StopResult::Denied => (
                format!("Windows did not let JPerformance stop {name}. It may be running as administrator."),
                true,
            ),
            procs::StopResult::Protected => (format!("{name} is part of Windows, so JPerformance does not stop it."), true),
        };
        self.run_notice = Some((msg, is_err));
        self.stops.insert(app.to_string(), Instant::now());
        self.last_live = None;
    }

    fn page_running(&mut self, ui: &mut Ui, t: &Tokens) {
        heading(
            ui,
            t,
            "Running apps",
            "Every app running right now. Stopping an app closes it, the same as quitting it. Nothing is uninstalled or deleted, and you can open it again any time.",
        );
        let total_procs: i64 = self.live.iter().map(|r| r.procs).sum();
        ui.label(RichText::new(format!("{} apps, {} processes", self.live.len(), total_procs)).color(t.text_2));
        ui.add_space(6.0);
        if let Some((msg, is_err)) = &self.run_notice {
            ui.horizontal(|ui| {
                status_dot(ui, if *is_err { t.critical } else { t.good });
                ui.label(RichText::new(msg).color(t.text_2));
            });
            ui.add_space(6.0);
        }
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.run_filter).hint_text("Search apps").desired_width(220.0));
            ui.add_space(12.0);
            ui.label(RichText::new("Sort by").color(t.text_2));
            ui.selectable_value(&mut self.run_sort, RunSort::Memory, "Memory");
            ui.selectable_value(&mut self.run_sort, RunSort::Cpu, "CPU");
            ui.selectable_value(&mut self.run_sort, RunSort::Name, "Name");
        });
        ui.add_space(6.0);

        let filter = self.run_filter.to_lowercase();
        let mut rows: Vec<db::LiveRow> = self
            .live
            .iter()
            .filter(|r| {
                filter.is_empty()
                    || r.app.to_lowercase().contains(&filter)
                    || r.description.as_deref().unwrap_or("").to_lowercase().contains(&filter)
            })
            .cloned()
            .collect();
        match self.run_sort {
            RunSort::Memory => rows.sort_by(|a, b| b.mem.cmp(&a.mem)),
            RunSort::Cpu => rows.sort_by(|a, b| b.cpu.total_cmp(&a.cpu)),
            RunSort::Name => rows.sort_by_key(|r| describe::display_name(&r.app, r.description.as_deref()).to_lowercase()),
        }

        let mut stop: Option<String> = None;
        let mut force: Option<String> = None;
        theme::card(t).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            if rows.is_empty() {
                let text = if self.live.is_empty() {
                    "Waiting for the background tracker. Turn on background tracking in Settings if this stays empty."
                } else {
                    "No running apps match your search."
                };
                ui.label(RichText::new(text).color(t.muted));
                return;
            }
            egui::Grid::new("running").num_columns(5).striped(true).spacing(vec2(18.0, 8.0)).show(ui, |ui| {
                for h in ["App", "Processes", "CPU", "Memory", ""] {
                    ui.label(RichText::new(h).small().color(t.muted));
                }
                ui.end_row();
                for r in &rows {
                    ui.vertical(|ui| {
                        ui.set_max_width(460.0);
                        ui.label(RichText::new(describe::display_name(&r.app, r.description.as_deref())).strong());
                        ui.label(RichText::new(describe::explain(&r.app, r.description.as_deref())).small().color(t.text_2));
                    });
                    ui.label(r.procs.to_string());
                    ui.label(fmt::pct(r.cpu));
                    ui.label(fmt::bytes(r.mem as f64));
                    if procs::is_protected(&r.app, r.exe_path.as_deref()) {
                        ui.label(RichText::new("Part of Windows").small().color(t.muted))
                            .on_hover_text("JPerformance does not stop Windows system processes.");
                    } else {
                        match self.stops.get(&r.app).map(|at| at.elapsed()) {
                            Some(e) if e < Duration::from_secs(8) => {
                                ui.label(RichText::new("Closing").color(t.text_2));
                            }
                            Some(_) => {
                                if ui
                                    .button(RichText::new("Force stop").color(t.critical))
                                    .on_hover_text("It is still running. Ends it right away; unsaved work is lost.")
                                    .clicked()
                                {
                                    force = Some(r.app.clone());
                                }
                            }
                            None => {
                                if ui.button("Stop app").clicked() {
                                    stop = Some(r.app.clone());
                                }
                            }
                        }
                    }
                    ui.end_row();
                }
            });
        });
        if let Some(app) = stop {
            self.stop_app(&app, false);
        }
        if let Some(app) = force {
            self.confirm_force = Some(app);
        }
        // Forget stop attempts for apps that have closed.
        let running: std::collections::HashSet<&str> = self.live.iter().map(|r| r.app.as_str()).collect();
        self.stops.retain(|app, at| running.contains(app.as_str()) || at.elapsed() < Duration::from_secs(8));
    }

    fn force_dialog(&mut self, ui: &mut Ui, t: &Tokens) {
        let Some(app) = self.confirm_force.clone() else { return };
        let desc = self.live.iter().find(|r| r.app == app).and_then(|r| r.description.clone());
        let name = describe::display_name(&app, desc.as_deref());
        let resp = egui::Modal::new(egui::Id::unique("confirm_force")).show(ui.ctx(), |ui| {
            ui.set_width(380.0);
            ui.label(RichText::new(format!("Force stop {name}?")).size(16.0).strong());
            ui.label(RichText::new("It did not close when asked. Forcing it ends it right away, and any unsaved work in it is lost.").color(t.text_2));
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui.button(RichText::new("Force stop").color(t.critical)).clicked() {
                    self.stop_app(&app, true);
                    self.confirm_force = None;
                }
                if ui.button("Cancel").clicked() {
                    self.confirm_force = None;
                }
            });
        });
        if resp.should_close() {
            self.confirm_force = None;
        }
    }

    fn page_settings(&mut self, ui: &mut Ui, t: &Tokens) {
        heading(ui, t, "Settings", "");
        if let Some((msg, is_err)) = &self.notice {
            ui.colored_label(if *is_err { t.critical } else { t.text_2 }, msg);
            ui.add_space(8.0);
        }

        theme::card(t).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.label(RichText::new("Background tracking").strong());
            ui.label(
                RichText::new("A small helper process records performance even when this window is closed.").color(t.text_2),
            );
            ui.add_space(6.0);
            let mut on = self.tracking;
            if ui.checkbox(&mut on, "Record performance in the background").changed() {
                self.set_tracking(on);
            }
            let mut auto = self.autostart;
            if ui.checkbox(&mut auto, "Start tracking when I sign in to Windows").changed() {
                match autostart::set_enabled(auto) {
                    Ok(()) => self.autostart = auto,
                    Err(e) => self.notice = Some((format!("Could not change startup setting: {e}"), true)),
                }
            }
            if let Some(s) = &self.status {
                ui.add_space(4.0);
                ui.label(
                    RichText::new(if self.collector_running {
                        format!("Running as process {} since {}.", s.pid, fmt::day_time(s.started))
                    } else {
                        format!("Last ran as process {}, last sample {}.", s.pid, fmt::ago(s.heartbeat))
                    })
                    .small()
                    .color(t.muted),
                );
            }
        });
        ui.add_space(theme::GAP);

        theme::card(t).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.label(RichText::new("Recording").strong());
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label("Take a sample every");
                for s in [2i64, 5, 10, 30] {
                    if ui.selectable_label(self.interval == s, format!("{s} s")).clicked() {
                        self.interval = s;
                        let _ = db::set_setting(&self.conn, db::KEY_INTERVAL, &s.to_string());
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.label("Keep history for");
                for d in [7i64, 14, 30, 90] {
                    if ui.selectable_label(self.retention == d, format!("{d} days")).clicked() {
                        self.retention = d;
                        let _ = db::set_setting(&self.conn, db::KEY_RETENTION, &d.to_string());
                    }
                }
            });
            ui.label(RichText::new("Older samples are removed automatically once an hour.").small().color(t.muted));
        });
        ui.add_space(theme::GAP);

        theme::card(t).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.label(RichText::new("Your data").strong());
            let path = paths::db_path();
            ui.label(RichText::new(format!("{}  ({})", path.display(), fmt::bytes(db::db_size_bytes(&path) as f64))).color(t.text_2));
            ui.label(RichText::new("Everything stays on this PC. Nothing is uploaded.").small().color(t.muted));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button("Open data folder").clicked() {
                    let _ = std::process::Command::new("explorer").arg(paths::data_dir()).spawn();
                }
                if ui.button(RichText::new("Clear history").color(t.critical)).clicked() {
                    self.confirm_clear = true;
                }
            });
        });
    }

    fn clear_dialog(&mut self, ui: &mut Ui, t: &Tokens) {
        let resp = egui::Modal::new(egui::Id::unique("confirm_clear")).show(ui.ctx(), |ui| {
            ui.set_width(360.0);
            ui.label(RichText::new("Clear all recorded history?").size(16.0).strong());
            ui.label(RichText::new("This permanently deletes every sample and app record. Settings are kept.").color(t.text_2));
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui.button(RichText::new("Delete history").color(t.critical)).clicked() {
                    match db::clear_history(&self.conn) {
                        Ok(()) => self.notice = Some(("History cleared.".into(), false)),
                        Err(e) => self.notice = Some((format!("Could not clear history: {e}"), true)),
                    }
                    self.range_dirty = true;
                    self.last_live = None;
                    self.confirm_clear = false;
                }
                if ui.button("Cancel").clicked() {
                    self.confirm_clear = false;
                }
            });
        });
        if resp.should_close() {
            self.confirm_clear = false;
        }
    }
}

/// Weekday by hour grid. Rows start on Monday.
/// Returns the (weekday, hour) the user clicked, weekday 0 = Sunday.
fn heatmap(ui: &mut Ui, t: &Tokens, grid: &[[Option<f64>; 24]; 7]) -> Option<(usize, usize)> {
    let mut clicked = None;
    const DAYS: [(&str, usize); 7] = [("Mon", 1), ("Tue", 2), ("Wed", 3), ("Thu", 4), ("Fri", 5), ("Sat", 6), ("Sun", 0)];
    let max = grid.iter().flatten().flatten().cloned().fold(0.0f64, f64::max).max(1.0);
    let label_w = 40.0;
    let avail = ui.available_width() - label_w;
    let cell = ((avail / 24.0) - 2.0).clamp(10.0, 34.0);
    let row_h = 22.0;
    let painter_font = egui::FontId::proportional(11.0);

    // Hour labels.
    let (rect, _) = ui.allocate_exact_size(vec2(label_w + 24.0 * (cell + 2.0), 16.0), Sense::hover());
    for h in (0..24).step_by(3) {
        let x = rect.left() + label_w + h as f32 * (cell + 2.0);
        ui.painter().text(egui::pos2(x, rect.center().y), egui::Align2::LEFT_CENTER, format!("{h:02}"), painter_font.clone(), t.muted);
    }
    for (name, d) in DAYS {
        let (rect, resp) = ui.allocate_exact_size(vec2(label_w + 24.0 * (cell + 2.0), row_h), Sense::click());
        ui.painter().text(egui::pos2(rect.left(), rect.center().y), egui::Align2::LEFT_CENTER, name, painter_font.clone(), t.text_2);
        let mut hovered = None;
        for h in 0..24 {
            let x = rect.left() + label_w + h as f32 * (cell + 2.0);
            let r = egui::Rect::from_min_size(egui::pos2(x, rect.top() + 1.0), vec2(cell, row_h - 2.0));
            let fill = match grid[d][h] {
                Some(v) => theme::seq(t, v / max),
                None => t.surface_alt,
            };
            ui.painter().rect_filled(r, 3.0, fill);
            if resp.hover_pos().is_some_and(|p| r.expand(1.0).contains(p)) {
                ui.painter().rect_stroke(r, 3.0, Stroke::new(1.5, t.text), egui::StrokeKind::Outside);
                hovered = Some(h);
            }
        }
        if let Some(h) = hovered {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            if resp.clicked() {
                clicked = Some((d, h));
            }
            let text = match grid[d][h] {
                Some(v) => format!("{name} {h:02}:00 to {:02}:00\nAverage CPU {}", (h + 1) % 24, fmt::pct(v)),
                None => format!("{name} {h:02}:00\nNo data"),
            };
            resp.on_hover_text_at_pointer(text);
        }
    }
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new("0%").small().color(t.muted));
        let (rect, _) = ui.allocate_exact_size(vec2(160.0, 10.0), Sense::hover());
        let steps = 32;
        for i in 0..steps {
            let w = rect.width() / steps as f32;
            let r = egui::Rect::from_min_size(egui::pos2(rect.left() + i as f32 * w, rect.top()), vec2(w + 0.5, rect.height()));
            ui.painter().rect_filled(r, 0.0, theme::seq(t, i as f64 / (steps - 1) as f64));
        }
        ui.label(RichText::new(format!("{} average CPU", fmt::pct(max))).small().color(t.muted));
        ui.add_space(12.0);
        let (sw, _) = ui.allocate_exact_size(vec2(12.0, 10.0), Sense::hover());
        ui.painter().rect_filled(sw, 2.0, t.surface_alt);
        ui.label(RichText::new("no data").small().color(t.muted));
    });
    clicked
}

/// Start of the most recent hour within `range_secs` that falls on local `weekday` (0 = Sunday) and `hour`.
fn latest_weekday_hour(weekday: usize, hour: usize, range_secs: i64) -> Option<i64> {
    use chrono::{Datelike, Timelike};
    let now = chrono::Utc::now().timestamp();
    let mut ts = now;
    while now - ts <= range_secs + 3600 {
        let l = fmt::local(ts);
        if l.weekday().num_days_from_sunday() as usize == weekday && l.hour() as usize == hour {
            return Some(ts - (l.minute() as i64) * 60 - l.second() as i64);
        }
        ts -= 3600;
    }
    None
}
