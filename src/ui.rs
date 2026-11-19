//! Look of the app (colours) and small drawing helpers.

use eframe::egui::{self, Align2, Color32, CornerRadius, FontId, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Vec2};

use std::sync::atomic::{AtomicBool, Ordering};

/// Which of the two palettes is in use. Set by `sync_theme`.
static LIGHT: AtomicBool = AtomicBool::new(false);

pub fn is_light() -> bool {
    LIGHT.load(Ordering::Relaxed)
}

/// A colour that differs by theme: `dark` or `light`, as 0xRRGGBB.
pub fn pick(dark: u32, light: u32) -> Color32 {
    let c = if is_light() { light } else { dark };
    Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

/// A faint wash for hover states: white on the dark theme, black on the light one.
pub fn wash(alpha: u8) -> Color32 {
    if is_light() { Color32::from_black_alpha((alpha as f32 * 0.7) as u8) } else { Color32::from_white_alpha(alpha) }
}

#[allow(non_snake_case)]
pub fn BG_WINDOW() -> Color32 {
    pick(0x171A2B, 0xE6E9F0)
}
#[allow(non_snake_case)]
pub fn BG_CONTENT() -> Color32 {
    pick(0x1E2236, 0xF4F5F9)
}
#[allow(non_snake_case)]
pub fn CARD() -> Color32 {
    pick(0x272B42, 0xFFFFFF)
}
#[allow(non_snake_case)]
pub fn CARD_HOVER() -> Color32 {
    pick(0x313650, 0xEBEDF3)
}
/// The fill of a hovered control.
#[allow(non_snake_case)]
pub fn HOVER() -> Color32 {
    pick(0x3B4160, 0xDDE0E8)
}
#[allow(non_snake_case)]
pub fn BORDER() -> Color32 {
    pick(0x363B57, 0xD3D7E1)
}
#[allow(non_snake_case)]
pub fn TEXT() -> Color32 {
    pick(0xE2E3E8, 0x1B1E2E)
}
#[allow(non_snake_case)]
pub fn MUTED() -> Color32 {
    pick(0x9AA0B8, 0x666B80)
}
#[allow(non_snake_case)]
pub fn BLUE() -> Color32 {
    pick(0x2091F6, 0x1B7FE3)
}
#[allow(non_snake_case)]
pub fn GREEN() -> Color32 {
    pick(0x21B568, 0x16914F)
}
#[allow(non_snake_case)]
pub fn RED() -> Color32 {
    pick(0xF24E50, 0xD63638)
}
#[allow(non_snake_case)]
pub fn AMBER() -> Color32 {
    pick(0xECA855, 0xB06A0F)
}

/// Follows the "App theme" setting ("dark", "light" or "system"): switches the palette and the egui style when it changes.
pub fn sync_theme(ctx: &egui::Context, pref: &str) {
    let want_light = match pref {
        "light" => true,
        "system" => ctx.system_theme() == Some(egui::Theme::Light),
        _ => false,
    };
    if want_light != is_light() {
        LIGHT.store(want_light, Ordering::Relaxed);
        install_style(ctx);
    }
}

/// Runs `f` with the dark palette, whatever the app theme is. For parts of the window that stay dark (the tab strip).
pub fn with_dark<R>(f: impl FnOnce() -> R) -> R {
    let was = LIGHT.swap(false, Ordering::Relaxed);
    let r = f();
    LIGHT.store(was, Ordering::Relaxed);
    r
}

/// Widget colours for the current palette.
pub fn visuals() -> egui::Visuals {
    let mut v = if is_light() { egui::Visuals::light() } else { egui::Visuals::dark() };
    v.panel_fill = BG_CONTENT();
    v.window_fill = CARD();
    v.extreme_bg_color = BG_WINDOW();
    v.faint_bg_color = CARD();
    v.override_text_color = Some(TEXT());
    v.selection.bg_fill = BLUE().linear_multiply(0.45);
    v.selection.stroke = Stroke::new(1.0, BLUE());
    v.hyperlink_color = BLUE();
    v.window_stroke = Stroke::new(1.0, BORDER());
    let r = CornerRadius::same(6);
    for w in [&mut v.widgets.noninteractive, &mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
        w.corner_radius = r;
    }
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER());
    v.widgets.inactive.bg_fill = CARD_HOVER();
    v.widgets.inactive.weak_bg_fill = CARD_HOVER();
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, BORDER());
    v.widgets.hovered.bg_fill = HOVER();
    v.widgets.hovered.weak_bg_fill = HOVER();
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, MUTED());
    v.widgets.active.bg_fill = BLUE();
    v.widgets.active.weak_bg_fill = BLUE();
    v
}

