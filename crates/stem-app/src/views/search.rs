//! Sample search on the start screen: words typed into the URL box (not a link, not a path)
//! search YouTube or SoundCloud through yt-dlp, and each hit can be split now or queued.
//!
//! The search runs on a worker thread with its own cancel flag; [`poll`] collects the result
//! each frame. Idea from danielravina/stemkit (no code taken).

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crossbeam_channel::{Receiver, unbounded};
use eframe::egui::{
    self, Align2, Color32, CornerRadius, CursorIcon, Galley, Id, Rect, Sense, Stroke, StrokeKind,
    Ui, pos2, vec2,
};
use stem_fetch::{FetchProgress, Fetcher, SEARCH_LIMIT, SearchResult, SearchSite};

use crate::app::StemApp;
use crate::pipeline::InputSource;
use crate::theme::{self, motion};
use crate::widgets::buttons::{self, Button};
use crate::widgets::icons::{self, Icon};
use crate::widgets::{overline, progress};

/// Searches remembered in Settings.
const MAX_RECENT: usize = 6;
/// Rows shown before "Show more".
const FOLDED: usize = 8;
const ROW_H: f32 = 50.0;

/// Brand dots, as on the start screen's platform chips.
fn site_color(site: SearchSite) -> Color32 {
    match site {
        SearchSite::YouTube => Color32::from_rgb(255, 72, 72),
        SearchSite::SoundCloud => Color32::from_rgb(255, 136, 40),
    }
}

#[derive(Default)]
pub enum Status {
    #[default]
    Idle,
    Running {
        /// What yt-dlp is doing ("Downloading yt-dlp (first run only)"), if anything.
        detail: String,
    },
    Done(Vec<SearchResult>),
    Failed(String),
}

enum Msg {
    Detail(String),
    Done(Result<Vec<SearchResult>, String>),
}

/// The current search and its results.
#[derive(Default)]
pub struct Search {
    pub query: String,
    pub site: SearchSite,
    pub status: Status,
    /// Results the user already split or queued (by URL), to mark their rows.
    pub used: HashSet<String>,
    /// Show every row, not just the first [`FOLDED`].
    pub expanded: bool,
    rx: Option<Receiver<Msg>>,
    cancel: Arc<AtomicBool>,
}

impl Search {
    pub fn is_busy(&self) -> bool {
        matches!(self.status, Status::Running { .. })
    }

    fn cancel(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.rx = None;
    }
}

/// Start searching `site` for `query`, replacing any search in flight.
pub fn start(app: &mut StemApp, ctx: &egui::Context, site: SearchSite, query: &str) {
    let query = query.split_whitespace().collect::<Vec<_>>().join(" ");
    if query.is_empty() {
        return;
    }
    let s = &mut app.search;
    s.cancel();
    s.query = query.clone();
    s.site = site;
    s.status = Status::Running {
        detail: String::new(),
    };
    s.expanded = false;
    s.used.clear();
    let cancel = Arc::new(AtomicBool::new(false));
    s.cancel = cancel.clone();
    let (tx, rx) = unbounded();
    s.rx = Some(rx);

    app.settings.search_site = site;
    remember(&mut app.settings.recent_searches, &query);

    let paths = app.backend.paths.clone();
    let demo = app.demo();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let send = |m: Msg| {
            let _ = tx.send(m);
            ctx.request_repaint();
        };
        let result = if demo {
            std::thread::sleep(Duration::from_millis(700));
            Ok(demo_results(site))
        } else {
            let f = Fetcher::new(paths.tools.clone(), paths.downloads.clone());
            let mut on_progress = |p: FetchProgress| {
                if let FetchProgress::Preparing(msg) = p {
                    send(Msg::Detail(msg));
                }
            };
            f.search(site, &query, SEARCH_LIMIT, &mut on_progress, &cancel)
                .map_err(|e| e.to_string())
        };
        if !cancel.load(Ordering::Relaxed) {
            send(Msg::Done(result));
        }
    });
}

