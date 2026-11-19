//! The first screen a new user sees: their name, three ways to start, and an optional master password.

use crate::app::App;
use crate::icons::Icon;
use crate::model::Host;
use crate::ui::*;
use eframe::egui::{self, Align2, Color32, FontId, Margin, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Vec2};

/// What is typed in the welcome screen.
pub struct WelcomeUi {
    name: String,
    named: bool,
    /// The "Protect your vault" card is open.
    open: bool,
    pw: String,
    confirm: String,
    show_pw: bool,
    show_confirm: bool,
    keep: bool,
    understood: bool,
    done: Option<String>,
    /// Whether ~/.ssh/config exists, looked up once.
    ssh_config: Option<bool>,
}

impl Default for WelcomeUi {
    fn default() -> Self {
        Self {
            name: String::new(),
            named: false,
            open: false,
            pw: String::new(),
            confirm: String::new(),
            show_pw: false,
            show_confirm: false,
            keep: false,
            understood: false,
            done: None,
            ssh_config: None,
        }
    }
}

enum Go {
    AddHost,
    Import,
    Local,
    Team,
    Skip,
    Encrypt,
    Name,
}

#[allow(non_snake_case)]
fn PRIMARY_FILL() -> Color32 {
    pick(0x1A2040, 0xE6ECFB)
}
#[allow(non_snake_case)]
fn LINK() -> Color32 {
    pick(0x9DA8FF, 0x4756D6)
}
#[allow(non_snake_case)]
fn PILL() -> Color32 {
    pick(0x2A2D42, 0xE3E6EE)
}

/// The key that goes with the shortcut chips: Cmd on a Mac, Ctrl elsewhere.
fn cmd_label() -> &'static str {
    if cfg!(target_os = "macos") {
        "⌘"
    } else {
        "Ctrl+"
    }
}

struct StartRow<'a> {
    icon: Icon,
    title: &'a str,
    /// Set in monospace right after the title.
    code: Option<&'a str>,
    badge: Option<&'a str>,
    text: &'a str,
    shortcut: &'a str,
    primary: bool,
}

fn start_row(ui: &mut egui::Ui, row: StartRow) -> bool {
    let (r, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 66.0), Sense::click());
    let p = ui.painter();
    let (fill, stroke) = if row.primary {
        (if resp.hovered() { PRIMARY_FILL().linear_multiply(1.25) } else { PRIMARY_FILL() }, Stroke::new(1.5, BLUE().linear_multiply(0.75)))
    } else {
        (if resp.hovered() { CARD_HOVER() } else { CARD() }, Stroke::new(1.0, BORDER()))
    };
    p.rect(r, 10, fill, stroke, StrokeKind::Inside);
    let ib = Rect::from_center_size(Pos2::new(r.left() + 36.0, r.center().y), Vec2::splat(38.0));
    if row.primary {
        p.rect_filled(ib, 9, BLUE());
        crate::icons::paint(p, ib.center(), 19.0, row.icon, Color32::WHITE);
    } else {
        p.rect_filled(ib, 9, BLUE().linear_multiply(0.18));
        crate::icons::paint(p, ib.center(), 19.0, row.icon, BLUE());
    }
    let x = r.left() + 68.0;
    let g = p.layout_no_wrap(row.title.to_string(), FontId::proportional(14.5), TEXT());
    let mut right = x + g.size().x;
    p.galley(Pos2::new(x, r.center().y - 21.0), g, TEXT());
    if let Some(code) = row.code {
        let g = p.layout_no_wrap(code.to_string(), FontId::monospace(13.0), TEXT());
        let w = g.size().x;
        p.galley(Pos2::new(right + 6.0, r.center().y - 20.0), g, TEXT());
        right += 6.0 + w;
    }
    if let Some(b) = row.badge {
        let g = p.layout_no_wrap(b.to_string(), FontId::proportional(11.5), GREEN());
        let pill = Rect::from_min_size(Pos2::new(right + 10.0, r.center().y - 21.0), Vec2::new(g.size().x + 26.0, 19.0));
        p.rect_filled(pill, 9.5, GREEN().linear_multiply(0.16));
        p.circle_filled(Pos2::new(pill.left() + 11.0, pill.center().y), 3.0, GREEN());
        p.galley(Pos2::new(pill.left() + 19.0, pill.center().y - g.size().y / 2.0), g, GREEN());
    }
    p.text(Pos2::new(x, r.center().y + 10.0), Align2::LEFT_CENTER, row.text, FontId::proportional(12.5), MUTED());
    crate::icons::paint(p, Pos2::new(r.right() - 22.0, r.center().y), 14.0, Icon::ChevRight, MUTED());
    // the shortcut chip
    let g = p.layout_no_wrap(format!("{}{}", cmd_label(), row.shortcut), FontId::monospace(11.5), MUTED());
    let chip = Rect::from_center_size(Pos2::new(r.right() - 48.0 - (g.size().x + 16.0) / 2.0, r.center().y), Vec2::new(g.size().x + 16.0, 24.0));
    p.rect(chip, 6, Color32::TRANSPARENT, Stroke::new(1.0, BORDER()), StrokeKind::Inside);
    p.galley(Pos2::new(chip.center().x - g.size().x / 2.0, chip.center().y - g.size().y / 2.0), g, MUTED());
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp.clicked()
}