pub fn install_style(ctx: &egui::Context) {
    ctx.set_theme(if is_light() { egui::ThemePreference::Light } else { egui::ThemePreference::Dark });
    ctx.set_visuals(visuals());
    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = Vec2::new(8.0, 8.0);
        s.spacing.button_padding = Vec2::new(12.0, 6.0);
        s.spacing.interact_size.y = 30.0;
        s.visuals.menu_corner_radius = CornerRadius::same(8);
        s.spacing.menu_margin = egui::Margin::same(8);
        // scrollbars keep one thin width instead of swelling when the pointer comes near
        // thin, and coloured from the app's own widget colours (grey at rest, lighter on hover, blue while dragged)
        let sc = &mut s.spacing.scroll;
        sc.bar_width = 4.0;
        sc.floating_width = 4.0;
        // keep a gutter for the bar so it doesn't sit on top of the cards
        sc.floating_allocated_width = 12.0;
        sc.foreground_color = false;
        sc.dormant_handle_opacity = 1.0;
        sc.active_handle_opacity = 1.0;
        sc.interact_handle_opacity = 1.0;
        sc.dormant_background_opacity = 0.0;
        sc.active_background_opacity = 0.0;
        sc.interact_background_opacity = 0.0;
    });
}

pub fn primary_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(text).color(Color32::WHITE).strong().size(12.5))
            .fill(BLUE())
            .stroke(Stroke::NONE)
            .corner_radius(6)
            .min_size(Vec2::new(0.0, 32.0)),
    )
}

pub fn flat_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(egui::Button::new(RichText::new(text).size(12.5)).corner_radius(6).min_size(Vec2::new(0.0, 32.0)))
}

/// A flat button with a drawn icon on its left.
pub fn icon_text_button(ui: &mut egui::Ui, icon: crate::icons::Icon, text: &str) -> egui::Response {
    let r = ui.add(egui::Button::new(RichText::new(format!("       {text}")).size(12.5)).corner_radius(6).min_size(Vec2::new(0.0, 32.0)));
    crate::icons::paint(ui.painter(), egui::Pos2::new(r.rect.left() + 20.0, r.rect.center().y), 14.0, icon, MUTED());
    r
}

/// A submenu entry with a drawn icon on its left, lined up with `icon_text_button`.
pub fn icon_menu_button<R>(ui: &mut egui::Ui, icon: crate::icons::Icon, text: &str, add: impl FnOnce(&mut egui::Ui) -> R) {
    ui.spacing_mut().button_padding = Vec2::new(12.0, 7.0);
    let r = ui.menu_button(RichText::new(format!("       {text}")).size(12.5), |ui| {
        ui.spacing_mut().button_padding = Vec2::new(10.0, 6.0);
        ui.spacing_mut().item_spacing.y = 2.0;
        ui.set_min_width(160.0);
        ui.set_max_width(220.0);
        add(ui)
    });
    crate::icons::paint(ui.painter(), egui::Pos2::new(r.response.rect.left() + 20.0, r.response.rect.center().y), 14.0, icon, MUTED());
}

/// One entry in a submenu: full width, the same text size as the menu it came from.
pub fn submenu_item(ui: &mut egui::Ui, text: &str) -> egui::Response {
    // the menu's layout stretches it to the submenu's width; asking for the available width here would widen the submenu
    ui.add(egui::Button::new(RichText::new(text).size(12.5)).corner_radius(6).min_size(Vec2::new(0.0, 30.0)))
}

