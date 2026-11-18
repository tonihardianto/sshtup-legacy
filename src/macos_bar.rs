//! macOS: centre the window buttons ("traffic lights") vertically in our tab strip.
use objc2_app_kit::{NSView, NSWindowButton};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

/// Height of the strip the buttons are centred in (points).
pub const BAR_H: f64 = 48.0;

pub fn align(frame: &impl HasWindowHandle) {
    let Ok(h) = frame.window_handle() else { return };
    let RawWindowHandle::AppKit(h) = h.as_raw() else { return };
    unsafe {
        let view: &NSView = &*(h.ns_view.as_ptr() as *const NSView);
        let Some(win) = view.window() else { return };
        let Some(close) = win.standardWindowButton(NSWindowButton::NSWindowCloseButton) else { return };
        let Some(mini) = win.standardWindowButton(NSWindowButton::NSWindowMiniaturizeButton) else { return };
        let Some(zoom) = win.standardWindowButton(NSWindowButton::NSWindowZoomButton) else { return };
        let Some(bar) = close.superview() else { return };
        let Some(container) = bar.superview() else { return };
        let win_h = win.frame().size.height;
        let mut cf = container.frame();
        if (cf.size.height - BAR_H).abs() > 0.5 || (cf.origin.y - (win_h - BAR_H)).abs() > 0.5 {
            cf.size.height = BAR_H;
            cf.origin.y = win_h - BAR_H;
            container.setFrame(cf);
        }
        let bh = close.frame().size.height;
        let y = ((BAR_H - bh) / 2.0).round();
        let mut x = 14.0;
        for b in [&close, &mini, &zoom] {
            let mut f = b.frame();
            if (f.origin.y - y).abs() > 0.5 || (f.origin.x - x).abs() > 0.5 {
                f.origin.y = y;
                f.origin.x = x;
                b.setFrame(f);
            }
            x += f.size.width + 8.0;
        }
    }
}
