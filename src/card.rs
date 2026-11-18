//! The small card that floats in a corner of a terminal (top right unless Settings says otherwise). Folded, it shows only the
//! connection dot (and the people, when the terminal is shared); it opens when the pointer is on
//! it, when it is clicked, and by itself when the connection is lost. It replaces a toolbar row,
//! so the terminal gets the whole height.

use crate::collab::{CollabAct, Share};
use crate::icons::Icon;
use crate::model::Snippet;
use crate::ui::*;
use eframe::egui::{self, Align2, Color32, Frame, Margin, Pos2, Rect, RichText, Sense, Stroke, Vec2};

/// What the person did in the card this frame.
#[derive(Default)]
pub struct CardAct {
    pub disconnect: bool,
    pub reconnect: bool,
    pub run_snippet: Option<String>,
    pub collab: Option<CollabAct>,
}

/// Which corner of the terminal the card sits in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Corner {
    TopRight,
    TopLeft,
    BottomRight,
    BottomLeft,
}

/// The choices in Settings: the stored value and what the person reads.
pub const POSITIONS: [(&str, &str); 5] = [
    ("top-right", "Top right"),
    ("top-left", "Top left"),
    ("bottom-right", "Bottom right"),
    ("bottom-left", "Bottom left"),
    ("hidden", "Hidden"),
];

impl Corner {
    /// The corner for a stored setting; `None` when the card is hidden. An unknown value counts as the default.
    pub fn from_setting(s: &str) -> Option<Corner> {
        match s {
            "hidden" => None,
            "top-left" => Some(Corner::TopLeft),
            "bottom-right" => Some(Corner::BottomRight),
            "bottom-left" => Some(Corner::BottomLeft),
            _ => Some(Corner::TopRight),
        }
    }
}

pub struct CardIn<'a> {
    pub corner: Corner,
    pub tab_id: u64,
    pub alive: bool,
    pub bell: bool,
    pub snippets: &'a [Snippet],
    pub share: Option<&'a Share>,
    /// The people in a shared terminal (the host first), for the avatar stack.
    pub people: &'a [(String, Color32)],
    /// False on a guest's tab: someone else's session is not ours to share.
    pub can_share: bool,
}

const ROW_H: f32 = 34.0;

/// A full-width row: an icon and a label, lit when the pointer is on it.
fn row(ui: &mut egui::Ui, icon: Icon, label: &str, color: Color32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(rect, 8.0, wash(14));
    }
    crate::icons::paint(ui.painter(), Pos2::new(rect.left() + 17.0, rect.center().y), 15.0, icon, MUTED());
    ui.painter().text(Pos2::new(rect.left() + 38.0, rect.center().y), Align2::LEFT_CENTER, label, egui::FontId::proportional(14.0), color);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// "Code  k7m2-9xqa" with a copy icon; a click copies the code.
fn code_row(ui: &mut egui::Ui, code: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(rect, 8.0, wash(14));
    }
    let y = rect.center().y;
    ui.painter().text(Pos2::new(rect.left() + 12.0, y), Align2::LEFT_CENTER, "Code", egui::FontId::proportional(14.0), MUTED());
    // the real code is long: keep both ends and drop the middle so it fits between the label and the copy icon
    let font = egui::FontId::monospace(14.0);
    let char_w = ui.painter().layout_no_wrap("0".into(), font.clone(), TEXT()).size().x.max(1.0);
    let room = ((rect.right() - 38.0 - (rect.left() + 58.0)) / char_w).floor() as usize;
    let chars: Vec<char> = code.chars().collect();
    let shown = if chars.len() <= room || room < 8 {
        code.to_string()
    } else {
        let tail = (room - 1) / 3;
        let head = room - 1 - tail;
        format!("{}…{}", chars[..head].iter().collect::<String>(), chars[chars.len() - tail..].iter().collect::<String>())
    };
    ui.painter().text(Pos2::new(rect.left() + 58.0, y), Align2::LEFT_CENTER, shown, font, TEXT());
    crate::icons::paint(ui.painter(), Pos2::new(rect.right() - 16.0, y), 15.0, Icon::Copy, MUTED());
    resp.on_hover_text(format!("Copy the session code\n{code}")).on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A guest: avatar, name and a switch for whether they may type. True when the switch was moved.
