//! Small vector icons drawn with lines and shapes (no emoji, no icon font needed).

use eframe::egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, StrokeKind, Vec2};
use std::f32::consts::PI;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Icon {
    Host,
    Key,
    Forward,
    Snippet,
    Shield,
    Logs,
    Folder,
    Settings,
    Group,
    User,
    Bell,
    Close,
    Terminal,
    Search,
    ChevLeft,
    ChevRight,
    ChevDown,
    ChevUp,
    FolderFill,
    File,
    Users,
    Upload,
    Copy,
    Stop,
    Power,
    Sliders,
}

impl Icon {
    /// Icons are named in text as `icon:<name>` so they can travel through the same text parameters as letters.
    pub fn parse(s: &str) -> Option<Icon> {
        Some(match s.strip_prefix("icon:")? {
            "host" => Icon::Host,
            "key" => Icon::Key,
            "forward" => Icon::Forward,
            "snippet" => Icon::Snippet,
            "shield" => Icon::Shield,
            "logs" => Icon::Logs,
            "folder" => Icon::Folder,
            "settings" => Icon::Settings,
            "group" => Icon::Group,
            "user" => Icon::User,
            "bell" => Icon::Bell,
            "close" => Icon::Close,
            "terminal" => Icon::Terminal,
            "search" => Icon::Search,
            "chev-left" => Icon::ChevLeft,
            "chev-right" => Icon::ChevRight,
            "chev-down" => Icon::ChevDown,
            "chev-up" => Icon::ChevUp,
            "folder-fill" => Icon::FolderFill,
            "file" => Icon::File,
            "users" => Icon::Users,
            "upload" => Icon::Upload,
            _ => return None,
        })
    }
}

fn arc(cx: f32, cy: f32, r: f32, a0: f32, a1: f32, n: usize) -> Vec<(f32, f32)> {
    (0..=n).map(|i| {
        let t = a0 + (a1 - a0) * i as f32 / n as f32;
        (cx + r * t.cos(), cy + r * t.sin())
    }).collect()
}

