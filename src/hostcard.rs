//! The host card on the Hosts screen: an avatar (with a green dot while connected), the name, `user@address:port`, chips for team and tags, and
//! actions that show while the pointer or the keyboard focus is on the card.

use crate::icons::{self, Icon};
use crate::model::{Host, Protocol};
use crate::ui::*;
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};

/// What the user asked for on a card.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CardAct {
    None,
    Open,
    Sftp,
    Edit,
    Duplicate,
    Pin,
    Delete,
    /// the n-th entry of `HostCardIn::moves`
    Move(usize),
    /// into a group (None: the top level)
    MoveGroup(Option<u64>),
}

/// The text inside the avatar: the last number of an IPv4 address (`.3`), otherwise the first two letters.
pub fn avatar_text(label: &str, address: &str) -> String {
    let parts: Vec<&str> = address.trim().split('.').collect();
    if parts.len() == 4 && parts.iter().all(|p| p.parse::<u8>().is_ok()) {
        return format!(".{}", parts[3]);
    }
    let name = if label.trim().is_empty() { address.split('.').next().unwrap_or(address) } else { label };
    let words: Vec<&str> = name.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let initials: String = if words.len() >= 2 {
        words.iter().take(2).filter_map(|w| w.chars().next()).collect()
    } else {
        words.first().map(|w| w.chars().take(2).collect()).unwrap_or_default()
    };
    initials.to_uppercase()
}

/// `user@address:port` shortened to at most `max` characters. The user name gives way first (`rslestari…@10.20.30.3:22`),
/// because the address is what tells hosts apart; only a very narrow card cuts the address too.
pub fn elide_sub(user: &str, addr_port: &str, max: usize) -> String {
    let full = if user.is_empty() { addr_port.to_string() } else { format!("{user}@{addr_port}") };
    if full.chars().count() <= max {
        return full;
    }
    let tail = addr_port.chars().count() + if user.is_empty() { 0 } else { 2 }; // "…@" is 2 characters
    if !user.is_empty() && max >= tail + 1 {
        let keep = max - tail;
        let head: String = user.chars().take(keep).collect();
        return format!("{head}…@{addr_port}");
    }
    let skip = full.chars().count() + 1 - max.max(2);
    format!("…{}", full.chars().skip(skip).collect::<String>())
}

pub struct HostCardIn<'a> {
    pub host: &'a Host,
    pub user: String,
    pub color: Color32,
    pub live: bool,
    /// "Team X" when the host belongs to a team.
    pub team: String,
    /// "Move to ..." labels for the menu
    pub moves: Vec<String>,
    /// other groups the host can go to: (id, name); the host's current group is left out
    pub groups: Vec<(u64, String)>,
    pub in_group: bool,
}

impl HostCardIn<'_> {
    fn chips(&self) -> Vec<(String, bool)> {
        let mut v: Vec<(String, bool)> = vec![];
        // if !self.team.is_empty() {
        //     v.push((self.team.clone(), false));
        // }
        let room = 3usize.saturating_sub(v.len());
        for t in self.host.tags.iter().take(room) {
            v.push((t.clone(), false));
        }
        if self.host.tags.len() > room {
            v.push((format!("+{}", self.host.tags.len() - room), false));
        }
        v
    }

    /// Cards with nothing to show in the chip row are shorter.
    pub fn height(&self) -> f32 {
        if self.chips().is_empty() { 66.0 } else { 98.0 }
    }
}