pub fn danger_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(text).color(RED()).size(12.5))
            .fill(Color32::TRANSPARENT)
            .stroke(Stroke::new(1.0, RED().linear_multiply(0.6)))
            .corner_radius(6)
            .min_size(Vec2::new(0.0, 32.0)),
    )
}

pub fn label_muted(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).color(MUTED()).size(12.0));
}

pub fn section_title(ui: &mut egui::Ui, text: &str) {
    ui.add_space(6.0);
    ui.label(RichText::new(text).strong().size(13.0).color(MUTED()));
}

/// A labelled field row used by the editor panels.
pub fn field(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.label(RichText::new(label).size(11.5).color(MUTED()));
    add(ui);
    ui.add_space(2.0);
}

pub fn text_field(ui: &mut egui::Ui, label: &str, value: &mut String, hint: &str, password: bool) {
    text_field_ex(ui, label, false, None, None, value, hint, password);
}

/// A section heading in a form: a small muted title with a hairline running to the right edge.
pub fn form_section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(10.0);
    let g = ui.painter().layout_no_wrap(title.to_string(), FontId::proportional(11.5), MUTED());
    let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), g.size().y + 2.0), Sense::hover());
    let y = r.center().y;
    let gw = g.size().x;
    ui.painter().galley(Pos2::new(r.left(), y - g.size().y / 2.0), g, MUTED());
    ui.painter().line_segment([Pos2::new(r.left() + gw + 8.0, y), Pos2::new(r.right(), y)], Stroke::new(1.0, BORDER()));
    ui.add_space(2.0);
}

/// The small muted label above a field; a red star marks a required one.
pub fn field_label(ui: &mut egui::Ui, label: &str, required: bool) {
    let mut job = egui::text::LayoutJob::default();
    job.append(label, 0.0, egui::text::TextFormat { font_id: FontId::proportional(11.5), color: MUTED(), ..Default::default() });
    if required {
        job.append(" *", 0.0, egui::text::TextFormat { font_id: FontId::proportional(11.5), color: RED(), ..Default::default() });
    }
    ui.label(job);
}

/// Two fields side by side. `right_w` fixes the right column's width; without it the columns are equal.
pub fn split_row(ui: &mut egui::Ui, right_w: Option<f32>, left: impl FnOnce(&mut egui::Ui), right: impl FnOnce(&mut egui::Ui)) {
    let gap = 8.0;
    let total = ui.available_width();
    let rw = right_w.unwrap_or((total - gap) / 2.0);
    let lw = (total - gap - rw).max(60.0);
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        ui.vertical(|ui| {
            ui.set_width(lw);
            ui.spacing_mut().item_spacing.y = 4.0;
            left(ui);
        });
        ui.vertical(|ui| {
            ui.set_width(rw);
            ui.spacing_mut().item_spacing.y = 4.0;
            right(ui);
        });
    });
}

/// A labelled text input with an optional leading icon, a required marker and an inline error.
pub fn text_field_ex(ui: &mut egui::Ui, label: &str, required: bool, error: Option<&str>, icon: Option<crate::icons::Icon>, value: &mut String, hint: &str, secret: bool) {
    field_label(ui, label, required);
    input_box(ui, egui::Id::new(("form-input", label)), value, hint, icon, secret, error.is_some());
    if let Some(e) = error {
        ui.label(RichText::new(e).size(11.5).color(RED()));
    }
    ui.add_space(2.0);
}

/// A multi-line box in the same style as `input_box` (monospace, for scripts).
pub fn multiline_box(ui: &mut egui::Ui, id: egui::Id, value: &mut String, hint: &str, rows: usize, invalid: bool) -> egui::Response {
    let edit_id = id.with("edit");
    let focused = ui.memory(|m| m.has_focus(edit_id));
    let (ring, halo) = if invalid {
        (RED(), Some(RED().linear_multiply(0.2)))
    } else if focused {
        (BLUE(), Some(BLUE().linear_multiply(0.25)))
    } else {
        (BORDER(), None)
    };
    let out = egui::Frame::new().fill(BG_WINDOW()).stroke(Stroke::new(1.0, ring)).corner_radius(6).inner_margin(egui::Margin::symmetric(10, 8)).show(ui, |ui| {
        ui.add(
            egui::TextEdit::multiline(value)
                .id(edit_id)
                .frame(egui::Frame::NONE)
                .font(egui::TextStyle::Monospace)
                .desired_rows(rows)
                .desired_width(f32::INFINITY)
                .hint_text(hint)
                .margin(Vec2::ZERO),
        )
    });
    if let Some(h) = halo {
        ui.painter().rect_stroke(out.response.rect, 6.0, Stroke::new(3.0, h), StrokeKind::Outside);
    }
    out.inner
}