/// Newest first, no duplicates (ignoring case), at most [`MAX_RECENT`].
fn remember(recent: &mut Vec<String>, query: &str) {
    recent.retain(|q| !q.eq_ignore_ascii_case(query));
    recent.insert(0, query.to_owned());
    recent.truncate(MAX_RECENT);
}

/// Collect the worker's messages. Call every frame.
pub fn poll(app: &mut StemApp) {
    let Some(rx) = &app.search.rx else { return };
    let mut done = None;
    while let Ok(m) = rx.try_recv() {
        match m {
            Msg::Detail(d) => {
                if let Status::Running { detail } = &mut app.search.status {
                    *detail = d;
                }
            }
            Msg::Done(r) => done = Some(r),
        }
    }
    let Some(r) = done else { return };
    app.search.rx = None;
    match r {
        Ok(results) => app.search.status = Status::Done(results),
        Err(e) => {
            app.toasts.error("Search failed", e.clone());
            app.search.status = Status::Failed(e);
        }
    }
}

/// The "Search YouTube" / "Search SoundCloud" chips shown under the URL box while it holds a
/// query. Enter searches the highlighted site.
pub fn site_chips(app: &mut StemApp, ui: &mut Ui, query: &str) {
    let selected = app.settings.search_site;
    let labels: Vec<_> = SearchSite::ALL
        .iter()
        .map(|s| format!("Search {}", s.label()))
        .collect();
    let hint = format!("Enter searches {}", selected.label());
    let widths: f32 = labels
        .iter()
        .map(|l| {
            ui.painter()
                .layout_no_wrap(l.clone(), theme::semi(12.0), theme::TEXT)
                .size()
                .x
                + 26.0
        })
        .sum::<f32>()
        + 8.0;
    let hint_g = ui
        .painter()
        .layout_no_wrap(hint, theme::regular(12.0), theme::TEXT_FAINT);
    let total = widths + 14.0 + hint_g.size().x;
    let mut go = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.add_space(((ui.available_width() - total) / 2.0).max(0.0));
        for (site, label) in SearchSite::ALL.iter().zip(&labels) {
            let resp = buttons::chip(ui, label, site_color(*site), *site == selected)
                .on_hover_cursor(CursorIcon::PointingHand)
                .on_hover_text(format!("Search {} for “{query}”", site.label()));
            if resp.clicked() {
                go = Some(*site);
            }
        }
        ui.add_space(6.0);
        let (r, _) = ui.allocate_exact_size(hint_g.size() + vec2(0.0, 4.0), Sense::hover());
        ui.painter().galley(
            pos2(r.left(), r.center().y - hint_g.size().y / 2.0),
            hint_g,
            theme::TEXT_FAINT,
        );
    });
    if let Some(site) = go {
        start(app, ui.ctx(), site, query);
    }
}

