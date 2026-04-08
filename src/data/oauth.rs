use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde::Deserialize;

// ---------------------------------------------------------------------------
// Usage report from /api/oauth/usage
// ---------------------------------------------------------------------------

/// A single usage window (5h or 7d).
#[derive(Debug, Clone, Deserialize)]
pub struct UsageWindow {
    pub utilization: f64,
    #[serde(default)]
    pub resets_at: Option<String>,
}

/// Extra usage / overages info.
#[derive(Debug, Clone, Deserialize)]
pub struct ExtraUsage {
    pub is_enabled: bool,
    pub monthly_limit: Option<f64>,
    pub used_credits: Option<f64>,
    pub utilization: Option<f64>,
}

/// Response from `GET /api/oauth/usage`.
#[derive(Debug, Clone, Deserialize)]
pub struct UsageReport {
    pub five_hour: Option<UsageWindow>,
    pub seven_day: Option<UsageWindow>,
    pub seven_day_opus: Option<UsageWindow>,
    pub seven_day_sonnet: Option<UsageWindow>,
    pub seven_day_cowork: Option<UsageWindow>,
    pub extra_usage: Option<ExtraUsage>,
}

/// Debug / display stats for usage polling.
#[derive(Debug, Clone)]
pub struct UsageStats {
    /// Total successful API calls.
    pub call_count: u32,
    /// Total 429 responses.
    pub rate_limit_count: u32,
    /// When the last successful fetch happened.
    pub last_fetch: Option<Instant>,
}

impl Default for UsageStats {
    fn default() -> Self {
        Self {
            call_count: 0,
            rate_limit_count: 0,
            last_fetch: None,
        }
    }
}

impl UsageStats {
    /// Human-readable "time since last fetch".
    pub fn last_fetch_ago(&self) -> String {
        match self.last_fetch {
            Some(t) => {
                let secs = t.elapsed().as_secs();
                if secs < 60 {
                    format!("{}s ago", secs)
                } else {
                    format!("{}m{}s ago", secs / 60, secs % 60)
                }
            }
            None => "never".to_string(),
        }
    }
}

/// OAuth credential info for a single Claude source, with async usage polling.
#[derive(Debug, Clone)]
pub struct OAuthCredential {
    /// Which source root this credential belongs to (e.g. `~/.claude/projects`).
    pub source_root: PathBuf,
    pub subscription_type: Option<String>,
    pub rate_limit_tier: Option<String>,
    pub expires_at: Option<u64>,
    /// The access token (kept for API calls).
    pub access_token: Option<String>,
    /// Latest usage report.
    pub usage: Option<UsageReport>,
    /// Polling stats.
    pub stats: UsageStats,
}

impl OAuthCredential {
    pub fn is_expired(&self) -> bool {
        match self.expires_at {
            Some(exp) => {
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);
                exp < now_ms
            }
            None => true,
        }
    }
}

// ---------------------------------------------------------------------------
// Usage polling — each credential has its own random interval
// ---------------------------------------------------------------------------

/// Result sent back from a background usage fetch thread.
pub struct UsageFetchResult {
    /// Index into the credentials vec.
    pub index: usize,
    pub usage: Option<UsageReport>,
    /// Whether we got a 429.
    pub was_rate_limited: bool,
}

/// Per-credential polling state (not cloned into render — lives in App).
pub struct UsagePoller {
    /// One receiver per credential for async results.
    entries: Vec<PollerEntry>,
    tx: mpsc::Sender<UsageFetchResult>,
    rx: mpsc::Receiver<UsageFetchResult>,
}

struct PollerEntry {
    next_fetch: Instant,
    in_flight: bool,
    token: Option<String>,
    expired: bool,
}

/// Random duration between 5 and 10 minutes.
fn random_interval() -> Duration {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    let secs = 300 + (nanos % 301); // 300..600s = 5..10 min
    Duration::from_secs(secs as u64)
}

