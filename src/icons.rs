//! Small vector icons: Lucide outlines (see `lucide_data.rs`) stroked with round caps and joins, plus one solid folder.

use eframe::egui::{Color32, Painter, Pos2, Rect, Shape, Stroke};

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
    List,
    FolderPlus,
    Pencil,
    Pin,
    PinOff,
    Trash,
    Download,
    Eye,
    EyeOff,
    Alert,
    Lock,
    Sync,
    Info,
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

/// Draws `icon` centred on `center`, fitting a square of side `size`.
pub fn paint(p: &Painter, center: Pos2, size: f32, icon: Icon, color: Color32) {
    if icon == Icon::FolderFill {
        let h = size / 2.0;
        let pt = |x: f32, y: f32| Pos2::new(center.x + x * h, center.y + y * h);
        p.rect_filled(Rect::from_min_max(pt(-0.95, -0.7), pt(-0.1, -0.3)), 1.5, color);
        p.rect_filled(Rect::from_min_max(pt(-0.95, -0.4), pt(0.95, 0.7)), 2.0, color);
        return;
    }
    // Lucide icons live on a 24 grid with about 2 units of air around the drawing
    let k = size / 22.0;
    let width = (2.0 * k).max(1.3);
    let st = Stroke::new(width, color);
    let at = |q: (f32, f32)| Pos2::new(center.x + (q.0 - 12.0) * k, center.y + (q.1 - 12.0) * k);
    for line in crate::lucide_data::strokes(icon) {
        let pts: Vec<Pos2> = line.iter().map(|q| at(*q)).collect();
        p.add(Shape::line(pts.clone(), st));
        // round caps at both ends, and round joins where the line turns sharply
        let dot = |c: Pos2| p.circle_filled(c, width / 2.0, color);
        dot(pts[0]);
        dot(pts[pts.len() - 1]);
        for w in pts.windows(3) {
            let (a, b) = (w[1] - w[0], w[2] - w[1]);
            let cos = a.normalized().dot(b.normalized());
            if cos < 0.9 {
                dot(w[1]);
            }
        }
    }
}