/// Recent searches as small chips (start screen, when the URL box is empty).
pub fn recent_row(app: &mut StemApp, ui: &mut Ui) {
    if app.settings.recent_searches.is_empty() {
        return;
    }
    let items: Vec<String> = app.settings.recent_searches.clone();
    let label = ui.painter().layout_no_wrap(
        "Recent searches".into(),
        theme::regular(12.0),
        theme::TEXT_FAINT,
    );
    let galleys: Vec<_> = items
        .iter()
        .map(|q| {
            ui.painter().layout_no_wrap(
                crate::widgets::toast::elide(q, 22),
                theme::regular(12.0),
                theme::TEXT_DIM,
            )
        })
        .collect();
    // As many chips as fit on one centred line.
    let avail = ui.available_width();
    let mut total = label.size().x + 8.0;
    let mut n = 0;
    for g in &galleys {
        let w = g.size().x + 30.0 + 6.0;
        if total + w > avail {
            break;
        }
        total += w;
        n += 1;
    }
    let mut go = None;
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        ui.add_space(((avail - total) / 2.0).max(0.0));
        let (r, _) = ui.allocate_exact_size(vec2(label.size().x + 2.0, 22.0), Sense::hover());
        let ly = r.center().y - label.size().y / 2.0;
        ui.painter()
            .galley(pos2(r.left(), ly), label, theme::TEXT_FAINT);
        for (q, g) in items.iter().zip(galleys).take(n) {
            let (r, resp) = ui.allocate_exact_size(vec2(g.size().x + 30.0, 22.0), Sense::click());
            let resp = resp
                .on_hover_cursor(CursorIcon::PointingHand)
                .on_hover_text(format!("Search {} again", app.settings.search_site.label()));
            let h = ui
                .ctx()
                .animate_bool_with_time(resp.id, resp.hovered(), motion::FAST);
            let p = ui.painter();
            p.rect(
                r,
                CornerRadius::same(11),
                theme::lerp_color(Color32::from_white_alpha(4), theme::CARD_HI, h),
                Stroke::new(
                    1.0,
                    theme::lerp_color(Color32::from_white_alpha(12), theme::STROKE_HI, h),
                ),
                StrokeKind::Inside,
            );
            icons::paint(
                p,
                Rect::from_center_size(pos2(r.left() + 12.0, r.center().y), vec2(11.0, 11.0)),
                Icon::Search,
                theme::TEXT_FAINT,
            );
            p.galley(
                pos2(r.left() + 22.0, r.center().y - g.size().y / 2.0),
                g,
                theme::lerp_color(theme::TEXT_DIM, theme::TEXT, h),
            );
            if resp.clicked() {
                go = Some(q.clone());
            }
        }
    });
    if let Some(q) = go {
        app.url_input = q.clone();
        let site = app.settings.search_site;
        start(app, &ui.ctx().clone(), site, &q);
    }
}

enum Action {
    Split(SearchResult),
    Queue(SearchResult),
    Retry,
    Cancel,
    Close,
    Expand,
}