fn icon_button(ui: &mut egui::Ui, rect: Rect, id: egui::Id, draw: impl FnOnce(&egui::Painter, Pos2, Color32)) -> egui::Response {
    let r = ui.interact(rect, id, Sense::click());
    if r.hovered() {
        ui.painter().rect_filled(rect, 6.0, BORDER());
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    draw(ui.painter(), rect.center(), if r.hovered() { TEXT() } else { MUTED() });
    r
}

fn menu_items(ui: &mut egui::Ui, h: &Host, moves: &[String], groups: &[(u64, String)], in_group: bool, act: &mut CardAct) {
    ui.set_min_width(150.0);
    ui.spacing_mut().button_padding = Vec2::new(12.0, 7.0);
    ui.spacing_mut().item_spacing.y = 2.0;
    let (pin, pin_icon) = if h.pinned { ("Unpin", Icon::PinOff) } else { ("Pin to top", Icon::Pin) };
    let mut items: Vec<(Icon, &str, CardAct)> = vec![(Icon::Terminal, "Connect", CardAct::Open)];
    if h.protocol == Protocol::Ssh {
        items.push((Icon::Folder, "Open SFTP", CardAct::Sftp));
    }
    items.extend([(Icon::Pencil, "Edit", CardAct::Edit), (Icon::Copy, "Duplicate", CardAct::Duplicate), (pin_icon, pin, CardAct::Pin), (Icon::Trash, "Delete", CardAct::Delete)]);
    for (icon, text, a) in items {
        if a == CardAct::Delete {
            icon_menu_button(ui, Icon::Group, "Move to group", |ui| {
                if in_group && submenu_item(ui, "No group").clicked() {
                    *act = CardAct::MoveGroup(None);
                    ui.close();
                }
                if groups.is_empty() && !in_group {
                    ui.label(egui::RichText::new("No groups yet").size(12.0).color(MUTED()));
                }
                for (gid, name) in groups {
                    if submenu_item(ui, name).clicked() {
                        *act = CardAct::MoveGroup(Some(*gid));
                        ui.close();
                    }
                }
            });
        }
        if a == CardAct::Delete && !moves.is_empty() {
            // one entry with a submenu, however many teams there are
            icon_menu_button(ui, Icon::Users, "Move to team", |ui| {
                for (i, m) in moves.iter().enumerate() {
                    if submenu_item(ui, crate::sync::ui::target_name(m)).clicked() {
                        *act = CardAct::Move(i);
                        ui.close();
                    }
                }
            });
        }
        let clicked = if a == CardAct::Delete {
            // a soft red field marks the one item that destroys something
            let r = ui.add(egui::Button::new(egui::RichText::new(format!("       {text}")).size(12.5).color(RED())).fill(RED().linear_multiply(0.14)).corner_radius(6).min_size(Vec2::new(0.0, 32.0)));
            icons::paint(ui.painter(), Pos2::new(r.rect.left() + 20.0, r.rect.center().y), 14.0, icon, RED());
            r.clicked()
        } else {
            icon_text_button(ui, icon, text).clicked()
        };
        if clicked {
            *act = a;
            ui.close();
        }
    }
}

pub fn host_card(ui: &mut egui::Ui, width: f32, c: &HostCardIn) -> CardAct {
    let h = c.host;
    let size = Vec2::new(width, c.height());
    let out = card(ui, size, false);
    let rect = out.rect;
    let p = ui.painter().clone();
    let mut act = CardAct::None;
    if out.resp.clicked() || (out.resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))) {
        act = CardAct::Open;
    }
    if c.live {
        p.rect_stroke(rect, 10.0, Stroke::new(1.0, GREEN().linear_multiply(0.55)), StrokeKind::Inside);
    }
    if out.resp.has_focus() {
        p.rect_stroke(rect, 10.0, Stroke::new(1.5, BLUE()), StrokeKind::Inside);
    }

    out.resp.context_menu(|ui| menu_items(ui, h, &c.moves, &c.groups, c.in_group, &mut act));

    // avatar
    let av = Rect::from_min_size(rect.min + Vec2::new(12.0, 12.0), Vec2::splat(40.0));
    p.rect_filled(av, 8.0, c.color.linear_multiply(0.9));
    p.text(av.center(), Align2::CENTER_CENTER, avatar_text(&h.label, &h.address), FontId::proportional(15.0), Color32::WHITE);
    if c.live {
        p.circle_filled(Pos2::new(av.right() - 1.0, av.bottom() - 1.0), 6.5, CARD());
        status_dot(&p, Pos2::new(av.right() - 1.0, av.bottom() - 1.0), GREEN());
    }

    // actions, only while the card is being pointed at or focused (or its menu is open)
    let menu_key = out.resp.id.with("menu-open");
    let menu_was_open = ui.ctx().data(|d| d.get_temp::<bool>(menu_key)).unwrap_or(false);
    let show = out.resp.contains_pointer() || out.resp.has_focus() || menu_was_open;
    let mut right_edge = rect.right() - 10.0;
    if show {
        let b = |x_right: f32| Rect::from_center_size(Pos2::new(x_right - 14.0, rect.top() + 24.0), Vec2::splat(28.0));
        let more_rect = b(right_edge);
        let more = icon_button(ui, more_rect, out.resp.id.with("more"), |pt, c, col| {
            for dx in [-5.0, 0.0, 5.0] {
                pt.circle_filled(Pos2::new(c.x + dx, c.y), 1.6, col);
            }
        });
        right_edge = more_rect.left() - 2.0;
        let mut open = false;
        egui::Popup::menu(&more).gap(6.0).show(|ui| {
            open = true;
            menu_items(ui, h, &c.moves, &c.groups, c.in_group, &mut act);
        });
        ui.ctx().data_mut(|d| d.insert_temp(menu_key, open));
        if h.protocol == Protocol::Ssh {
            let sftp_rect = b(right_edge);
            let s = icon_button(ui, sftp_rect, out.resp.id.with("sftp"), |pt, c, col| icons::paint(pt, c, 15.0, Icon::Folder, col));
            s.clone().on_hover_text("Open SFTP");
            if s.clicked() {
                act = CardAct::Sftp;
            }
            right_edge = sftp_rect.left() - 2.0;
        }
    } else {
        ui.ctx().data_mut(|d| d.insert_temp(menu_key, false));
    }

    // text
    let tx = av.right() + 12.0;
    let title_w = (right_edge - tx - 4.0).max(20.0);
    let tp = p.with_clip_rect(Rect::from_min_max(Pos2::new(tx, rect.top()), Pos2::new(tx + title_w, rect.bottom())));
    tp.text(Pos2::new(tx, rect.top() + 22.0), Align2::LEFT_CENTER, h.display_label(), FontId::proportional(14.5), TEXT());
    let mono = FontId::monospace(11.5);
    let char_w = p.layout_no_wrap("0".into(), mono.clone(), MUTED()).size().x.max(1.0);
    let sub_w = rect.right() - 12.0 - tx;
    let addr_port = format!("{}:{}", h.address, h.effective_port());
    let sub = elide_sub(&c.user, &addr_port, (sub_w / char_w).floor() as usize);
    p.text(Pos2::new(tx, rect.top() + 42.0), Align2::LEFT_CENTER, &sub, mono, MUTED());
    if sub.contains('…') {
        let full = if c.user.is_empty() { addr_port.clone() } else { format!("{}@{}", c.user, addr_port) };
        ui.interact(Rect::from_min_size(Pos2::new(tx, rect.top() + 34.0), Vec2::new(sub_w, 16.0)), out.resp.id.with("sub"), Sense::hover()).on_hover_text(full);
    }

    // chips
    let mut x = rect.left() + 12.0;
    let y = rect.top() + 12.0 + 40.0 + 10.0 + 11.0;
    for (text, good) in c.chips() {
        let g = p.layout_no_wrap(text, FontId::proportional(11.5), if good { GREEN() } else { MUTED() });
        let w = g.size().x + 16.0;
        if x + w > rect.right() - 12.0 {
            break;
        }
        let r = Rect::from_min_size(Pos2::new(x, y - 11.0), Vec2::new(w, 22.0));
        p.rect_filled(r, 11.0, if good { GREEN().linear_multiply(0.18) } else { wash(16) });
        p.galley(Pos2::new(r.left() + 8.0, r.center().y - g.size().y / 2.0), g, TEXT());
        x += w + 6.0;
    }
    act
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ip_address_shows_its_last_number() {
        assert_eq!(avatar_text("LESTARI RAHARJA", "10.20.30.3"), ".3");
        assert_eq!(avatar_text("", "192.168.1.142"), ".142");
    }

    #[test]
    fn a_name_shows_its_first_letters() {
        assert_eq!(avatar_text("private", "host.example.com"), "PR");
        assert_eq!(avatar_text("web server", "x.example.com"), "WS");
        assert_eq!(avatar_text("", "db.example.com"), "DB");
        assert_eq!(avatar_text("1.2.3", "x.example.com"), "12");
    }

    #[test]
    fn a_long_user_name_gives_way_before_the_address() {
        assert_eq!(elide_sub("rslestariraharja", "10.20.30.3:22", 40), "rslestariraharja@10.20.30.3:22");
        assert_eq!(elide_sub("rslestariraharja", "10.20.30.3:22", 20), "rsles…@10.20.30.3:22");
        assert_eq!(elide_sub("", "10.20.30.3:22", 40), "10.20.30.3:22");
    }

    #[test]
    fn a_very_narrow_card_cuts_the_front_of_the_address() {
        let s = elide_sub("me", "very.long.example.com:22", 10);
        assert_eq!(s.chars().count(), 10);
        assert!(s.starts_with('…') && s.ends_with(":22"));
    }
}