/// The input itself: a dark rounded box that gets a blue ring while focused and a red one when invalid.
/// A secret box shows an eye on the right to reveal what was typed.
pub fn input_box(ui: &mut egui::Ui, id: egui::Id, value: &mut String, hint: &str, icon: Option<crate::icons::Icon>, secret: bool, invalid: bool) -> egui::Response {
    let edit_id = id.with("edit");
    let focused = ui.memory(|m| m.has_focus(edit_id));
    let show = ui.data(|d| d.get_temp::<bool>(id.with("show"))).unwrap_or(false);
    let (ring, halo) = if invalid {
        (RED(), Some(RED().linear_multiply(0.2)))
    } else if focused {
        (BLUE(), Some(BLUE().linear_multiply(0.25)))
    } else {
        (BORDER(), None)
    };
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 34.0), Sense::hover());
    let boxed = ui.interact(rect, id.with("box"), Sense::click());
    if boxed.clicked() {
        ui.memory_mut(|m| m.request_focus(edit_id));
    }
    if let Some(h) = halo {
        ui.painter().rect_stroke(rect, 6.0, Stroke::new(3.0, h), StrokeKind::Outside);
    }
    ui.painter().rect(rect, 6.0, BG_WINDOW(), Stroke::new(1.0, ring), StrokeKind::Inside);
    let mut left = rect.left() + 10.0;
    if let Some(ic) = icon {
        crate::icons::paint(ui.painter(), Pos2::new(left + 7.0, rect.center().y), 14.0, ic, if invalid { RED() } else { MUTED() });
        left += 22.0;
    }
    let right = rect.right() - if secret { 34.0 } else { 10.0 };
    let inner = Rect::from_min_max(Pos2::new(left, rect.top()), Pos2::new(right.max(left + 20.0), rect.bottom()));
    let mut edit_ui = ui.new_child(egui::UiBuilder::new().max_rect(inner));
    let r = edit_ui.add(
        egui::TextEdit::singleline(value)
            .id(edit_id)
            .frame(egui::Frame::NONE)
            .hint_text(hint)
            .password(secret && !show)
            .desired_width(inner.width())
            .margin(Vec2::ZERO)
            .vertical_align(egui::Align::Center)
            .min_size(Vec2::new(0.0, 34.0)),
    );
    if secret {
        let eye = Rect::from_center_size(Pos2::new(rect.right() - 18.0, rect.center().y), Vec2::splat(24.0));
        let resp = ui.interact(eye, id.with("eye"), Sense::click());
        let col = if resp.hovered() { TEXT() } else { MUTED() };
        crate::icons::paint(ui.painter(), eye.center(), 15.0, if show { crate::icons::Icon::EyeOff } else { crate::icons::Icon::Eye }, col);
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if resp.clicked() {
            ui.data_mut(|d| d.insert_temp(id.with("show"), !show));
        }
    }
    r
}

pub fn toggle(ui: &mut egui::Ui, on: &mut bool) -> egui::Response {
    let size = Vec2::new(34.0, 18.0);
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool_responsive(resp.id, *on);
    let bg = if *on { GREEN() } else { BORDER() };
    ui.painter().rect_filled(rect, 9.0, bg);
    let cx = rect.left() + 9.0 + t * (size.x - 18.0);
    ui.painter().circle_filled(Pos2::new(cx, rect.center().y), 7.0, Color32::WHITE);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

pub fn setting_row(ui: &mut egui::Ui, title: &str, desc: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.set_width(ui.available_width() - 190.0);
            ui.label(RichText::new(title).size(13.5));
            if !desc.is_empty() {
                ui.label(RichText::new(desc).size(11.5).color(MUTED()));
            }
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), add);
    });
    ui.add_space(6.0);
}