/// The results card. Draws nothing while no search has been made.
pub fn panel(app: &mut StemApp, ui: &mut Ui) {
    if matches!(app.search.status, Status::Idle) {
        return;
    }
    let t = ui.input(|i| i.time);
    let mut action = None;
    let s = &app.search;
    let color = site_color(s.site);
    ui.add_space(14.0);
    crate::widgets::paint::glass_card(ui, egui::Margin::symmetric(18, 14), None, |ui| {
        ui.set_width(ui.available_width());
        // Header: what was searched, where, and how it went.
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let (r, _) = ui.allocate_exact_size(vec2(10.0, 18.0), Sense::hover());
            ui.painter().circle_filled(r.center(), 4.0, color);
            overline(ui, &format!("{} search", s.site.label()));
            let summary = match &s.status {
                Status::Done(r) if r.is_empty() => "no results".to_owned(),
                Status::Done(r) => {
                    format!("{} result{}", r.len(), if r.len() == 1 { "" } else { "s" })
                }
                _ => String::new(),
            };
            let right_w = 110.0;
            let max_q = (ui.available_width() - right_w - 120.0).max(60.0);
            let q = one_line(
                ui,
                format!("“{}”", s.query),
                theme::semi(13.5),
                theme::TEXT,
                max_q,
            );
            let (qr, _) = ui.allocate_exact_size(q.size(), Sense::hover());
            ui.painter().galley(qr.min, q, theme::TEXT);
            if !summary.is_empty() {
                ui.label(
                    egui::RichText::new(summary)
                        .font(theme::mono(11.5))
                        .color(theme::TEXT_DIM),
                );
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let busy = s.is_busy();
                let tip = if busy {
                    "Stop searching"
                } else {
                    "Close the results"
                };
                if buttons::icon_button(ui, Icon::Close, 26.0, false, theme::BAD, tip).clicked() {
                    action = Some(if busy { Action::Cancel } else { Action::Close });
                }
            });
        });
        ui.add_space(8.0);
        match &s.status {
            Status::Idle => {}
            Status::Running { detail } => {
                let w = ui.available_width();
                let (r, _) = ui.allocate_exact_size(vec2(w, 64.0), Sense::hover());
                let p = ui.painter();
                let c = pos2(r.left() + 22.0, r.center().y - 6.0);
                progress::dots(p, c, 7.0, t, theme::ACCENT);
                let msg = if detail.is_empty() {
                    format!("Searching {}…", s.site.label())
                } else {
                    format!("{detail}…")
                };
                p.text(
                    pos2(r.left() + 44.0, c.y),
                    Align2::LEFT_CENTER,
                    msg,
                    theme::semi(13.5),
                    theme::TEXT_DIM,
                );
                let bar = Rect::from_min_size(
                    pos2(r.left() + 44.0, c.y + 16.0),
                    vec2((w - 60.0).min(260.0), 4.0),
                );
                progress::bar(p, bar, None, t);
            }
            Status::Failed(msg) => {
                ui.horizontal(|ui| {
                    ui.add_space(4.0);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(msg)
                                .font(theme::regular(13.0))
                                .color(theme::lerp_color(theme::BAD, theme::TEXT, 0.25)),
                        )
                        .wrap(),
                    );
                });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if Button::new("Try again")
                        .icon(Icon::Refresh)
                        .height(30.0)
                        .show(ui)
                        .clicked()
                    {
                        action = Some(Action::Retry);
                    }
                });
            }
            Status::Done(results) if results.is_empty() => {
                ui.label(
                    egui::RichText::new(format!(
                        "Nothing found on {}. Try fewer or different words.",
                        s.site.label()
                    ))
                    .font(theme::regular(13.0))
                    .color(theme::TEXT_DIM),
                );
            }
            Status::Done(results) => {
                let shown = if s.expanded {
                    results.len()
                } else {
                    results.len().min(FOLDED)
                };
                for (idx, r) in results.iter().take(shown).enumerate() {
                    if let Some(a) = row(ui, idx, r, s.used.contains(&r.url)) {
                        action = Some(a);
                    }
                    ui.add_space(4.0);
                }
                if shown < results.len() {
                    ui.add_space(2.0);
                    ui.vertical_centered(|ui| {
                        let label = format!("Show {} more", results.len() - shown);
                        if Button::ghost(&label).height(28.0).show(ui).clicked() {
                            action = Some(Action::Expand);
                        }
                    });
                }
            }
        }
    });
    ui.add_space(4.0);

    let now = ui.input(|i| i.time);
    match action {
        Some(Action::Split(r)) => {
            app.search.used.insert(r.url.clone());
            app.start_job(InputSource::Url(r.url), now);
        }
        Some(Action::Queue(r)) => {
            app.search.used.insert(r.url.clone());
            let id = app.queue.push(InputSource::Url(r.url), false);
            if let Some(item) = app.queue.get_mut(id) {
                item.title = r.title.clone();
            }
            app.toasts.info("Added to the queue", r.title);
        }
        Some(Action::Retry) => {
            let (site, q) = (app.search.site, app.search.query.clone());
            start(app, &ui.ctx().clone(), site, &q);
        }
        Some(Action::Cancel) | Some(Action::Close) => {
            app.search.cancel();
            app.search.status = Status::Idle;
        }
        Some(Action::Expand) => app.search.expanded = true,
        None => {}
    }
}