/// Draws `icon` centred on `center`, fitting a square of side `size`.
pub fn paint(p: &Painter, center: Pos2, size: f32, icon: Icon, color: Color32) {
    let h = size / 2.0;
    let st = Stroke::new((size / 11.0).max(1.2), color);
    let pt = |x: f32, y: f32| Pos2::new(center.x + x * h, center.y + y * h);
    let line = |a: (f32, f32), b: (f32, f32)| p.line_segment([pt(a.0, a.1), pt(b.0, b.1)], st);
    let poly = |v: &[(f32, f32)], closed: bool| {
        let pts: Vec<Pos2> = v.iter().map(|q| pt(q.0, q.1)).collect();
        p.add(if closed { Shape::closed_line(pts, st) } else { Shape::line(pts, st) });
    };
    match icon {
        Icon::Host => {
            p.rect_stroke(Rect::from_min_max(pt(-1.0, -0.8), pt(1.0, 0.4)), 2.0, st, StrokeKind::Middle);
            line((0.0, 0.4), (0.0, 0.85));
            line((-0.5, 0.85), (0.5, 0.85));
        }
        Icon::Key => {
            p.circle_stroke(pt(-0.45, -0.1), 0.45 * h, st);
            line((0.0, -0.1), (1.0, -0.1));
            line((0.6, -0.1), (0.6, 0.4));
            line((0.95, -0.1), (0.95, 0.3));
        }
        Icon::Forward => {
            line((-0.9, -0.4), (0.9, -0.4));
            poly(&[(0.5, -0.8), (0.9, -0.4), (0.5, 0.0)], false);
            line((0.9, 0.45), (-0.9, 0.45));
            poly(&[(-0.5, 0.05), (-0.9, 0.45), (-0.5, 0.85)], false);
        }
        Icon::Snippet => {
            poly(&[(-0.4, -0.65), (-1.0, 0.0), (-0.4, 0.65)], false);
            poly(&[(0.4, -0.65), (1.0, 0.0), (0.4, 0.65)], false);
            line((0.18, -0.8), (-0.18, 0.8));
        }
        Icon::Shield => {
            poly(&[(0.0, -0.95), (0.8, -0.6), (0.8, 0.1), (0.0, 0.95), (-0.8, 0.1), (-0.8, -0.6)], true);
            poly(&[(-0.35, 0.0), (-0.08, 0.3), (0.4, -0.25)], false);
        }
        Icon::Logs => {
            p.rect_stroke(Rect::from_min_max(pt(-0.65, -0.9), pt(0.65, 0.9)), 2.0, st, StrokeKind::Middle);
            for y in [-0.4, 0.0, 0.4] {
                line((-0.32, y), (0.32, y));
            }
        }
        Icon::Folder => {
            poly(&[(-0.95, -0.65), (-0.3, -0.65), (0.0, -0.35), (0.95, -0.35), (0.95, 0.7), (-0.95, 0.7)], true);
        }
        Icon::Settings => {
            p.circle_stroke(pt(0.0, 0.0), 0.3 * h, st);
            p.circle_stroke(pt(0.0, 0.0), 0.68 * h, st);
            for i in 0..8 {
                let a = i as f32 * PI / 4.0;
                line((0.68 * a.cos(), 0.68 * a.sin()), (0.95 * a.cos(), 0.95 * a.sin()));
            }
        }
        Icon::Group => {
            for (x, y) in [(-0.9, -0.9), (0.1, -0.9), (-0.9, 0.1), (0.1, 0.1)] {
                p.rect_stroke(Rect::from_min_max(pt(x, y), pt(x + 0.8, y + 0.8)), 1.5, st, StrokeKind::Middle);
            }
        }
        Icon::User => {
            p.circle_stroke(pt(0.0, -0.4), 0.38 * h, st);
            poly(&arc(0.0, 1.0, 0.8, PI, 2.0 * PI, 12), false);
        }
        Icon::Bell => {
            let mut v = arc(0.0, -0.1, 0.6, PI, 2.0 * PI, 12);
            v.push((0.6, 0.5));
            v.push((0.9, 0.55));
            v.push((-0.9, 0.55));
            v.push((-0.6, 0.5));
            poly(&v, true);
            p.circle_stroke(pt(0.0, 0.85), 0.14 * h, st);
        }
        Icon::Copy => {
            p.rect_stroke(Rect::from_min_max(pt(-0.35, -0.35), pt(0.85, 0.85)), 2.0, st, StrokeKind::Middle);
            poly(&[(-0.35, 0.35), (-0.85, 0.35), (-0.85, -0.85), (0.35, -0.85), (0.35, -0.35)], false);
        }
        Icon::Stop => {
            p.rect_stroke(Rect::from_min_max(pt(-0.7, -0.7), pt(0.7, 0.7)), 2.0, st, StrokeKind::Middle);
        }
        Icon::Power => {
            poly(&arc(0.0, 0.1, 0.8, -PI * 0.3, PI * 1.3, 18), false);
            line((0.0, -0.95), (0.0, 0.05));
        }
        Icon::Sliders => {
            p.circle_stroke(pt(-0.5, -0.5), 0.3 * h, st);
            line((-0.15, -0.5), (0.9, -0.5));
            p.circle_stroke(pt(0.5, 0.5), 0.3 * h, st);
            line((0.15, 0.5), (-0.9, 0.5));
        }
        Icon::Close => {
            line((-0.7, -0.7), (0.7, 0.7));
            line((0.7, -0.7), (-0.7, 0.7));
        }
        Icon::Terminal => {
            poly(&[(-0.85, -0.5), (-0.2, 0.0), (-0.85, 0.5)], false);
            line((0.0, 0.55), (0.85, 0.55));
        }
        Icon::Search => {
            p.circle_stroke(pt(-0.2, -0.2), 0.6 * h, st);
            line((0.25, 0.25), (0.85, 0.85));
        }
        Icon::ChevLeft => poly(&[(0.35, -0.7), (-0.35, 0.0), (0.35, 0.7)], false),
        Icon::ChevRight => poly(&[(-0.35, -0.7), (0.35, 0.0), (-0.35, 0.7)], false),
        Icon::ChevDown => poly(&[(-0.7, -0.35), (0.0, 0.35), (0.7, -0.35)], false),
        Icon::ChevUp => poly(&[(-0.7, 0.35), (0.0, -0.35), (0.7, 0.35)], false),
        Icon::FolderFill => {
            p.rect_filled(Rect::from_min_max(pt(-0.95, -0.7), pt(-0.1, -0.3)), 1.5, color);
            p.rect_filled(Rect::from_min_max(pt(-0.95, -0.4), pt(0.95, 0.7)), 2.0, color);
        }
        Icon::File => {
            poly(&[(-0.6, -0.9), (0.2, -0.9), (0.6, -0.5), (0.6, 0.9), (-0.6, 0.9)], true);
            poly(&[(0.2, -0.9), (0.2, -0.5), (0.6, -0.5)], false);
            line((-0.3, 0.1), (0.3, 0.1));
            line((-0.3, 0.45), (0.3, 0.45));
        }
        Icon::Upload => {
            line((0.0, -0.85), (0.0, 0.25));
            poly(&[(-0.45, -0.4), (0.0, -0.85), (0.45, -0.4)], false);
            poly(&[(-0.8, 0.2), (-0.8, 0.85), (0.8, 0.85), (0.8, 0.2)], false);
        }
        Icon::Users => {
            p.circle_stroke(pt(-0.35, -0.45), 0.3 * h, st);
            poly(&arc(-0.35, 0.95, 0.65, PI, 2.0 * PI, 12), false);
            p.circle_stroke(pt(0.55, -0.3), 0.24 * h, st);
            poly(&arc(0.55, 0.8, 0.45, PI * 1.05, PI * 1.95, 10), false);
        }
    }
    let _ = Vec2::ZERO;
}
