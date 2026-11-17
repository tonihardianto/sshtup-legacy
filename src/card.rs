//! The small card that floats at the top right of a terminal. Folded, it shows only the
//! connection dot (and the people, when the terminal is shared); it opens when the pointer is on
//! it, when it is clicked, and by itself when the connection is lost. It replaces a toolbar row,
//! so the terminal gets the whole height.

use crate::collab::{collab_items, CollabAct, Share};
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

pub struct CardIn<'a> {
    pub tab_id: u64,
    pub alive: bool,
    pub bell: bool,
    pub snippets: &'a [Snippet],
    pub share: Option<&'a Share>,
    /// The people in a shared terminal (the host first), for the avatar stack.
    pub people: &'a [(String, Color32)],
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
        .pivot(Align2::RIGHT_TOP)
        .fixed_pos(Pos2::new(area.right() - 14.0, area.top() + 12.0))
        .show(ctx, |ui| {
            Frame::new()
                .fill(Color32::from_rgba_unmultiplied(0x1D, 0x20, 0x33, 238))
                .stroke(Stroke::new(1.0, BORDER))
                .corner_radius(12)
                .shadow(egui::Shadow { offset: [0, 6], blur: 20, spread: 0, color: Color32::from_black_alpha(90) })
                .inner_margin(Margin::symmetric(11, 6))
                .show(ui, |ui| {
                    if open {
                        ui.set_min_width(190.0);
                    }
                    let head = ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        ui.spacing_mut().interact_size.y = 18.0;
                        let (r, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
                        status_dot(ui.painter(), r.center(), if c.alive { GREEN } else { RED });
                        if c.bell {
                            let (br, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
                            crate::icons::paint(ui.painter(), br.center(), 13.0, crate::icons::Icon::Bell, AMBER);
                        }
                        if !c.people.is_empty() {
                            let n = c.people.len();
                            let (r, _) = ui.allocate_exact_size(Vec2::new(18.0 + 14.0 * (n - 1) as f32, 20.0), Sense::hover());
                            for (i, (name, col)) in c.people.iter().enumerate().rev() {
                                avatar(ui.painter(), Pos2::new(r.left() + 9.0 + 14.0 * i as f32, r.center().y), 9.0, name, *col);
                            }
                        }
                    });
                    if ui.interact(head.response.rect, id.with("head"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        *pinned = !*pinned;
                    }
                    if open {
                        ui.add_space(4.0);
                        ui.separator();
                        if !c.alive {
                            if ui.add_sized([ui.available_width(), 30.0], egui::Button::new(RichText::new("Reconnect").color(Color32::WHITE)).fill(BLUE)).clicked() {
                                act.reconnect = true;
                            }
                        } else {
                            ui.menu_button("Snippets", |ui| {
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
                            ui.menu_button(RichText::new(if c.share.is_some() { "Sharing" } else { "Share" }).color(if c.share.is_some() { GREEN } else { TEXT }), |ui| {
                                let a = collab_items(ui, c.share);
                                if a != CollabAct::None {
                                    act.collab = Some(a);
                                }
                            });
                            if ui.button("Disconnect").clicked() {
                                act.disconnect = true;
                            }
                        }
                    }
                });
        });
    ctx.data_mut(|d| d.insert_temp(id, resp.response.rect));
    act
}