pub fn icon_color(seed: &str) -> Color32 {
    let pal = [BLUE(), GREEN(), AMBER(), Color32::from_rgb(0xC1, 0x32, 0x82), Color32::from_rgb(0x3E, 0xD7, 0xBE), RED(), Color32::from_rgb(0x6B, 0xA0, 0xC3)];
    let h = seed.bytes().fold(7usize, |a, b| a.wrapping_mul(31).wrapping_add(b as usize));
    pal[h % pal.len()]
}

/// Rounded square with a letter or glyph inside (stands in for host / key icons).
pub fn badge(painter: &egui::Painter, rect: Rect, text: &str, color: Color32) {
    painter.rect_filled(rect, 8.0, color.linear_multiply(0.9));
    if let Some(ic) = crate::icons::Icon::parse(text) {
        crate::icons::paint(painter, rect.center(), rect.height() * 0.52, ic, Color32::WHITE);
    } else {
        painter.text(rect.center(), Align2::CENTER_CENTER, text, FontId::proportional(rect.height() * 0.45), Color32::WHITE);
    }
}

pub struct CardOut {
    pub resp: egui::Response,
    pub rect: Rect,
}

/// A clickable card; the caller draws the contents with the returned painter and rect.
pub fn card(ui: &mut egui::Ui, size: Vec2, selected: bool) -> CardOut {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let hovered = resp.hovered();
    let fill = if hovered { CARD_HOVER() } else { CARD() };
    ui.painter().rect_filled(rect, 10.0, fill);
    let stroke = if selected { Stroke::new(1.5, BLUE()) } else { Stroke::new(1.0, BORDER()) };
    ui.painter().rect_stroke(rect, 10.0, stroke, StrokeKind::Inside);
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    CardOut { resp, rect }
}

pub fn status_dot(painter: &egui::Painter, center: Pos2, color: Color32) {
    painter.circle_filled(center, 4.0, color);
}

/// Returns (tab clicked, close clicked, the tab's response for context menus).
pub fn tab_button(ui: &mut egui::Ui, label: &str, active: bool, closable: bool, dot: Option<Color32>, glyph: Option<&str>) -> (bool, bool, egui::Response) {
    tab_button_with(ui, label, active, closable, dot, glyph, &[])
}

/// A tab that also shows a stack of avatars (the people in a shared terminal).
pub fn tab_button_with(ui: &mut egui::Ui, label: &str, active: bool, closable: bool, dot: Option<Color32>, glyph: Option<&str>, people: &[(String, Color32)]) -> (bool, bool, egui::Response) {
    let font = FontId::proportional(13.0);
    let galley = ui.painter().layout_no_wrap(label.to_string(), font.clone(), TEXT());
    let mut w = galley.size().x + 28.0;
    if closable {
        w += 22.0;
    }
    if dot.is_some() || glyph.is_some() {
        w += 18.0;
    }
    let stack = if people.is_empty() { 0.0 } else { 18.0 + 14.0 * (people.len() - 1) as f32 + 6.0 };
    let w = (w + stack).clamp(80.0, 260.0);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 32.0), Sense::click());
    let hovered = resp.hovered();
    let bg = if active { BG_CONTENT() } else if hovered { CARD() } else { Color32::TRANSPARENT };
    ui.painter().rect_filled(rect, CornerRadius { nw: 8, ne: 8, sw: 0, se: 0 }, bg);
    let mut x = rect.left() + 12.0;
    if let Some(d) = dot {
        status_dot(ui.painter(), Pos2::new(x + 3.0, rect.center().y), d);
        x += 14.0;
    }
    if let Some(g) = glyph {
        let gc = if active { TEXT() } else { MUTED() };
        if let Some(ic) = crate::icons::Icon::parse(g) {
            crate::icons::paint(ui.painter(), Pos2::new(x + 7.0, rect.center().y), 14.0, ic, gc);
        } else {
            ui.painter().text(Pos2::new(x, rect.center().y), Align2::LEFT_CENTER, g, FontId::proportional(14.0), gc);
        }
        x += 18.0;
    }
    let max_w = rect.right() - x - stack - if closable { 26.0 } else { 10.0 };
    // the people, overlapping, just before the close button
    let right = rect.right() - if closable { 28.0 } else { 10.0 };
    for (i, (name, col)) in people.iter().enumerate().rev() {
        avatar(ui.painter(), Pos2::new(right - 9.0 - 14.0 * (people.len() - 1 - i) as f32, rect.center().y), 9.0, name, *col);
    }
    let clip = Rect::from_min_max(Pos2::new(x, rect.top()), Pos2::new(x + max_w, rect.bottom()));
    ui.painter().with_clip_rect(clip).text(Pos2::new(x, rect.center().y), Align2::LEFT_CENTER, label, font, if active { TEXT() } else { MUTED() });
    let mut close_clicked = false;
    if closable {
        let cr = Rect::from_center_size(Pos2::new(rect.right() - 16.0, rect.center().y), Vec2::splat(18.0));
        let cresp = ui.interact(cr, resp.id.with("close"), Sense::click());
        if cresp.hovered() {
            ui.painter().rect_filled(cr, 4.0, BORDER());
        }
        crate::icons::paint(ui.painter(), cr.center(), 9.0, crate::icons::Icon::Close, MUTED());
        close_clicked = cresp.clicked();
    }
    (resp.clicked() && !close_clicked, close_clicked, resp)
}