impl UsagePoller {
    pub fn new(credentials: &[OAuthCredential]) -> Self {
        let (tx, rx) = mpsc::channel();
        let entries = credentials
            .iter()
            .map(|c| PollerEntry {
                // Already fetched at startup — next refresh in 5-10 min
                next_fetch: Instant::now() + random_interval(),
                in_flight: false,
                token: c.access_token.clone(),
                expired: c.is_expired(),
            })
            .collect();
        Self { entries, tx, rx }
    }

    /// Call this in the event loop. Spawns background fetches when timers expire,
    /// and applies results back to the credentials vec.
    /// Returns `true` if any credential's usage was updated this tick.
    pub fn poll(&mut self, credentials: &mut [OAuthCredential]) -> bool {
        let now = Instant::now();

        // Spawn fetches for credentials whose timer has expired
        for (i, entry) in self.entries.iter_mut().enumerate() {
            if entry.in_flight || entry.token.is_none() || entry.expired {
                continue;
            }
            if now >= entry.next_fetch {
                entry.in_flight = true;
                let tx = self.tx.clone();
                let token = entry.token.clone().unwrap();
                std::thread::spawn(move || {
                    let (usage, was_rate_limited) = fetch_usage_raw(&token);
                    let _ = tx.send(UsageFetchResult {
                        index: i,
                        usage,
                        was_rate_limited,
                    });
                });
            }
        }

        // Collect results
        let mut updated = false;
        while let Ok(result) = self.rx.try_recv() {
            let i = result.index;
            if i >= credentials.len() {
                continue;
            }

            let entry = &mut self.entries[i];
            entry.in_flight = false;
            // Schedule next fetch with a new random interval
            entry.next_fetch = Instant::now() + random_interval();

            let stats = &mut credentials[i].stats;
            stats.call_count += 1;
            if result.was_rate_limited {
                stats.rate_limit_count += 1;
            }
            if result.usage.is_some() {
                stats.last_fetch = Some(Instant::now());
                credentials[i].usage = result.usage;
                updated = true;
            }
        }
        updated
    }
}

/// Raw fetch that distinguishes 429 from other errors.
fn fetch_usage_raw(token: &str) -> (Option<UsageReport>, bool) {
    let result = ureq::get("https://api.anthropic.com/api/oauth/usage")
        .header("Authorization", &format!("Bearer {}", token))
        .header("anthropic-beta", "oauth-2025-04-20")
        .call();

    match result {
        Ok(mut resp) => {
            let body = resp.body_mut().read_to_string().unwrap_or_default();
            let usage = serde_json::from_str(&body).ok();
            (usage, false)
        }
        Err(e) => {
            let is_429 = e
                .to_string()
                .contains("429");
            (None, is_429)
        }
    }
}

// ---------------------------------------------------------------------------
// Credential discovery (unchanged logic)
// ---------------------------------------------------------------------------

/// Discover credentials and immediately fetch usage for each (blocking, for startup).
pub fn discover_credentials_with_usage(source_roots: &[PathBuf]) -> Vec<OAuthCredential> {
    use rayon::prelude::*;
    let mut creds = discover_credentials(source_roots);
    creds.par_iter_mut().for_each(|cred| {
        if cred.access_token.is_some() && !cred.is_expired() {
            let (usage, _) = fetch_usage_raw(
                cred.access_token.as_deref().unwrap(),
            );
            if usage.is_some() {
                cred.stats.last_fetch = Some(Instant::now());
                cred.stats.call_count += 1;
            }
            cred.usage = usage;
        }
    });
    creds
}

/// Discover OAuth credentials for all known source roots.
pub fn discover_credentials(source_roots: &[PathBuf]) -> Vec<OAuthCredential> {
    let mut seen_parents = std::collections::HashSet::new();
    let mut credentials = Vec::new();

    for root in source_roots {
        let Some(parent) = root.parent() else {
            continue;
        };
        if !seen_parents.insert(parent.to_path_buf()) {
            continue;
        }

        if let Some(cred) = try_credentials_file(parent, root) {
            credentials.push(cred);
            continue;
        }

        #[cfg(target_os = "macos")]
        if let Some(cred) = try_keychain(parent, root) {
            credentials.push(cred);
        }
    }

    credentials
}