/// A small rounded tag, like "Optional".
fn pill(ui: &mut egui::Ui, text: &str) {
    let g = ui.painter().layout_no_wrap(text.to_string(), FontId::proportional(11.0), MUTED());
    let (r, _) = ui.allocate_exact_size(Vec2::new(g.size().x + 16.0, 18.0), Sense::hover());
    ui.painter().rect_filled(r, 9, PILL());
    ui.painter().galley(Pos2::new(r.center().x - g.size().x / 2.0, r.center().y - g.size().y / 2.0), g, MUTED());
}

/// A password box with the show / hide eye inside its right edge.
fn password_box(ui: &mut egui::Ui, text: &mut String, show: &mut bool) {
    let w = ui.available_width();
    let r = ui.add(egui::TextEdit::singleline(text).password(!*show).desired_width(w).margin(Margin { left: 10, right: 40, top: 8, bottom: 8 }));
    let eye = Rect::from_center_size(Pos2::new(r.rect.right() - 22.0, r.rect.center().y), Vec2::splat(26.0));
    let resp = ui.interact(eye, r.id.with("eye"), Sense::click());
    let color = if resp.hovered() { TEXT() } else { MUTED() };
    crate::icons::paint(ui.painter(), eye.center(), 16.0, if *show { Icon::EyeOff } else { Icon::Eye }, color);
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    if resp.clicked() {
        *show = !*show;
    }
}

/// How good a password is, 0 to 4 (0 = shorter than the 8 characters it needs).
fn strength(pw: &str) -> usize {
    let n = pw.chars().count();
    if n < 8 {
        return 0;
    }
    let classes = [pw.chars().any(|c| c.is_lowercase()), pw.chars().any(|c| c.is_uppercase()), pw.chars().any(|c| c.is_ascii_digit()), pw.chars().any(|c| !c.is_alphanumeric())];
    let kinds = classes.iter().filter(|b| **b).count();
    let mut s = 1;
    if n >= 12 || kinds >= 3 {
        s += 1;
    }
    if n >= 12 && kinds >= 3 {
        s += 1;
    }
    if n >= 16 && kinds >= 3 {
        s += 1;
    }
    s.min(4)
}

fn field_label(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).size(12.5).strong().color(TEXT()));
}

/// A checkbox with a line of small print under its label.
fn check_row(ui: &mut egui::Ui, on: &mut bool, title: &str, note: Option<&str>) {
    ui.horizontal_top(|ui| {
        ui.spacing_mut().interact_size.y = 18.0;
        ui.checkbox(on, "");
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.label(RichText::new(title).size(12.5).color(TEXT()));
            if let Some(n) = note {
                ui.label(RichText::new(n).size(11.5).color(MUTED()));
            }
        });
    });
}