pub fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\x1b' => match chars.peek() {
                Some('[') => {
                    chars.next();
                    for n in chars.by_ref() {
                        if ('@'..='~').contains(&n) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    chars.next();
                    while let Some(n) = chars.next() {
                        if n == '\x07' {
                            break;
                        }
                        if n == '\x1b' {
                            chars.next();
                            break;
                        }
                    }
                }
                Some(_) => {
                    chars.next();
                }
                None => {}
            },
            '\r' => {}
            '\x07' | '\x08' => {}
            c => out.push(c),
        }
    }
    out
}

pub fn fmt_time(secs: u64, tz_offset: i64) -> String {
    let t = secs as i64 + tz_offset;
    let days = t.div_euclid(86400);
    let rem = t.rem_euclid(86400);
    // civil-from-days (Howard Hinnant)
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{:04}-{:02}-{:02} {:02}:{:02}", y, m, d, rem / 3600, (rem % 3600) / 60)
}

pub fn fmt_size(n: u64) -> String {
    if n < 1024 {
        format!("{} B", n)
    } else if n < 1024 * 1024 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else {
        format!("{:.1} MB", n as f64 / 1048576.0)
    }
}

/// A dimmed cover with a spinner card, shown over a screen while a connection is being made.
/// Returns true when Cancel was pressed.
pub fn connecting_overlay(ui: &mut egui::Ui, rect: Rect, title: &str, sub: &str) -> bool {
    let p = ui.painter().clone();
    p.rect_filled(rect, 0.0, Color32::from_rgba_unmultiplied(0x10, 0x10, 0x21, 215));
    let card = Rect::from_center_size(rect.center(), Vec2::new(340.0, 196.0));
    p.rect_filled(card, 14.0, CARD());
    p.rect_stroke(card, 14.0, Stroke::new(1.0, BORDER()), StrokeKind::Inside);
    let c = Pos2::new(card.center().x, card.top() + 50.0);
    let t = ui.input(|i| i.time) as f32;
    p.circle_stroke(c, 16.0, Stroke::new(3.0, BORDER()));
    let a0 = t * 4.5;
    let pts: Vec<Pos2> = (0..=24).map(|i| {
        let a = a0 + std::f32::consts::PI * 1.2 * i as f32 / 24.0;
        Pos2::new(c.x + 16.0 * a.cos(), c.y + 16.0 * a.sin())
    }).collect();
    p.add(egui::Shape::line(pts, Stroke::new(3.0, BLUE())));
    let inner = Rect::from_min_max(Pos2::new(card.left() + 16.0, card.top()), Pos2::new(card.right() - 16.0, card.bottom()));
    let pc = p.with_clip_rect(inner);
    pc.text(Pos2::new(card.center().x, card.top() + 98.0), Align2::CENTER_CENTER, title, FontId::proportional(15.0), TEXT());
    pc.text(Pos2::new(card.center().x, card.top() + 122.0), Align2::CENTER_CENTER, sub, FontId::proportional(12.0), MUTED());
    let br = Rect::from_center_size(Pos2::new(card.center().x, card.bottom() - 28.0), Vec2::new(96.0, 30.0));
    let resp = ui.put(br, egui::Button::new(RichText::new("Cancel").size(12.5)).corner_radius(6));
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(16));
    resp.clicked()
}