/// One result row: number, title over channel · views, duration, Split and Queue.
fn row(ui: &mut Ui, idx: usize, r: &SearchResult, used: bool) -> Option<Action> {
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(w, ROW_H), Sense::hover());
    let hover = ui.ctx().animate_bool_with_time(
        Id::unique(("search-row", &r.url)),
        resp.hovered(),
        motion::FAST,
    );
    let p = ui.painter().clone();
    p.rect(
        rect,
        CornerRadius::same(12),
        theme::lerp_color(theme::alpha(theme::CARD_HI, 0.45), theme::CARD_HI, hover),
        Stroke::new(1.0, theme::STROKE),
        StrokeKind::Inside,
    );
    p.text(
        pos2(rect.left() + 16.0, rect.center().y),
        Align2::LEFT_CENTER,
        format!("{:02}", idx + 1),
        theme::mono(11.5),
        theme::TEXT_FAINT,
    );

    // Actions first (right to left), so the text gets exactly the room they leave.
    // Narrow rows (small windows) drop the button labels and keep the icons.
    let compact = w < 600.0;
    let ar = Rect::from_min_max(
        pos2(rect.left() + 48.0, rect.top()),
        pos2(rect.right() - 10.0, rect.bottom()),
    );
    let mut au = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(ar)
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    au.spacing_mut().item_spacing.x = 6.0;
    let mut action = None;
    let queue_label = match (compact, used) {
        (true, _) => "",
        (false, true) => "Queued",
        (false, false) => "Queue",
    };
    let queue_tip = if used {
        "Already split or queued; add it again"
    } else {
        "Add to the queue: tracks split one after another"
    };
    if Button::ghost(queue_label)
        .icon(if used { Icon::Check } else { Icon::Plus })
        .height(30.0)
        .min_width(if compact { 0.0 } else { 118.0 })
        .tooltip(queue_tip)
        .show(&mut au)
        .clicked()
    {
        action = Some(Action::Queue(r.clone()));
    }
    if Button::new(if compact { "" } else { "Split" })
        .icon(Icon::Split)
        .height(30.0)
        .tooltip("Download and split this now (queues it if something is running)")
        .show(&mut au)
        .clicked()
    {
        action = Some(Action::Split(r.clone()));
    }
    let actions_left = au.min_rect().left();

    let dur = r.duration_secs.map_or("–".to_owned(), duration);
    let dur_g = p.layout_no_wrap(dur, theme::mono(12.0), theme::TEXT_DIM);
    let dur_right = actions_left - 14.0;
    let text_x = rect.left() + 48.0;
    let text_w = (dur_right - dur_g.size().x - 20.0 - text_x).max(40.0);
    p.galley(
        pos2(
            dur_right - dur_g.size().x,
            rect.center().y - dur_g.size().y / 2.0,
        ),
        dur_g,
        theme::TEXT_DIM,
    );
    let title = one_line(ui, r.title.clone(), theme::semi(13.5), theme::TEXT, text_w);
    p.galley(
        pos2(text_x, rect.top() + 17.0 - title.size().y / 2.0),
        title,
        theme::TEXT,
    );
    let mut sub = r.uploader.clone().unwrap_or_default();
    if let Some(v) = r.views {
        if !sub.is_empty() {
            sub.push_str("  ·  ");
        }
        sub.push_str(&views(v));
    }
    if sub.is_empty() {
        sub = host(&r.url);
    }
    let sub = one_line(ui, sub, theme::regular(12.0), theme::TEXT_FAINT, text_w);
    p.galley(
        pos2(text_x, rect.top() + 34.0 - sub.size().y / 2.0),
        sub,
        theme::TEXT_FAINT,
    );
    // Hover the text for the full title and link.
    let text_rect = Rect::from_min_max(
        pos2(text_x, rect.top()),
        pos2(text_x + text_w, rect.bottom()),
    );
    ui.interact(
        text_rect,
        Id::unique(("search-tip", &r.url)),
        Sense::hover(),
    )
    .on_hover_text(format!("{}\n{}", r.title, r.url));
    action
}

/// `text` on one line, cut with an ellipsis at `max_w` points.
fn one_line(ui: &Ui, text: String, font: egui::FontId, color: Color32, max_w: f32) -> Arc<Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text, font, color);
    job.wrap.max_width = max_w.max(10.0);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    ui.painter().fonts_mut(|f| f.layout_job(job))
}

