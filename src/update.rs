//! "Check for updates": asks GitHub for the latest release and compares its version with this build's.
//! Only runs when the user presses the button; nothing is sent besides a plain GET.

use std::sync::{Arc, Mutex};
use std::time::Duration;

const API_URL: &str = "https://api.github.com/repos/tonihardianto/sshtup/releases/latest";

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Idle,
    Checking,
    UpToDate,
    Available { version: String, url: String },
    Failed(String),
}

#[derive(Clone)]
pub struct Checker {
    state: Arc<Mutex<State>>,
}

impl Default for Checker {
    fn default() -> Self {
        Self { state: Arc::new(Mutex::new(State::Idle)) }
    }
}

impl Checker {
    pub fn state(&self) -> State {
        self.state.lock().map(|s| s.clone()).unwrap_or(State::Idle)
    }

    /// Starts a check in the background; does nothing if one is already running.
    pub fn start(&self, ctx: &eframe::egui::Context) {
        {
            let Ok(mut s) = self.state.lock() else { return };
            if *s == State::Checking {
                return;
            }
            *s = State::Checking;
        }
        let state = self.state.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = fetch_latest().map(|(tag, url)| {
                if is_newer(&tag, env!("CARGO_PKG_VERSION")) {
                    State::Available { version: tag.trim_start_matches('v').to_string(), url }
                } else {
                    State::UpToDate
                }
            });
            if let Ok(mut s) = state.lock() {
                *s = result.unwrap_or_else(State::Failed);
            }
            ctx.request_repaint();
        });
    }
}

fn fetch_latest() -> Result<(String, String), String> {
    let agent = ureq::Agent::new_with_config(ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(10))).http_status_as_error(false).build());
    let mut resp = agent
        .get(API_URL)
        .header("User-Agent", "sshtup-update-check")
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|_| "Could not reach GitHub. Check your connection.".to_string())?;
    if resp.status().as_u16() == 404 {
        return Err("No release has been published yet.".into());
    }
    if !resp.status().is_success() {
        return Err(format!("GitHub answered with status {}.", resp.status().as_u16()));
    }
    let body = resp.body_mut().read_to_string().map_err(|_| "The answer from GitHub was unreadable.".to_string())?;
    let v: serde_json::Value = serde_json::from_str(&body).map_err(|_| "The answer from GitHub was unreadable.".to_string())?;
    let tag = v.get("tag_name").and_then(|t| t.as_str()).ok_or("The release has no version tag.")?.to_string();
    let url = v.get("html_url").and_then(|t| t.as_str()).unwrap_or("https://github.com/tonihardianto/sshtup/releases/latest").to_string();
    Ok((tag, url))
}

/// `1.2.3`, `v1.2.3` or `1.2.3-beta` as numbers; missing parts count as 0.
fn parse(v: &str) -> Option<[u64; 3]> {
    let core = v.trim().trim_start_matches(['v', 'V']).split(['-', '+']).next()?;
    let mut out = [0u64; 3];
    for (i, part) in core.split('.').enumerate() {
        if i >= 3 {
            break;
        }
        out[i] = part.parse().ok()?;
    }
    Some(out)
}

/// Whether `latest` is a higher version than `current`. An unreadable tag never counts as newer.
pub fn is_newer(latest: &str, current: &str) -> bool {
    match (parse(latest), parse(current)) {
        (Some(l), Some(c)) => l > c,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_higher_version_is_newer() {
        assert!(is_newer("v0.3.0", "0.2.1"));
        assert!(is_newer("0.2.2", "0.2.1"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(is_newer("v0.10.0", "0.9.0"));
    }

    #[test]
    fn the_same_or_a_lower_version_is_not() {
        assert!(!is_newer("v0.2.1", "0.2.1"));
        assert!(!is_newer("0.2.0", "0.2.1"));
        assert!(!is_newer("0.1.9", "0.2.0"));
    }

    #[test]
    fn short_and_suffixed_tags_are_read() {
        assert!(is_newer("v1.1", "1.0.5"));
        assert!(is_newer("1.2.0-beta.1", "1.1.0"));
        assert!(!is_newer("nightly", "0.2.1"));
    }
}