/// A check run can force the overlay on (no network needed) with SSHTUP_FORCE_CONNECTING=1.
pub fn force_connecting() -> bool {
    std::env::var("SSHTUP_FORCE_CONNECTING").map(|v| v == "1").unwrap_or(false)
}

/// A check run can open the SFTP path box with a list under it (no clicks needed) with SSHTUP_DEMO_PATHAC=1.
pub fn demo_pathac() -> bool {
    std::env::var("SSHTUP_DEMO_PATHAC").map(|v| v == "1").unwrap_or(false)
}

/// What the overlay says under its title, depending on how long the wait has been.
pub fn connecting_hint(secs: u64) -> String {
    match secs {
        0..=2 => "Opening the connection…".into(),
        3..=9 => format!("Waiting for the server… {secs} s"),
        _ => format!("Still waiting… {secs} s. Cancel if the host is not reachable"),
    }
}


/// A blocking dialog: dimmed backdrop, clicks do not pass through, Esc or a click outside closes it.
/// Returns what `content` returned and whether the user asked to close it.
pub fn app_modal<T>(ctx: &egui::Context, id: &str, title: &str, content: impl FnOnce(&mut egui::Ui) -> T) -> (T, bool) {
    let frame = egui::Frame::popup(&ctx.global_style()).inner_margin(egui::Margin::symmetric(28, 24));
    let m = egui::Modal::new(egui::Id::new(id)).frame(frame).show(ctx, |ui| {
        ui.label(RichText::new(title).size(15.0).strong().color(TEXT()));
        ui.add_space(8.0);
        content(ui)
    });
    let closed = m.should_close();
    (m.inner, closed)
}

/// Buttons of a dialog, pushed to the right edge. The first one added ends up at the far right.
pub fn modal_buttons<T>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> T) -> T {
    ui.add_space(14.0);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), add).inner
}

pub enum PwAnswer {
    Waiting,
    Submit,
    Cancel,
}

/// A check run can force the password window on (no network needed) with SSHTUP_FORCE_PASSWORD=1.
pub fn force_password() -> bool {
    std::env::var("SSHTUP_FORCE_PASSWORD").map(|v| v == "1").unwrap_or(false)
}

/// The window that asks for a password or passphrase over the page, the page is dimmed,
/// a small card holds the field, and Enter or Continue sends it.
#[allow(clippy::too_many_arguments)]
pub fn password_modal(ctx: &egui::Context, area: Rect, who: &str, passphrase: bool, wrong: bool, text: &mut String, remember: Option<&mut bool>) -> PwAnswer {
    let _ = area;
    let mut ans = PwAnswer::Waiting;
    let what = if passphrase { "Passphrase" } else { "Password" };
    let (_, closed) = app_modal(ctx, &format!("pw_card_{who}"), &format!("{what} required"), |ui| {
            ui.set_width(330.0);
            ui.label(RichText::new(who).size(13.5).strong().color(TEXT()));
            ui.add_space(2.0);
            if wrong {
                ui.label(RichText::new(format!("{what} was not accepted. Try again.")).size(12.0).color(RED()));
            } else {
                ui.label(RichText::new(if passphrase { "Enter the passphrase for the key." } else { "Enter the password for this host." }).size(12.0).color(MUTED()));
            }
            ui.add_space(8.0);
            let r = ui.add(egui::TextEdit::singleline(text).password(true).hint_text(what).desired_width(f32::INFINITY).margin(Vec2::new(10.0, 8.0)));
            r.request_focus();
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if let Some(rem) = remember {
                ui.add_space(6.0);
                ui.checkbox(rem, RichText::new("Save it for this host").size(12.5));
            }
            ui.add_space(10.0);
            modal_buttons(ui, |ui| {
                if (primary_button(ui, "Continue").clicked() || enter) && !text.is_empty() {
                    ans = PwAnswer::Submit;
                }
                if flat_button(ui, "Cancel").clicked() {
                    ans = PwAnswer::Cancel;
                }
            });
    });
    if closed {
        ans = PwAnswer::Cancel;
    }
    ctx.request_repaint_after(std::time::Duration::from_millis(100));
    ans
}

