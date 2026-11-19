//! The terminal font. egui ships one monospace face (Hack); other monospace fonts already installed on this computer can be
//! picked in Settings. Nothing is bundled: a font that is not installed simply does not show in the list.
//!
//! Terminals draw with the font family `term`, which is the chosen font followed by egui's own monospace fallbacks, so the
//! rest of the app keeps Hack whatever the terminal uses.

use eframe::egui::{self, FontData, FontDefinitions, FontFamily, FontId};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

/// Shown for the empty setting.
pub const BUILT_IN: &str = "Hack";

/// Fonts looked for, by name, with the file names they are installed under (regular weight only).
const KNOWN: &[(&str, &[&str])] = &[
    ("JetBrains Mono", &["JetBrainsMono-Regular.ttf", "JetBrainsMono[wght].ttf", "JetBrainsMono-VariableFont_wght.ttf"]),
    ("Fira Code", &["FiraCode-Regular.ttf", "FiraCode-VariableFont_wght.ttf"]),
    ("Fira Mono", &["FiraMono-Regular.ttf"]),
    ("Source Code Pro", &["SourceCodePro-Regular.ttf", "SourceCodePro-Regular.otf"]),
    ("IBM Plex Mono", &["IBMPlexMono-Regular.ttf"]),
    ("Cascadia Mono", &["CascadiaMono.ttf", "CascadiaMono-Regular.ttf"]),
    ("Cascadia Code", &["CascadiaCode.ttf", "CascadiaCode-Regular.ttf"]),
    ("SF Mono", &["SFNSMono.ttf", "SFMono-Regular.otf"]),
    ("Menlo", &["Menlo.ttc"]),
    ("Monaco", &["Monaco.ttf"]),
    ("Consolas", &["consola.ttf"]),
    ("Courier New", &["cour.ttf"]),
    ("DejaVu Sans Mono", &["DejaVuSansMono.ttf"]),
    ("Ubuntu Mono", &["UbuntuMono-R.ttf", "UbuntuMono-Regular.ttf"]),
    ("Liberation Mono", &["LiberationMono-Regular.ttf"]),
    ("Noto Sans Mono", &["NotoSansMono-Regular.ttf"]),
];

fn font_dirs() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let mut dirs: Vec<PathBuf> = vec![];
    if cfg!(target_os = "macos") {
        if let Some(h) = &home {
            dirs.push(h.join("Library/Fonts"));
        }
        dirs.push("/Library/Fonts".into());
        dirs.push("/System/Library/Fonts".into());
        dirs.push("/Applications/Utilities/Terminal.app/Contents/Resources/Fonts".into());
        dirs.push("/System/Applications/Utilities/Terminal.app/Contents/Resources/Fonts".into());
    } else if cfg!(target_os = "windows") {
        let win = std::env::var_os("WINDIR").map(PathBuf::from).unwrap_or_else(|| "C:\\Windows".into());
        dirs.push(win.join("Fonts"));
        if let Some(l) = std::env::var_os("LOCALAPPDATA") {
            dirs.push(PathBuf::from(l).join("Microsoft\\Windows\\Fonts"));
        }
    } else {
        if let Some(h) = &home {
            dirs.push(h.join(".local/share/fonts"));
            dirs.push(h.join(".fonts"));
        }
        dirs.push("/usr/share/fonts".into());
        dirs.push("/usr/local/share/fonts".into());
    }
    dirs
}

/// Looks for `file` in `dir` and, on Linux where fonts sit in sub folders, two levels below it.
fn find_in(dir: &std::path::Path, file: &str, depth: u8) -> Option<PathBuf> {
    let direct = dir.join(file);
    if direct.is_file() {
        return Some(direct);
    }
    if depth == 0 {
        return None;
    }
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            if let Some(p) = find_in(&e.path(), file, depth - 1) {
                return Some(p);
            }
        }
    }
    None
}