// ---------------------------------------------------------------------------
// Serde helpers
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct CredentialsFile {
    #[serde(rename = "claudeAiOauth")]
    claude_ai_oauth: Option<OAuthEntry>,
}

#[derive(Deserialize)]
struct OAuthEntry {
    #[serde(rename = "accessToken")]
    access_token: Option<String>,
    #[serde(rename = "expiresAt")]
    expires_at: Option<u64>,
    #[serde(rename = "subscriptionType")]
    subscription_type: Option<String>,
    #[serde(rename = "rateLimitTier")]
    rate_limit_tier: Option<String>,
}

fn new_credential(oauth: OAuthEntry, source_root: &PathBuf) -> OAuthCredential {
    OAuthCredential {
        source_root: source_root.clone(),
        subscription_type: oauth.subscription_type,
        rate_limit_tier: oauth.rate_limit_tier,
        expires_at: oauth.expires_at,
        access_token: oauth.access_token,
        usage: None,
        stats: UsageStats::default(),
    }
}

fn try_credentials_file(parent: &Path, source_root: &PathBuf) -> Option<OAuthCredential> {
    let cred_path = parent.join(".credentials.json");
    let content = std::fs::read_to_string(&cred_path).ok()?;
    let parsed: CredentialsFile = serde_json::from_str(&content).ok()?;
    Some(new_credential(parsed.claude_ai_oauth?, source_root))
}

#[cfg(target_os = "macos")]
fn try_keychain(parent: &Path, source_root: &PathBuf) -> Option<OAuthCredential> {
    let service = keychain_service_name(parent);

    let output = std::process::Command::new("security")
        .args(["find-generic-password", "-s", &service, "-w"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let json_str = String::from_utf8(output.stdout).ok()?;
    let parsed: CredentialsFile = serde_json::from_str(json_str.trim()).ok()?;
    Some(new_credential(parsed.claude_ai_oauth?, source_root))
}

#[cfg(target_os = "macos")]
fn keychain_service_name(parent: &Path) -> String {
    let parent_str = parent.to_string_lossy();

    if parent_str.ends_with("/.claude") || parent_str.ends_with("/claude") {
        return "Claude Code-credentials".to_string();
    }

    let hash = sha256_prefix(parent_str.as_ref());
    format!("Claude Code-credentials-{}", hash)
}

#[cfg(target_os = "macos")]
fn sha256_prefix(input: &str) -> String {
    let output = std::process::Command::new("shasum")
        .args(["-a", "256"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            if let Some(ref mut stdin) = child.stdin {
                let _ = stdin.write_all(input.as_bytes());
            }
            child.wait_with_output()
        });

    match output {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .split_whitespace()
            .next()
            .map(|h| h[..8].to_string())
            .unwrap_or_default(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn parses_credentials_file() {
        let tmp = std::env::temp_dir().join(format!("ccmeter_oauth_test_{}", std::process::id()));
        let projects = tmp.join("projects");
        std::fs::create_dir_all(&projects).unwrap();

        let cred_path = tmp.join(".credentials.json");
        let mut f = std::fs::File::create(&cred_path).unwrap();
        write!(
            f,
            r#"{{
                "claudeAiOauth": {{
                    "accessToken": "sk-ant-oat01-test",
                    "refreshToken": "sk-ant-ort01-test",
                    "expiresAt": 9999999999999,
                    "scopes": ["user:inference"],
                    "subscriptionType": "max",
                    "rateLimitTier": "default_claude_max_5x"
                }}
            }}"#
        )
        .unwrap();

        let creds = discover_credentials(&[projects.clone()]);
        assert_eq!(creds.len(), 1);
        assert_eq!(creds[0].subscription_type.as_deref(), Some("max"));
        assert_eq!(
            creds[0].rate_limit_tier.as_deref(),
            Some("default_claude_max_5x")
        );
        assert!(!creds[0].is_expired());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn detects_expired_token() {
        let cred = OAuthCredential {
            source_root: PathBuf::from("/tmp"),
            subscription_type: None,
            rate_limit_tier: None,
            expires_at: Some(1000),
            access_token: None,
            usage: None,
            stats: UsageStats::default(),
        };
        assert!(cred.is_expired());
    }
}