impl App {
    /// A new user (empty vault, never got past this screen) sees a welcome instead of an empty Hosts page.
    pub fn needs_welcome(&self) -> bool {
        if self.demo {
            return std::env::var("SSHTUP_DEMO_WELCOME").is_ok();
        }
        !self.vault.settings.onboarded && self.vault.hosts.is_empty() && self.active.is_none()
    }

    pub fn welcome_screen(&mut self, ui: &mut egui::Ui) {
        let mut go: Option<Go> = None;
        // for the docs screenshots: SSHTUP_DEMO_WELCOME=2 shows the step after the name, =3 with the vault card open
        if self.demo && !self.wel.named {
            let step = std::env::var("SSHTUP_DEMO_WELCOME").unwrap_or_default();
            if step == "2" || step == "3" {
                self.wel.named = true;
                self.wel.open = step == "3";
                self.vault.settings.user_name = "Toni".into();
            }
        }
        if self.wel.ssh_config.is_none() {
            self.wel.ssh_config = Some(std::path::Path::new(&crate::app::expand_home("~/.ssh/config")).exists());
        }
        let ssh_found = self.wel.ssh_config == Some(true);
        if self.wel.named {
            let (add, import, local) = ui.input_mut(|i| {
                use egui::{Key, KeyboardShortcut as K, Modifiers as M};
                (i.consume_shortcut(&K::new(M::COMMAND, Key::N)), i.consume_shortcut(&K::new(M::COMMAND, Key::I)), i.consume_shortcut(&K::new(M::COMMAND, Key::T)))
            });
            if add {
                go = Some(Go::AddHost);
            } else if import {
                go = Some(Go::Import);
            } else if local {
                go = Some(Go::Local);
            }
        }
        egui::CentralPanel::default().frame(egui::Frame::new().fill(BG_CONTENT())).show_inside(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space((ui.available_height() * 0.06).clamp(20.0, 70.0));
                    ui.allocate_ui(Vec2::new(560.0, 0.0), |ui| {
                        ui.set_width(560.0);
                        ui.spacing_mut().item_spacing.y = 10.0;
                        ui.vertical_centered(|ui| {
                            let title = if self.wel.named { format!("Welcome, {}", self.vault.settings.user_name) } else { "Welcome to SSHTUP".to_string() };
                            ui.label(RichText::new(title).size(28.0).strong().color(TEXT()));
                            ui.label(RichText::new("Connect to your servers, move files and share terminals, all in one place.").size(13.5).color(MUTED()));
                        });
                        ui.add_space(14.0);
                        if !self.wel.named {
                            ui.add_space(10.0);
                            ui.label(RichText::new("What should we call you?").size(14.0).strong().color(TEXT()));
                            let r = ui.add(egui::TextEdit::singleline(&mut self.wel.name).hint_text("Your name").desired_width(320.0).horizontal_align(egui::Align::Center).margin(Vec2::new(10.0, 8.0)));
                            if !r.has_focus() && self.wel.name.is_empty() {
                                r.request_focus();
                            }
                            let ok = !self.wel.name.trim().is_empty();
                            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                            ui.add_space(2.0);
                            ui.add_enabled_ui(ok, |ui| {
                                if primary_button(ui, "Continue").clicked() || (ok && enter) {
                                    go = Some(Go::Name);
                                }
                            });
                            return;
                        }
                        if start_row(ui, StartRow { icon: Icon::Host, title: "Add a host", code: None, badge: None, text: "Save a server to connect to with one click", shortcut: "N", primary: true }) {
                            go = Some(Go::AddHost);
                        }
                        if start_row(ui, StartRow { icon: Icon::Download, title: "Import from", code: Some("~/.ssh/config"), badge: ssh_found.then_some("File found"), text: "Bring in the hosts and keys you already use", shortcut: "I", primary: false }) {
                            go = Some(Go::Import);
                        }
                        if start_row(ui, StartRow { icon: Icon::Terminal, title: "Open a local terminal", code: None, badge: None, text: "A shell on this computer, to try things out", shortcut: "T", primary: false }) {
                            go = Some(Go::Local);
                        }
                        ui.vertical_centered(|ui| {
                            ui.horizontal(|ui| {
                                let a = ui.painter().layout_no_wrap("Working with a team? ".to_string(), FontId::proportional(12.5), MUTED());
                                let b = ui.painter().layout_no_wrap("Share a terminal with a teammate →".to_string(), FontId::proportional(12.5), LINK());
                                ui.add_space(((ui.available_width() - a.size().x - b.size().x) / 2.0).max(0.0));
                                ui.spacing_mut().item_spacing.x = 0.0;
                                ui.label(RichText::new("Working with a team? ").size(12.5).color(MUTED()));
                                if ui.add(egui::Label::new(RichText::new("Share a terminal with a teammate →").size(12.5).strong().color(LINK())).sense(Sense::click())).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                                    go = Some(Go::Team);
                                }
                            });
                        });
                        ui.add_space(4.0);
                        self.vault_card(ui, &mut go);
                        ui.add_space(6.0);
                        ui.vertical_centered(|ui| {
                            if ui.add(egui::Label::new(RichText::new("Skip setup and go to Hosts →").size(12.5).color(MUTED())).sense(Sense::click())).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                                go = Some(Go::Skip);
                            }
                        });
                    });
                });
            });
        });
        match go {
            Some(Go::Name) => {
                self.vault.settings.user_name = self.wel.name.trim().to_string();
                self.dirty = true;
                self.wel.named = true;
            }
            Some(Go::Encrypt) => {
                self.sec.new_pw = std::mem::take(&mut self.wel.pw);
                self.sec.confirm = std::mem::take(&mut self.wel.confirm);
                self.sec.message = None;
                self.set_master_password(false);
                if let Some((_, true)) = self.sec.message.clone() {
                    let mut msg = "The vault is encrypted.".to_string();
                    if self.wel.keep {
                        if let Some(k) = &self.vault_key {
                            if crate::keystore::save(&k.raw()) {
                                msg.push_str(" This device opens it without asking for the password.");
                            }
                        }
                    }
                    self.wel.done = Some(msg);
                    self.wel.open = false;
                    self.sec.message = None;
                }
            }
            Some(Go::Skip) => self.finish_welcome(),
            Some(Go::Team) => {
                self.finish_welcome();
                self.screen = crate::app::Screen::Collaboration;
            }
            Some(Go::AddHost) => {
                self.finish_welcome();
                self.screen = crate::app::Screen::Hosts;
                self.edit_host(Host::default(), true);
            }
            Some(Go::Import) => {
                self.finish_welcome();
                self.screen = crate::app::Screen::Hosts;
                self.confirm_import = true;
            }
            Some(Go::Local) => {
                self.finish_welcome();
                let spec = self.local_spec();
                self.open_session(spec);
            }
            None => {}
        }
    }

    /// "Protect your vault": a one-line card that opens into the master password form.
    fn vault_card(&mut self, ui: &mut egui::Ui, go: &mut Option<Go>) {
        egui::Frame::new().fill(CARD()).stroke(Stroke::new(1.0, BORDER())).corner_radius(10).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 8.0;
            ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                egui::Frame::new().inner_margin(Margin::symmetric(16, 14)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(Vec2::splat(22.0), Sense::hover());
                        crate::icons::paint(ui.painter(), r.center(), 19.0, Icon::Shield, GREEN());
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 2.0;
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("Protect your vault").size(14.0).strong().color(TEXT()));
                                pill(ui, "Optional");
                            });
                            match &self.wel.done {
                                Some(msg) => ui.label(RichText::new(msg).size(12.5).color(GREEN())),
                                None => ui.label(RichText::new("Encrypt hosts, passwords and history with a master password.").size(12.5).color(MUTED())),
                            };
                        });
                        if !self.wel.open && self.wel.done.is_none() {
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if flat_button(ui, "Set up").clicked() {
                                    self.wel.open = true;
                                }
                            });
                        }
                    });
                });
                if !self.wel.open {
                    return;
                }
                let (line, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
                ui.painter().rect_filled(line, 0, BORDER());
                egui::Frame::new().inner_margin(Margin::same(16)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.spacing_mut().item_spacing.y = 8.0;
                    ui.label(RichText::new("A master password encrypts your hosts, saved passwords, logs and command history on this computer.").size(12.5).color(TEXT()));
                    egui::Frame::new().fill(AMBER().linear_multiply(0.12)).stroke(Stroke::new(1.0, AMBER().linear_multiply(0.45))).corner_radius(8).inner_margin(Margin::symmetric(12, 10)).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal_top(|ui| {
                            let (r, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
                            crate::icons::paint(ui.painter(), r.center() + Vec2::new(0.0, 2.0), 16.0, Icon::Alert, AMBER());
                            let mut job = egui::text::LayoutJob::default();
                            job.append("There is no way to recover it. ", 0.0, egui::TextFormat { font_id: FontId::proportional(12.5), color: AMBER(), ..Default::default() });
                            job.append("If you forget this password, the vault and everything in it is lost.", 0.0, egui::TextFormat { font_id: FontId::proportional(12.5), color: TEXT(), ..Default::default() });
                            ui.add(egui::Label::new(job).wrap());
                        });
                    });
                    ui.add_space(2.0);
                    field_label(ui, "Master password");
                    password_box(ui, &mut self.wel.pw, &mut self.wel.show_pw);
                    let s = strength(&self.wel.pw);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        let col = [BORDER(), RED(), AMBER(), AMBER(), GREEN()][s];
                        for i in 0..4 {
                            let (r, _) = ui.allocate_exact_size(Vec2::new(36.0, 4.0), Sense::hover());
                            ui.painter().rect_filled(r, 2, if i < s { col } else { BORDER() });
                        }
                        ui.add_space(6.0);
                        let (t, c) = if self.wel.pw.is_empty() || s == 0 { ("At least 8 characters", MUTED()) } else { (["", "Weak", "Fair", "Good", "Strong"][s], col) };
                        ui.label(RichText::new(t).size(11.5).color(c));
                    });
                    ui.add_space(2.0);
                    field_label(ui, "Repeat password");
                    password_box(ui, &mut self.wel.confirm, &mut self.wel.show_confirm);
                    let mismatch = !self.wel.confirm.is_empty() && self.wel.pw != self.wel.confirm;
                    if mismatch {
                        ui.label(RichText::new("The passwords do not match.").size(11.5).color(RED()));
                    }
                    ui.add_space(6.0);
                    check_row(ui, &mut self.wel.keep, "Keep me unlocked on this device", Some("Stored in the system keychain. Anyone using this computer can open the vault without the password."));
                    check_row(ui, &mut self.wel.understood, "I understand this password can't be recovered", None);
                    if let Some((m, false)) = &self.sec.message {
                        ui.label(RichText::new(m).size(12.0).color(RED()));
                    }
                    ui.add_space(4.0);
                    let ok = self.wel.pw.chars().count() >= 8 && self.wel.pw == self.wel.confirm && self.wel.understood;
                    ui.horizontal(|ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.add_enabled_ui(ok, |ui| {
                                if primary_button(ui, "Encrypt vault").clicked() {
                                    *go = Some(Go::Encrypt);
                                }
                            });
                            if flat_button(ui, "Not now").clicked() {
                                self.wel.open = false;
                                self.wel.pw.clear();
                                self.wel.confirm.clear();
                                self.wel.understood = false;
                                self.sec.message = None;
                            }
                        });
                    });
                });
            });
        });
    }

    fn finish_welcome(&mut self) {
        self.vault.settings.onboarded = true;
        self.dirty = true;
        self.wel = Default::default();
    }
}

#[cfg(test)]
mod tests {
    use super::strength;

    #[test]
    fn short_passwords_score_zero_and_long_mixed_ones_score_high() {
        assert_eq!(strength(""), 0);
        assert_eq!(strength("abc1234"), 0);
        assert_eq!(strength("abcdefgh"), 1);
        assert_eq!(strength("Abcdefg1"), 2);
        assert_eq!(strength("Abcdefghij1k"), 3);
        assert_eq!(strength("Abcdefghijk1lmn!"), 4);
    }
}
