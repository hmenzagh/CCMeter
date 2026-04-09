use std::sync::mpsc;
use std::thread;

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const RELEASES_URL: &str = "https://api.github.com/repos/hmenzagh/CCMeter/releases/latest";

/// The result of a background version check.
pub(crate) struct UpdateInfo {
    pub(crate) latest_version: String,
}

/// Spawns a background thread that queries GitHub for the latest release.
/// Returns a receiver that will eventually contain `Some(UpdateInfo)` if a
/// newer version is available, or nothing if the check fails / version is
/// current.
pub(crate) fn spawn_check() -> mpsc::Receiver<UpdateInfo> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        if let Some(info) = check_latest() {
            let _ = tx.send(info);
        }
    });
    rx
}

fn check_latest() -> Option<UpdateInfo> {
    let resp = ureq::get(RELEASES_URL)
        .header("Accept", "application/vnd.github.v3+json")
        .header("User-Agent", concat!("ccmeter/", env!("CARGO_PKG_VERSION")))
        .call()
        .ok()?;

    let body: String = resp.into_body().read_to_string().ok()?;
    let json: serde_json::Value = serde_json::from_str(&body).ok()?;
    let tag = json.get("tag_name")?.as_str()?;
    let latest = tag.trim_start_matches('v');

    if version_newer(latest, CURRENT_VERSION) {
        Some(UpdateInfo {
            latest_version: latest.to_string(),
        })
    } else {
        None
    }
}

/// Returns true if `latest` is strictly newer than `current` (simple semver).
fn version_newer(latest: &str, current: &str) -> bool {
    let parse = |s: &str| -> (u32, u32, u32) {
        let mut parts = s.splitn(3, '.');
        let major = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
        let minor = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
        let patch = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
        (major, minor, patch)
    };
    parse(latest) > parse(current)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_newer() {
        assert!(version_newer("1.5.0", "1.4.0"));
        assert!(version_newer("2.0.0", "1.9.9"));
        assert!(!version_newer("1.4.0", "1.4.0"));
        assert!(!version_newer("1.3.0", "1.4.0"));
    }
}