/// Fonts found on this computer, in the order of `KNOWN`. Looked up once.
pub fn installed() -> &'static [(&'static str, PathBuf)] {
    static FOUND: OnceLock<Vec<(&'static str, PathBuf)>> = OnceLock::new();
    FOUND.get_or_init(|| {
        let dirs = font_dirs();
        let depth = if cfg!(any(target_os = "macos", target_os = "windows")) { 0 } else { 2 };
        KNOWN
            .iter()
            .filter_map(|(name, files)| {
                files.iter().find_map(|f| dirs.iter().find_map(|d| find_in(d, f, depth))).map(|p| (*name, p))
            })
            .collect()
    })
}

/// The name shown for a setting value.
pub fn label(setting: &str) -> &str {
    if setting.is_empty() { BUILT_IN } else { setting }
}

/// The font terminals draw with.
pub fn term(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("term".into()))
}

static INTER: &[u8] = include_bytes!("../assets/fonts/Inter.ttf");
static DM_SANS: &[u8] = include_bytes!("../assets/fonts/DMSans.ttf");

/// The choices for the app's own font: setting value and name shown.
pub const UI_FONTS: &[(&str, &str)] = &[("inter", "Inter"), ("dmsans", "DM Sans"), ("system", "System"), ("default", "Classic")];

/// The font the operating system uses for its own interface, if it can be found.
fn system_ui_font() -> Option<Vec<u8>> {
    let files: &[&str] = if cfg!(target_os = "macos") {
        &["SFNS.ttf", "HelveticaNeue.ttc"]
    } else if cfg!(target_os = "windows") {
        &["segoeui.ttf"]
    } else {
        &["NotoSans-Regular.ttf", "Ubuntu-R.ttf", "DejaVuSans.ttf"]
    };
    let depth = if cfg!(any(target_os = "macos", target_os = "windows")) { 0 } else { 3 };
    let dirs = font_dirs();
    files.iter().find_map(|f| dirs.iter().find_map(|d| find_in(d, f, depth))).and_then(|p| std::fs::read(p).ok())
}

/// Makes the `term` family use `name` (empty, or a font that cannot be read: Hack) and the app's own text use `ui`
/// (see `UI_FONTS`). Returns false when the terminal font could not be loaded.
pub fn apply(ctx: &egui::Context, name: &str, ui: &str) -> bool {
    let mut defs = FontDefinitions::default();
    let ui_bytes: Option<Vec<u8>> = match ui {
        "default" => None,
        "dmsans" => Some(DM_SANS.to_vec()),
        "system" => system_ui_font().or_else(|| Some(INTER.to_vec())),
        _ => Some(INTER.to_vec()),
    };
    if let Some(b) = ui_bytes {
        defs.font_data.insert("ui-font".into(), Arc::new(FontData::from_owned(b)));
        defs.families.entry(FontFamily::Proportional).or_default().insert(0, "ui-font".into());
    }
    let mut family: Vec<String> = defs.families.get(&FontFamily::Monospace).cloned().unwrap_or_default();
    let mut ok = true;
    if !name.is_empty() {
        let bytes = installed().iter().find(|(n, _)| *n == name).and_then(|(_, p)| std::fs::read(p).ok());
        match bytes {
            Some(b) => {
                defs.font_data.insert("term-font".into(), Arc::new(FontData::from_owned(b)));
                family.insert(0, "term-font".into());
            }
            None => ok = false,
        }
    }
    defs.families.insert(FontFamily::Name("term".into()), family);
    ctx.set_fonts(defs);
    ok
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_empty_setting_is_the_built_in_font() {
        assert_eq!(label(""), "Hack");
        assert_eq!(label("Menlo"), "Menlo");
    }

    #[test]
    fn a_font_is_found_one_folder_down() {
        let dir = std::env::temp_dir().join(format!("sshtup-fonts-{}", std::process::id()));
        let sub = dir.join("truetype/dejavu");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("DejaVuSansMono.ttf"), b"x").unwrap();
        assert_eq!(find_in(&dir, "DejaVuSansMono.ttf", 2), Some(sub.join("DejaVuSansMono.ttf")));
        assert_eq!(find_in(&dir, "DejaVuSansMono.ttf", 1), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