fn guest_row(ui: &mut egui::Ui, g: &crate::collab_net::GuestInfo, on: &mut bool) -> bool {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H), Sense::hover());
    let y = rect.center().y;
    let c = crate::collab_net::guest_color(g.gid);
    avatar(ui.painter(), Pos2::new(rect.left() + 17.0, y), 10.0, &g.name, Color32::from_rgb(c[0], c[1], c[2]));
    ui.painter().text(Pos2::new(rect.left() + 38.0, y), Align2::LEFT_CENTER, &g.name, egui::FontId::proportional(14.0), TEXT());
    let slot = Rect::from_min_max(Pos2::new(rect.right() - 48.0, rect.top()), Pos2::new(rect.right() - 6.0, rect.bottom()));
    let mut changed = false;
    ui.scope_builder(egui::UiBuilder::new().max_rect(slot).layout(egui::Layout::right_to_left(egui::Align::Center)), |ui| {
        changed = toggle(ui, on).on_hover_text(if *on { "Can type" } else { "View only" }).changed();
    });
    changed
}

/// Draws the card in the top right corner of `area`. `pinned` is whether a click has opened it.
pub fn session_card(ctx: &egui::Context, area: Rect, pinned: &mut bool, c: &CardIn) -> CardAct {
    let mut act = CardAct::default();
    let id = egui::Id::new(("session-card", c.tab_id));
    // was the pointer on the card in the last frame? (the card's size depends on the answer, so use the old rect)
    let last: Option<Rect> = ctx.data(|d| d.get_temp(id));
    let hover = last.zip(ctx.pointer_hover_pos()).map(|(r, p)| r.expand(4.0).contains(p)).unwrap_or(false);
    let open = *pinned || hover || !c.alive || egui::Popup::is_any_open(ctx);

    let resp = egui::Area::new(id)
        .order(egui::Order::Foreground)
        .pivot(match c.corner {
            Corner::TopRight => Align2::RIGHT_TOP,
            Corner::TopLeft => Align2::LEFT_TOP,
            Corner::BottomRight => Align2::RIGHT_BOTTOM,
            Corner::BottomLeft => Align2::LEFT_BOTTOM,
        })
        .fixed_pos(match c.corner {
            Corner::TopRight => Pos2::new(area.right() - 14.0, area.top() + 12.0),
            Corner::TopLeft => Pos2::new(area.left() + 14.0, area.top() + 12.0),
            Corner::BottomRight => Pos2::new(area.right() - 14.0, area.bottom() - 12.0),
            Corner::BottomLeft => Pos2::new(area.left() + 14.0, area.bottom() - 12.0),
        })
        .show(ctx, |ui| {
            let round = !(open || c.bell || !c.people.is_empty());
            Frame::new()
                .fill(CARD().gamma_multiply(0.93))
                .stroke(Stroke::new(1.0, BORDER()))
                .corner_radius(if round { 16 } else { 12 })
                .shadow(egui::Shadow { offset: [0, 6], blur: 20, spread: 0, color: Color32::from_black_alpha(90) })
                .inner_margin(if round { Margin::same(7) } else { Margin::symmetric(11, 6) })
                .show(ui, |ui| {
                    if open {
                        ui.set_min_width(230.0);
                    }
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.spacing_mut().interact_size.y = 18.0;
                    let head = ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = if round { 0.0 } else { 6.0 };
                        ui.spacing_mut().interact_size.y = 18.0;
                        let (r, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
                        crate::icons::paint(ui.painter(), r.center(), 16.0, Icon::Sliders, if c.alive { GREEN() } else { RED() });
                        if c.bell {
                            let (br, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
                            crate::icons::paint(ui.painter(), br.center(), 13.0, crate::icons::Icon::Bell, AMBER());
                        }
                        if !c.people.is_empty() {
                            let n = c.people.len();
                            let (r, _) = ui.allocate_exact_size(Vec2::new(18.0 + 14.0 * (n - 1) as f32, 20.0), Sense::hover());
                            for (i, (name, col)) in c.people.iter().enumerate().rev() {
                                avatar(ui.painter(), Pos2::new(r.left() + 9.0 + 14.0 * i as f32, r.center().y), 9.0, name, *col);
                            }
                        }
                        if open {
                            let (text, col) = match (c.alive, c.share.is_some()) {
                                (false, _) => ("Disconnected", RED()),
                                (true, true) => ("Sharing", GREEN()),
                                (true, false) => ("Connected", GREEN()),
                            };
                            ui.label(RichText::new(text).size(13.0).color(col));
                        }
                    });
                    if ui.interact(head.response.rect, id.with("head"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        *pinned = !*pinned;
                    }
                    if open {
                        ui.separator();
                        if !c.alive {
                            if ui.add_sized([ui.available_width(), 30.0], egui::Button::new(RichText::new("Reconnect").color(Color32::WHITE)).fill(BLUE())).clicked() {
                                act.reconnect = true;
                            }
                        } else {
                            // one flat card: no menus inside menus (the snippet list is the only pop-up)
                            let r = row(ui, Icon::Snippet, "Snippets", TEXT());
                            egui::Popup::menu(&r).show(|ui| {
                                ui.set_min_width(180.0);
                                if c.snippets.is_empty() {
                                    label_muted(ui, "No snippets yet");
                                }
                                for sn in c.snippets {
                                    if ui.button(sn.display_label()).clicked() {
                                        act.run_snippet = Some(sn.script.clone());
                                        ui.close();
                                    }
                                }
                            });
                            match c.share {
                                None if !c.can_share => {}
                                None => {
                                    if row(ui, Icon::Users, "Share this terminal", TEXT()).clicked() {
                                        act.collab = Some(CollabAct::Start);
                                    }
                                }
                                Some(s) => {
                                    if code_row(ui, &s.code()).clicked() {
                                        act.collab = Some(CollabAct::CopyCode);
                                    }
                                    ui.separator();
                                    for g in s.guests() {
                                        let mut on = g.can_type;
                                        if guest_row(ui, &g, &mut on) {
                                            act.collab = Some(CollabAct::GuestType(g.gid, on));
                                        }
                                    }
                                    if row(ui, Icon::Stop, "Stop sharing", TEXT()).clicked() {
                                        act.collab = Some(CollabAct::Stop);
                                    }
                                }
                            }
                            if row(ui, Icon::Power, "Disconnect", TEXT()).clicked() {
                                act.disconnect = true;
                            }
                        }
                    }
                });
        });
    ctx.data_mut(|d| d.insert_temp(id, resp.response.rect));
    act
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_choice_in_settings_maps_to_a_corner_or_hidden() {
        assert_eq!(Corner::from_setting("top-right"), Some(Corner::TopRight));
        assert_eq!(Corner::from_setting("top-left"), Some(Corner::TopLeft));
        assert_eq!(Corner::from_setting("bottom-right"), Some(Corner::BottomRight));
        assert_eq!(Corner::from_setting("bottom-left"), Some(Corner::BottomLeft));
        assert_eq!(Corner::from_setting("hidden"), None);
        for (k, _) in POSITIONS {
            assert!(k == "hidden" || Corner::from_setting(k).is_some());
        }
    }

    #[test]
    fn an_unknown_value_falls_back_to_the_default_corner() {
        assert_eq!(Corner::from_setting(""), Some(Corner::TopRight));
        assert_eq!(Corner::from_setting("middle"), Some(Corner::TopRight));
    }

    #[test]
    fn settings_saved_before_this_option_still_load() {
        let s: crate::model::Settings = serde_json::from_str(r#"{"font_size": 14.0}"#).unwrap();
        assert_eq!(s.card_position, "top-right");
    }
}
