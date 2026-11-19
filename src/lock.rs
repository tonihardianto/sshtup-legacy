//! The master password: the screen that opens an encrypted vault, and the Settings section
//! that turns encryption on, changes the password or turns it off.

use crate::app::App;
use crate::model::{save_vault, vault_path, Vault};
use crate::ui::*;
use crate::model::data_dir;
use crate::vaultcrypt::{self, CryptError, LogKey, VaultKey};
use eframe::egui::{self, Align2, RichText, Vec2};
use std::sync::mpsc::{channel, Receiver};
use zeroize::Zeroizing;

pub const MIN_PASSWORD_LEN: usize = 8;

type Opened = Result<(Zeroizing<Vec<u8>>, VaultKey), CryptError>;

/// The vault on disk is encrypted and has not been opened yet.
pub struct LockState {
    bytes: Vec<u8>,
    input: String,
    error: Option<String>,
    working: Option<Receiver<Opened>>,
    /// Keep the vault key in the system keychain after a successful unlock.
    remember: bool,
    /// The keychain has been tried this run.
    auto_tried: bool,
}

impl LockState {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self { bytes, input: String::new(), error: None, working: None, remember: true, auto_tried: false }
    }
}

/// What is typed in the Settings section.
#[derive(Default)]
pub struct SecurityUi {
    current: String,
    pub(crate) new_pw: String,
    pub(crate) confirm: String,
    pub(crate) message: Option<(String, bool)>,
    /// Whether the vault key is in the system keychain (None = not looked up yet).
    remember: Option<bool>,
}

impl SecurityUi {
    fn clear(&mut self) {
        self.current.clear();
        self.new_pw.clear();
        self.confirm.clear();
    }
}

fn password_field(ui: &mut egui::Ui, text: &mut String, hint: &str) -> egui::Response {
    ui.add(egui::TextEdit::singleline(text).password(true).hint_text(hint).desired_width(260.0).margin(Vec2::new(10.0, 8.0)))
}