/// 158.0 -> "2:38", 3725.0 -> "1:02:05".
pub fn duration(secs: f64) -> String {
    let s = secs.round().max(0.0) as u64;
    let (h, m, s) = (s / 3600, (s / 60) % 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// 4_812_337 -> "4.8M views".
fn views(n: u64) -> String {
    let short = match n {
        0..1_000 => n.to_string(),
        1_000..1_000_000 => trim_one(n as f64 / 1e3, "K"),
        1_000_000..1_000_000_000 => trim_one(n as f64 / 1e6, "M"),
        _ => trim_one(n as f64 / 1e9, "B"),
    };
    format!("{short} view{}", if n == 1 { "" } else { "s" })
}

fn trim_one(x: f64, unit: &str) -> String {
    if x >= 100.0 {
        format!("{x:.0}{unit}")
    } else {
        let s = format!("{x:.1}");
        format!("{}{unit}", s.trim_end_matches(".0"))
    }
}

fn host(url: &str) -> String {
    url.split("://")
        .nth(1)
        .and_then(|r| r.split('/').next())
        .unwrap_or(url)
        .trim_start_matches("www.")
        .to_owned()
}

/// Stand-in results for demo mode and screenshots.
pub fn demo_results(site: SearchSite) -> Vec<SearchResult> {
    let rows: &[(&str, &str, f64, u64)] = match site {
        SearchSite::YouTube => &[
            (
                "The Winstons - Amen, Brother (Original Amen Break)",
                "Break Archive",
                158.0,
                4_812_337,
            ),
            (
                "Amen Break - 136 BPM drum loop (free sample)",
                "drumtrax",
                33.0,
                120_455,
            ),
            (
                "Dusty soul sample 1972 - Eb major, 92 BPM (no drums)",
                "Crate Digger",
                94.0,
                38_120,
            ),
            (
                "Bob James - Nautilus (full album version, remastered 2004 edition with liner notes)",
                "CTI Records",
                305.0,
                2_204_998,
            ),
            (
                "Vintage Rhodes chords for sampling | royalty free loop pack vol. 3",
                "Keys & Loops",
                412.0,
                9_870,
            ),
            (
                "How the amen break changed music",
                "Sample History",
                1_085.0,
                731_002,
            ),
            ("lofi drum break 85bpm", "beatsbyjm", 72.0, 4_412),
            (
                "Funky Drummer (isolated drums)",
                "Breaks Club",
                187.0,
                99_400,
            ),
            (
                "Soul flip tutorial - chopping on the MPC",
                "Kei",
                731.0,
                18_904,
            ),
            (
                "Think (About It) - break only",
                "Break Archive",
                41.0,
                655_310,
            ),
        ],
        SearchSite::SoundCloud => &[
            ("Soul Sample 1972", "Crate Digger", 94.5, 20_311),
            ("chopped soul sample pack", "beatsbyjm", 61.0, 0),
            ("Free Soul Sample - \"Velvet\"", "Loop Kitchen", 61.0, 4_100),
            ("Dusty soul loop (prod. kei)", "kei", 128.0, 1_830),
        ],
    };
    rows.iter()
        .enumerate()
        .map(|(i, (title, up, dur, v))| SearchResult {
            url: match site {
                SearchSite::YouTube => format!("https://www.youtube.com/watch?v=demo{i}"),
                SearchSite::SoundCloud => format!("https://soundcloud.com/demo/{i}"),
            },
            title: (*title).to_owned(),
            uploader: Some((*up).to_owned()),
            duration_secs: Some(*dur),
            views: (*v > 0).then_some(*v),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_durations_and_views() {
        assert_eq!(duration(158.0), "2:38");
        assert_eq!(duration(33.4), "0:33");
        assert_eq!(duration(3725.0), "1:02:05");
        assert_eq!(views(1), "1 view");
        assert_eq!(views(999), "999 views");
        assert_eq!(views(1_000), "1K views");
        assert_eq!(views(120_455), "120K views");
        assert_eq!(views(4_812_337), "4.8M views");
        assert_eq!(views(2_000_000_000), "2B views");
        assert_eq!(host("https://www.youtube.com/watch?v=x"), "youtube.com");
    }

    #[test]
    fn remembers_recent_searches() {
        let mut r = Vec::new();
        for q in ["a", "b", "A", "c", "d", "e", "f", "g"] {
            remember(&mut r, q);
        }
        assert_eq!(r, ["g", "f", "e", "d", "c", "A"]);
    }
}
