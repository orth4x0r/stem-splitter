//! Vector icons painted with strokes, so they stay crisp at any DPI and need no icon font.

use std::f32::consts::{FRAC_PI_2, PI, TAU};

use eframe::egui::{Color32, Painter, Rect, Shape, Stroke, pos2, vec2};

use super::paint::arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Play,
    Pause,
    Stop,
    Split,
    Library,
    Models,
    Settings,
    Folder,
    Download,
    Link,
    Scissors,
    Check,
    Close,
    Chip,
    Loop,
    Headphones,
    Export,
    Refresh,
    Plus,
    Sparkle,
    Search,
}

/// Paint `icon` inside `rect` (square-ish), using `color`.
pub fn paint(p: &Painter, rect: Rect, icon: Icon, color: Color32) {
    let c = rect.center();
    let s = rect.width().min(rect.height());
    let u = s / 16.0; // design grid: 16x16
    let w = (1.6 * u).max(1.2);
    let st = Stroke::new(w, color);
    let at = |x: f32, y: f32| pos2(c.x + (x - 8.0) * u, c.y + (y - 8.0) * u);
    let line = |pts: &[(f32, f32)]| {
        p.add(Shape::line(
            pts.iter().map(|&(x, y)| at(x, y)).collect(),
            st,
        ));
    };
    match icon {
        Icon::Play => {
            p.add(Shape::convex_polygon(
                vec![at(4.5, 2.5), at(13.5, 8.0), at(4.5, 13.5)],
                color,
                Stroke::new(w * 0.6, color),
            ));
        }
        Icon::Pause => {
            for x in [4.0, 9.5] {
                p.rect_filled(Rect::from_min_max(at(x, 2.5), at(x + 2.6, 13.5)), u, color);
            }
        }
        Icon::Stop => {
            p.rect_filled(
                Rect::from_min_max(at(3.5, 3.5), at(12.5, 12.5)),
                1.5 * u,
                color,
            );
        }
        Icon::Split => {
            // Waveform splitting into three lanes.
            line(&[(1.5, 8.0), (4.0, 8.0)]);
            line(&[(4.0, 8.0), (7.0, 3.5), (14.5, 3.5)]);
            line(&[(4.0, 8.0), (14.5, 8.0)]);
            line(&[(4.0, 8.0), (7.0, 12.5), (14.5, 12.5)]);
        }
        Icon::Library => {
            line(&[(3.0, 2.5), (3.0, 13.5)]);
            line(&[(6.5, 2.5), (6.5, 13.5)]);
            line(&[(10.0, 3.0), (13.5, 13.0)]);
            line(&[(1.5, 13.5), (14.5, 13.5)]);
        }
        Icon::Models => {
            // Stacked layers.
            line(&[(8.0, 2.0), (14.0, 5.0), (8.0, 8.0), (2.0, 5.0), (8.0, 2.0)]);
            line(&[(2.0, 8.0), (8.0, 11.0), (14.0, 8.0)]);
            line(&[(2.0, 11.0), (8.0, 14.0), (14.0, 11.0)]);
        }
        Icon::Settings => {
            // Sliders.
            line(&[(2.0, 4.5), (14.0, 4.5)]);
            line(&[(2.0, 11.5), (14.0, 11.5)]);
            p.circle(at(10.5, 4.5), 1.8 * u, Color32::TRANSPARENT, st);
            p.circle(at(5.5, 11.5), 1.8 * u, Color32::TRANSPARENT, st);
            p.circle_filled(at(10.5, 4.5), 1.0 * u, color);
            p.circle_filled(at(5.5, 11.5), 1.0 * u, color);
        }
        Icon::Folder => {
            line(&[
                (1.5, 4.0),
                (1.5, 13.0),
                (14.5, 13.0),
                (14.5, 5.5),
                (8.0, 5.5),
                (6.5, 3.0),
                (1.5, 3.0),
                (1.5, 4.0),
            ]);
        }
        Icon::Download => {
            line(&[(8.0, 2.0), (8.0, 10.5)]);
            line(&[(4.5, 7.0), (8.0, 10.5), (11.5, 7.0)]);
            line(&[(2.5, 11.5), (2.5, 13.5), (13.5, 13.5), (13.5, 11.5)]);
        }
        Icon::Export => {
            line(&[(8.0, 10.5), (8.0, 2.0)]);
            line(&[(4.5, 5.5), (8.0, 2.0), (11.5, 5.5)]);
            line(&[(2.5, 9.5), (2.5, 13.5), (13.5, 13.5), (13.5, 9.5)]);
        }
        Icon::Link => {
            let r = 2.6 * u;
            let a = at(5.5, 10.5);
            let b = at(10.5, 5.5);
            arc(p, a, r, FRAC_PI_2 * 0.5, FRAC_PI_2 * 0.5 + PI * 1.25, st);
            arc(p, b, r, -FRAC_PI_2 * 1.5, -FRAC_PI_2 * 1.5 + PI * 1.25, st);
            line(&[(6.0, 10.0), (10.0, 6.0)]);
        }
        Icon::Scissors => {
            p.circle(at(4.0, 12.0), 2.2 * u, Color32::TRANSPARENT, st);
            p.circle(at(12.0, 12.0), 2.2 * u, Color32::TRANSPARENT, st);
            line(&[(5.5, 10.3), (11.5, 2.0)]);
            line(&[(10.5, 10.3), (4.5, 2.0)]);
        }
        Icon::Check => line(&[(2.5, 8.5), (6.5, 12.5), (13.5, 3.5)]),
        Icon::Close => {
            line(&[(3.5, 3.5), (12.5, 12.5)]);
            line(&[(12.5, 3.5), (3.5, 12.5)]);
        }
        Icon::Plus => {
            line(&[(8.0, 2.5), (8.0, 13.5)]);
            line(&[(2.5, 8.0), (13.5, 8.0)]);
        }
        Icon::Chip => {
            p.rect_stroke(
                Rect::from_min_max(at(4.0, 4.0), at(12.0, 12.0)),
                1.5 * u,
                st,
                eframe::egui::StrokeKind::Middle,
            );
            p.rect_filled(
                Rect::from_min_max(at(6.5, 6.5), at(9.5, 9.5)),
                0.5 * u,
                color,
            );
            for k in [6.0, 8.0, 10.0] {
                line(&[(k, 1.5), (k, 4.0)]);
                line(&[(k, 12.0), (k, 14.5)]);
                line(&[(1.5, k), (4.0, k)]);
                line(&[(12.0, k), (14.5, k)]);
            }
        }
        Icon::Loop => {
            line(&[(2.5, 9.5), (2.5, 5.0), (12.5, 5.0)]);
            line(&[(10.0, 2.5), (12.5, 5.0), (10.0, 7.5)]);
            line(&[(13.5, 6.5), (13.5, 11.0), (3.5, 11.0)]);
            line(&[(6.0, 8.5), (3.5, 11.0), (6.0, 13.5)]);
        }
        Icon::Headphones => {
            arc(p, at(8.0, 9.0), 6.0 * u, PI, TAU, st);
            for x in [2.0, 12.0] {
                p.rect_filled(Rect::from_min_max(at(x, 9.0), at(x + 2.2, 14.0)), u, color);
            }
        }
        Icon::Refresh => {
            arc(p, c, 5.5 * u, -PI * 0.35, PI * 1.45, st);
            let tip = c + vec2((-PI * 0.35).cos(), (-PI * 0.35).sin()) * 5.5 * u;
            p.add(Shape::convex_polygon(
                vec![
                    tip + vec2(-2.8, -1.4) * u,
                    tip + vec2(1.6, -2.6) * u,
                    tip + vec2(1.0, 2.2) * u,
                ],
                color,
                Stroke::NONE,
            ));
        }
        Icon::Sparkle => {
            let star = |cx: f32, cy: f32, r: f32| {
                let o = at(cx, cy);
                let k = r * 0.28 * u;
                let r = r * u;
                for (a, b) in [(vec2(0.0, r), vec2(k, 0.0)), (vec2(r, 0.0), vec2(0.0, k))] {
                    p.add(Shape::convex_polygon(
                        vec![o - a, o + b, o + a, o - b],
                        color,
                        Stroke::NONE,
                    ));
                }
            };
            star(7.0, 9.0, 6.0);
            star(13.0, 3.5, 2.6);
        }
        Icon::Search => {
            p.circle(at(6.8, 6.8), 4.4 * u, Color32::TRANSPARENT, st);
            line(&[(10.1, 10.1), (13.8, 13.8)]);
        }
    }
}