impl App {
    /// Shows the unlock card instead of the app while the vault is locked.
    /// Returns true while the app must stay hidden.
    pub fn lock_screen(&mut self, ui: &mut egui::Ui) -> bool {
        let ctx = ui.ctx().clone();
        let ctx = &ctx;
        let Some(lock) = self.lock.as_mut() else { return false };

        // an unlock attempt running on its own thread (Argon2id takes a moment on purpose)
        let mut outcome: Option<Opened> = None;
        let mut from_keychain = false;
        // the key kept in the system keychain opens the vault without asking
        if !lock.auto_tried && !self.demo {
            lock.auto_tried = true;
            if let Some(k) = crate::keystore::load() {
                match vaultcrypt::decrypt_with_key(&lock.bytes, &k) {
                    Ok(r) => {
                        outcome = Some(Ok(r));
                        from_keychain = true;
                    }
                    // the password was changed elsewhere: that key is stale
                    Err(_) => crate::keystore::forget(),
                }
            }
        }
        if let Some(rx) = &lock.working {
            match rx.try_recv() {
                Ok(r) => outcome = Some(r),
                Err(std::sync::mpsc::TryRecvError::Empty) => ctx.request_repaint_after(std::time::Duration::from_millis(50)),
                Err(_) => {
                    lock.working = None;
                    lock.error = Some("Could not open the vault. Try again.".into());
                }
            }
        }
        if let Some(r) = outcome {
            lock.working = None;
            match r {
                Ok((json, key)) => match serde_json::from_slice::<Vault>(&json) {
                    Ok(v) => {
                        if lock.remember && !from_keychain && !self.demo {
                            crate::keystore::save(&key.raw());
                        }
                        self.vault = v;
                        self.vault_key = Some(key);
                        self.lock = None;
                        // the history file is encrypted with the key that was inside the vault
                        self.history = crate::autocomplete::History::load(self.log_key().as_ref());
                        self.hist_tried.clear();
                        ctx.request_repaint();
                        return false;
                    }
                    Err(_) => lock.error = Some("The vault opened but its data is damaged.".into()),
                },
                Err(CryptError::WrongPassword) => lock.error = Some("Wrong master password.".into()),
                Err(_) => lock.error = Some("The vault file is damaged or not supported.".into()),
            }
            lock.input.clear();
        }

        let busy = lock.working.is_some();
        let mut submit = false;
        egui::CentralPanel::default().frame(egui::Frame::new().fill(BG_CONTENT())).show_inside(ui, |_| {});
        egui::Window::new("vault_lock")
            .title_bar(false)
            .frame(egui::Frame::window(&ctx.global_style()).inner_margin(egui::Margin::symmetric(32, 26)))
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.set_width(300.0);
                // Enter works from anywhere on this screen, whatever has the keyboard focus
                let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                ui.vertical_centered(|ui| {
                    ui.add_space(6.0);
                    ui.label(RichText::new("Vault is locked").size(17.0).strong().color(TEXT()));
                    ui.add_space(4.0);
                    ui.label(RichText::new("Enter your master password to continue.").size(12.5).color(MUTED()));
                    ui.add_space(14.0);
                    let r = ui.add_enabled(
                        !busy,
                        egui::TextEdit::singleline(&mut lock.input)
                            .password(true)
                            .hint_text("Master password")
                            .align(egui::Align2::CENTER_CENTER)
                            .desired_width(f32::INFINITY)
                            .margin(Vec2::new(10.0, 8.0)),
                    );
                    if !busy && !r.has_focus() {
                        r.request_focus();
                    }
                    if let Some(e) = &lock.error {
                        ui.add_space(6.0);
                        ui.label(RichText::new(e).size(12.0).color(RED()));
                    }
                    ui.add_space(8.0);
                    ui.add_enabled(!busy, egui::Checkbox::new(&mut lock.remember, RichText::new("Keep me unlocked on this device").size(12.0).color(MUTED())));
                    ui.add_space(8.0);
                    if busy {
                        ui.label(RichText::new("Opening…").size(12.5).color(MUTED()));
                    } else {
                        let clicked = ui
                            .add_sized([ui.available_width(), 32.0], egui::Button::new(RichText::new("Unlock").color(egui::Color32::WHITE).strong().size(12.5)).fill(BLUE()).stroke(egui::Stroke::NONE).corner_radius(6))
                            .clicked();
                        if (clicked || enter) && !lock.input.is_empty() {
                            submit = true;
                        }
                    }
                    ui.add_space(6.0);
                });
            });
        if submit {
            let (tx, rx) = channel();
            let bytes = lock.bytes.clone();
            let pw = Zeroizing::new(std::mem::take(&mut lock.input));
            lock.error = None;
            lock.working = Some(rx);
            let ctx2 = ctx.clone();
            std::thread::spawn(move || {
                let _ = tx.send(vaultcrypt::decrypt(&bytes, &pw));
                ctx2.request_repaint();
            });
        }
        true
    }

    /// Does `pw` open the vault file that is on disk right now?
    fn current_password_ok(&self, pw: &str) -> bool {
        std::fs::read(vault_path()).map(|b| vaultcrypt::decrypt(&b, pw).is_ok()).unwrap_or(false)
    }

    pub fn settings_security(&mut self, ui: &mut egui::Ui) {
        let encrypted = self.vault_key.is_some();
        section_title(ui, "SECURITY");
        let mut enable = false;
        let mut change = false;
        let mut remove = false;
        let su = &mut self.sec;
        if self.demo {
            ui.label(RichText::new("Not available in a demo run.").size(12.0).color(MUTED()));
            return;
        }
        if !encrypted {
            ui.label(RichText::new("Hosts, saved passwords, key passphrases and snippets are stored unencrypted in vault.json, session logs and the command history are stored as plain text. A master password encrypts the vault, the logs and the history (Argon2id + XChaCha20-Poly1305).").size(12.0).color(MUTED()));
            ui.add_space(6.0);
            password_field(ui, &mut su.new_pw, "New master password");
            password_field(ui, &mut su.confirm, "Repeat the password");
            ui.add_space(4.0);
            ui.label(RichText::new("There is no way to recover the vault if you forget this password.").size(12.0).color(AMBER()));
            ui.add_space(6.0);
            if primary_button(ui, "Encrypt the vault").clicked() {
                enable = true;
            }
        } else {
            ui.label(RichText::new("The vault, the session logs and the command history are encrypted with your master password.").size(12.0).color(GREEN()));
            ui.add_space(6.0);
            let mut remember = *su.remember.get_or_insert_with(|| crate::keystore::load().is_some());
            if ui.checkbox(&mut remember, RichText::new("Unlock automatically on this device").size(12.5)).changed() {
                let ok = if remember { self.vault_key.as_ref().map(|k| crate::keystore::save(&k.raw())).unwrap_or(false) } else {
                    crate::keystore::forget();
                    true
                };
                su.remember = Some(if ok { remember } else { false });
                su.message = Some(if !ok {
                    ("Could not use the system keychain.".into(), false)
                } else if remember {
                    ("The key is kept in the system keychain. The app opens without the master password on this device.".into(), true)
                } else {
                    ("The master password is asked for every time the app opens.".into(), true)
                });
            }
            ui.label(RichText::new("The key is kept in the system keychain, guarded by your computer login.").size(11.5).color(MUTED()));
            ui.add_space(8.0);
            password_field(ui, &mut su.current, "Current password");
            password_field(ui, &mut su.new_pw, "New password (to change it)");
            password_field(ui, &mut su.confirm, "Repeat the new password");
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if primary_button(ui, "Change password").clicked() {
                    change = true;
                }
                if danger_button(ui, "Remove encryption").clicked() {
                    remove = true;
                }
            });
        }
        if let Some((m, ok)) = &su.message {
            ui.add_space(6.0);
            ui.label(RichText::new(m).size(12.0).color(if *ok { GREEN() } else { RED() }));
        }

        if enable || change {
            self.set_master_password(encrypted);
        }
        if remove {
            if !self.current_password_ok(&self.sec.current) {
                self.sec.message = Some(("Type the current password to remove encryption.".into(), false));
            } else if self.tabs.iter().any(|t| t.sess.spec.log_key.is_some()) || self.forwards_live.values().any(|s| s.spec.log_key.is_some()) {
                // their logs are encrypted with the key that is about to go away
                self.sec.message = Some(("Close the open terminals first, then remove encryption.".into(), false));
            } else {
                // logs first: they need the key that the vault is about to drop
                let key = self.log_key();
                let (_, failed) = vaultcrypt::convert_logs(&data_dir().join("logs"), None, key.as_ref(), &[]);
                self.history.set_key(None);
                let history_ok = self.history.save();
                if failed > 0 || !history_ok {
                    // keep everything encrypted: put the history back under the key if it was not rewritten
                    if !history_ok {
                        self.history.set_key(key.clone());
                        self.history.save();
                    }
                    self.sec.message = Some((if failed > 0 { format!("{failed} encrypted log(s) could not be decrypted, so encryption was kept. Nothing was lost.") } else { "The command history could not be rewritten, so encryption was kept. Nothing was lost.".to_string() }, false));
                } else {
                    let old = self.vault.log_key.take();
                    if save_vault(&self.vault, None) {
                        self.vault_key = None;
                        crate::keystore::forget();
                        self.sec.remember = None;
                        self.sec.clear();
                        self.sec.message = Some(("Encryption removed. The vault, the logs and the command history are stored as plain text again.".into(), true));
                    } else {
                        self.vault.log_key = old;
                        // the vault is still encrypted: the history goes back under its key
                        self.history.set_key(key);
                        self.history.save();
                        self.sec.message = Some(("Could not write the vault. Nothing was changed.".into(), false));
                    }
                }
            }
        }
    }

    /// Encrypts the vault with the password typed in `self.sec` (or changes the password when `encrypted`).
    /// The result is left in `self.sec.message`.
    pub fn set_master_password(&mut self, encrypted: bool) {
            let err = if encrypted && !self.current_password_ok(&self.sec.current) {
                Some("The current password is not correct.")
            } else if self.sec.new_pw.chars().count() < MIN_PASSWORD_LEN {
                Some("Use at least 8 characters.")
            } else if self.sec.new_pw != self.sec.confirm {
                Some("The two passwords do not match.")
            } else {
                None
            };
            match err {
                Some(e) => self.sec.message = Some((e.into(), false)),
                None => match VaultKey::new(&self.sec.new_pw) {
                    Ok(key) => {
                        // the log key is made once and lives inside the encrypted vault
                        let old_log_key = self.vault.log_key.clone();
                        let mut made_log_key = None;
                        if self.vault.log_key.is_none() {
                            if let Ok(k) = LogKey::random() {
                                self.vault.log_key = Some(k.to_hex().to_string());
                                made_log_key = Some(k);
                            }
                        }
                        if self.vault.log_key.is_some() && save_vault(&self.vault, Some(&key)) {
                            if self.sec.remember == Some(true) || (encrypted && crate::keystore::load().is_some()) {
                                crate::keystore::save(&key.raw());
                            }
                            self.vault_key = Some(key);
                            self.sec.clear();
                            let mut msg = if encrypted { "Password changed.".to_string() } else { "The vault is now encrypted.".to_string() };
                            if let (false, Some(k)) = (encrypted, made_log_key) {
                                self.history.set_key(Some(k.clone()));
                                if self.history.save() {
                                    msg.push_str(" Command history encrypted.");
                                }
                                let skip = self.open_log_paths();
                                let (done, failed) = vaultcrypt::convert_logs(&data_dir().join("logs"), Some(&k), None, &skip);
                                msg.push_str(&format!(" {done} existing log{} encrypted.", if done == 1 { "" } else { "s" }));
                                if failed > 0 {
                                    msg.push_str(&format!(" {failed} could not be converted."));
                                }
                                if !skip.is_empty() {
                                    msg.push_str(" Terminals that are open keep a plain log until they reconnect.");
                                }
                            }
                            self.sec.message = Some((msg, true));
                        } else {
                            self.vault.log_key = old_log_key;
                            self.sec.message = Some(("Could not write the encrypted vault. Nothing was changed.".into(), false));
                        }
                    }
                    Err(_) => self.sec.message = Some(("Could not write the encrypted vault. Nothing was changed.".into(), false)),
                },
            }
        }

    /// Log files of the sessions that are open right now (they are still being written).
    fn open_log_paths(&self) -> Vec<std::path::PathBuf> {
        self.tabs.iter().filter_map(|t| t.sess.log_path.clone()).chain(self.forwards_live.values().filter_map(|s| s.log_path.clone())).collect()
    }
}