/// A round badge with the first letter of a name, in the person's colour (rings use the same colour).
pub fn avatar(painter: &egui::Painter, center: Pos2, radius: f32, name: &str, color: Color32) {
    painter.circle_filled(center, radius, color);
    painter.circle_stroke(center, radius, Stroke::new(1.5, Color32::from_black_alpha(90)));
    let letter: String = name.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_else(|| "?".into());
    painter.text(center, Align2::CENTER_CENTER, letter, FontId::proportional(radius * 1.15), Color32::from_rgb(20, 22, 30));
}

#[derive(Clone, Copy, PartialEq)]
pub enum SplitIcon {
    Right,
    Below,
    Close,
    Search,
}

/// A small square button in the tab strip: a window cut in two (or crossed out, to join the halves again).
pub fn split_button(ui: &mut egui::Ui, icon: SplitIcon, tip: &str) -> egui::Response {
    let (r, resp) = ui.allocate_exact_size(Vec2::splat(28.0), egui::Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(r, 6.0, CARD_HOVER());
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let col = if resp.hovered() { TEXT() } else { MUTED() };
    let p = ui.painter();
    let st = egui::Stroke::new(1.4, col);
    let b = egui::Rect::from_center_size(r.center(), Vec2::new(14.0, 12.0));
    match icon {
        SplitIcon::Right => {
            p.rect_stroke(b, 2.0, st, egui::StrokeKind::Inside);
            p.line_segment([Pos2::new(b.center().x, b.top()), Pos2::new(b.center().x, b.bottom())], st);
        }
        SplitIcon::Below => {
            p.rect_stroke(b, 2.0, st, egui::StrokeKind::Inside);
            p.line_segment([Pos2::new(b.left(), b.center().y), Pos2::new(b.right(), b.center().y)], st);
        }
        SplitIcon::Search => {
            let c = r.center() + Vec2::new(-1.5, -1.5);
            p.circle_stroke(c, 5.0, st);
            p.line_segment([c + Vec2::new(3.6, 3.6), c + Vec2::new(7.0, 7.0)], st);
        }
        SplitIcon::Close => {
            let c = r.center();
            p.line_segment([c + Vec2::new(-4.5, -4.5), c + Vec2::new(4.5, 4.5)], st);
            p.line_segment([c + Vec2::new(-4.5, 4.5), c + Vec2::new(4.5, -4.5)], st);
        }
    }
    resp.on_hover_text(tip)
}

/// A row in a pop-up menu: an icon, then the text. `color` tints both (the text turns white on hover).
pub fn icon_item(ui: &mut egui::Ui, icon: crate::icons::Icon, label: &str, color: Color32) -> egui::Response {
    let w = 200.0;
    let (r, resp) = ui.allocate_exact_size(Vec2::new(w, 32.0), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(r, 6.0, CARD_HOVER().gamma_multiply(1.0).lerp_to_gamma(if is_light() { Color32::BLACK } else { Color32::WHITE }, 0.05));
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    crate::icons::paint(ui.painter(), Pos2::new(r.left() + 18.0, r.center().y), 15.0, icon, color);
    ui.painter().text(Pos2::new(r.left() + 38.0, r.center().y), Align2::LEFT_CENTER, label, FontId::proportional(13.5), color);
    resp
}
