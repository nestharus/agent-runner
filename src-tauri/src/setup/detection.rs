use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

#[derive(Clone, Serialize)]
pub struct DetectionReport {
    pub clis: Vec<CliInfo>,
    pub os: OsInfo,
    pub wrappers: Vec<WrapperInfo>,
}

#[derive(Clone, Serialize)]
pub struct CliInfo {
    pub name: String,
    pub installed: bool,
    pub path: Option<String>,
    pub version: Option<String>,
    pub authenticated: bool,
    pub config_dir: Option<PathBuf>,
    /// Profiles / accounts discovered for this CLI.
    pub profiles: Vec<CliProfile>,
    /// Set when a VersionTracker is used and the version differs from what was
    /// previously stored. `None` means no prior version was recorded.
    pub version_changed: Option<bool>,
    /// The previously-stored version, if any.
    pub previous_version: Option<String>,
}

/// A single account / profile discovered for a CLI.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CliProfile {
    /// Human-readable identifier (email, provider name, profile name, ...).
    pub id: String,
    /// What kind of credential backs this profile.
    pub auth_method: String,
    /// Whether this is the currently-active profile.
    pub active: bool,
    /// Extra metadata (plan type, org name, etc.) — serialised JSON.
    pub details: Option<String>,
}

#[derive(Clone, Serialize)]
pub struct OsInfo {
    pub os_type: String,
    pub arch: String,
}

#[derive(Clone, Serialize)]
pub struct WrapperInfo {
    pub name: String,
    pub path: PathBuf,
    pub target_cli: Option<String>,
}

// ---------------------------------------------------------------------------
// Version tracking (SQLite)
// ---------------------------------------------------------------------------

/// Persistent store for CLI version history.  Lives in its own table inside
/// the same database used by MemoryGraph / StateDb — the caller passes the
/// path.
pub struct VersionTracker {
    conn: Connection,
}

/// A single row from the `cli_versions` table.
#[derive(Clone, Debug, Serialize)]
pub struct VersionRecord {
    pub cli_name: String,
    pub version: String,
    pub path: Option<String>,
    pub detected_at: String,
}

impl VersionTracker {
    /// Open (or create) the version-tracking table inside the given SQLite
    /// database file.
    pub fn open(db_path: &Path) -> Result<Self, String> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create directory for version DB: {e}"))?;
        }
        let conn = Connection::open(db_path)
            .map_err(|e| format!("Failed to open version DB: {e}"))?;

        conn.execute_batch("PRAGMA journal_mode=WAL;")
            .map_err(|e| format!("Failed to set WAL mode: {e}"))?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS cli_versions (
                cli_name    TEXT PRIMARY KEY,
                version     TEXT NOT NULL,
                path        TEXT,
                detected_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS cli_version_history (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                cli_name    TEXT NOT NULL,
                version     TEXT NOT NULL,
                path        TEXT,
                detected_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_cli_version_history_name
                ON cli_version_history (cli_name, detected_at);",
        )
        .map_err(|e| format!("Failed to create cli_versions tables: {e}"))?;

        Ok(VersionTracker { conn })
    }

    /// Return the most-recently stored version for `cli_name`, if any.
    pub fn get_current(&self, cli_name: &str) -> Result<Option<VersionRecord>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT cli_name, version, path, detected_at
                 FROM cli_versions WHERE cli_name = ?1",
            )
            .map_err(|e| format!("Failed to prepare query: {e}"))?;

        let result = stmt.query_row(params![cli_name], |row| {
            Ok(VersionRecord {
                cli_name: row.get(0)?,
                version: row.get(1)?,
                path: row.get(2)?,
                detected_at: row.get(3)?,
            })
        });

        match result {
            Ok(rec) => Ok(Some(rec)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(format!("Failed to query cli_versions: {e}")),
        }
    }

    /// Upsert the current version.  If the version changed, a row is also
    /// appended to `cli_version_history` so the full timeline is preserved.
    /// Returns `true` when the version differs from what was stored (i.e. a
    /// change was detected).  Returns `false` on first record or same version.
    pub fn record(
        &self,
        cli_name: &str,
        version: &str,
        path: Option<&str>,
    ) -> Result<bool, String> {
        let now = chrono::Utc::now().to_rfc3339();
        let prev = self.get_current(cli_name)?;

        let changed = prev
            .as_ref()
            .map(|r| r.version != version)
            .unwrap_or(false);

        self.conn
            .execute(
                "INSERT INTO cli_versions (cli_name, version, path, detected_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (cli_name) DO UPDATE SET
                    version = ?2, path = ?3, detected_at = ?4",
                params![cli_name, version, path, &now],
            )
            .map_err(|e| format!("Failed to upsert cli_versions: {e}"))?;

        // Always append to history so we have a timeline.
        self.conn
            .execute(
                "INSERT INTO cli_version_history (cli_name, version, path, detected_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![cli_name, version, path, &now],
            )
            .map_err(|e| format!("Failed to insert cli_version_history: {e}"))?;

        Ok(changed)
    }

    /// Return the full version history for `cli_name`, newest first.
    pub fn history(&self, cli_name: &str, limit: u32) -> Result<Vec<VersionRecord>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT cli_name, version, path, detected_at
                 FROM cli_version_history
                 WHERE cli_name = ?1
                 ORDER BY detected_at DESC
                 LIMIT ?2",
            )
            .map_err(|e| format!("Failed to prepare history query: {e}"))?;

        let rows = stmt
            .query_map(params![cli_name, limit], |row| {
                Ok(VersionRecord {
                    cli_name: row.get(0)?,
                    version: row.get(1)?,
                    path: row.get(2)?,
                    detected_at: row.get(3)?,
                })
            })
            .map_err(|e| format!("Failed to query history: {e}"))?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to collect history rows: {e}"))
    }
}

// ---------------------------------------------------------------------------
// Known CLIs
// ---------------------------------------------------------------------------

const KNOWN_CLIS: &[(&str, &[&str])] = &[
    ("claude", &[".claude"]),
    ("codex", &[".codex"]),
    ("opencode", &[".opencode"]),
    ("gemini", &[".gemini"]),
];

// ---------------------------------------------------------------------------
// Public detection API
// ---------------------------------------------------------------------------

/// Run full detection for all known CLIs.  When `tracker` is `Some`, version
/// changes are detected and recorded.
pub fn detect_all() -> DetectionReport {
    detect_all_with_tracker(None)
}

/// Run full detection with an optional `VersionTracker` for persistent version
/// change monitoring.
pub fn detect_all_with_tracker(tracker: Option<&VersionTracker>) -> DetectionReport {
    let clis: Vec<CliInfo> = KNOWN_CLIS
        .iter()
        .map(|(name, config_dirs)| detect_cli(name, config_dirs, tracker))
        .collect();

    let wrappers = scan_wrappers();

    DetectionReport {
        clis,
        os: detect_os(),
        wrappers,
    }
}

pub fn detect_single_cli(name: &str) -> CliInfo {
    detect_single_cli_with_tracker(name, None)
}

pub fn detect_single_cli_with_tracker(name: &str, tracker: Option<&VersionTracker>) -> CliInfo {
    let config_dirs: &[&str] = KNOWN_CLIS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, dirs)| *dirs)
        .unwrap_or(&[]);
    detect_cli(name, config_dirs, tracker)
}

pub fn detect_os_public() -> OsInfo {
    detect_os()
}

// ---------------------------------------------------------------------------
// Core detection
// ---------------------------------------------------------------------------

fn detect_cli(name: &str, config_dirs: &[&str], tracker: Option<&VersionTracker>) -> CliInfo {
    let which_result = Command::new("which").arg(name).output();

    let (installed, path) = match which_result {
        Ok(output) if output.status.success() => {
            let p = String::from_utf8_lossy(&output.stdout).trim().to_string();
            (true, Some(p))
        }
        _ => (false, None),
    };

    let version = if installed {
        get_version(name)
    } else {
        None
    };

    let home = dirs::home_dir().unwrap_or_default();
    let config_dir = config_dirs
        .iter()
        .map(|d| home.join(d))
        .find(|p| p.exists());

    let authenticated = if installed {
        check_auth(name)
    } else {
        false
    };

    let profiles = if installed {
        enumerate_profiles(name)
    } else {
        vec![]
    };

    // Version tracking
    let (version_changed, previous_version) = match (&version, tracker) {
        (Some(ver), Some(t)) => {
            let prev = t.get_current(name).ok().flatten();
            let prev_ver = prev.map(|r| r.version);
            let changed = t
                .record(name, ver, path.as_deref())
                .unwrap_or(false);
            (Some(changed), prev_ver)
        }
        _ => (None, None),
    };

    CliInfo {
        name: name.to_string(),
        installed,
        path,
        version,
        authenticated,
        config_dir,
        profiles,
        version_changed,
        previous_version,
    }
}

fn get_version(cli: &str) -> Option<String> {
    let output = Command::new(cli)
        .arg("--version")
        .output()
        .ok()?;
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Auth checking
// ---------------------------------------------------------------------------

fn check_auth(cli: &str) -> bool {
    let home = dirs::home_dir().unwrap_or_default();
    match cli {
        "claude" => {
            // Claude stores auth in ~/.claude/.credentials.json or credentials.json
            home.join(".claude").join(".credentials.json").exists()
                || home.join(".claude").join("credentials.json").exists()
        }
        "codex" => {
            // Codex uses environment variable or OAuth tokens in auth.json
            std::env::var("OPENAI_API_KEY").is_ok()
                || home.join(".codex").join("auth.json").exists()
        }
        "gemini" => {
            // Gemini stores OAuth creds in ~/.gemini/oauth_creds.json
            home.join(".gemini").join("oauth_creds.json").exists()
        }
        "opencode" => {
            // OpenCode stores auth in ~/.local/share/opencode/auth.json
            let data_dir = dirs::data_dir().unwrap_or_default();
            data_dir.join("opencode").join("auth.json").exists()
        }
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Profile / account enumeration
// ---------------------------------------------------------------------------

fn enumerate_profiles(cli: &str) -> Vec<CliProfile> {
    match cli {
        "claude" => enumerate_claude_profiles(),
        "codex" => enumerate_codex_profiles(),
        "gemini" => enumerate_gemini_profiles(),
        "opencode" => enumerate_opencode_profiles(),
        _ => vec![],
    }
}

/// Claude: `claude auth status` returns JSON with email, authMethod,
/// subscriptionType, etc.
fn enumerate_claude_profiles() -> Vec<CliProfile> {
    let output = match Command::new("claude")
        .args(["auth", "status"])
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return vec![],
    };

    let text = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = match serde_json::from_str(text.trim()) {
        Ok(v) => v,
        Err(_) => return vec![],
    };

    let logged_in = parsed.get("loggedIn").and_then(|v| v.as_bool()).unwrap_or(false);
    if !logged_in {
        return vec![];
    }

    let email = parsed
        .get("email")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    let auth_method = parsed
        .get("authMethod")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    let details = serde_json::json!({
        "subscriptionType": parsed.get("subscriptionType").and_then(|v| v.as_str()),
        "apiProvider": parsed.get("apiProvider").and_then(|v| v.as_str()),
        "orgId": parsed.get("orgId").and_then(|v| v.as_str()),
        "orgName": parsed.get("orgName").and_then(|v| v.as_str()),
    });

    vec![CliProfile {
        id: email,
        auth_method,
        active: true,
        details: Some(details.to_string()),
    }]
}

/// Codex: `codex login status` returns a single line like "Logged in using
/// ChatGPT".  Profile names come from `[profile.*]` sections in
/// `~/.codex/config.toml` (selected via `codex -p <name>`).
fn enumerate_codex_profiles() -> Vec<CliProfile> {
    let mut profiles = Vec::new();

    // 1. Active login
    let output = Command::new("codex")
        .args(["login", "status"])
        .output();

    if let Ok(o) = output {
        let text = String::from_utf8_lossy(&o.stdout).trim().to_string();
        let logged_in = o.status.success() && text.to_lowercase().contains("logged in");
        if logged_in {
            // Try to extract email from auth.json for a richer ID
            let id = read_codex_email().unwrap_or_else(|| text.clone());
            profiles.push(CliProfile {
                id,
                auth_method: extract_codex_auth_method(&text),
                active: true,
                details: None,
            });
        }
    }

    // 2. Named profiles from config.toml
    if let Some(home) = dirs::home_dir() {
        let config_path = home.join(".codex").join("config.toml");
        if let Ok(content) = std::fs::read_to_string(&config_path) {
            if let Ok(table) = content.parse::<toml::Table>() {
                for key in table.keys() {
                    if key.starts_with("profile.") || key == "profile" {
                        // top-level `[profile.X]` sections appear as dotted keys
                        // in some TOML parsers; handle both styles.
                        let name = key.strip_prefix("profile.").unwrap_or(key);
                        if !name.is_empty() {
                            profiles.push(CliProfile {
                                id: format!("profile:{name}"),
                                auth_method: "config_profile".to_string(),
                                active: false,
                                details: None,
                            });
                        }
                    }
                }
                // Also check for `[profiles]` table with sub-tables
                if let Some(toml::Value::Table(pt)) = table.get("profiles") {
                    for name in pt.keys() {
                        profiles.push(CliProfile {
                            id: format!("profile:{name}"),
                            auth_method: "config_profile".to_string(),
                            active: false,
                            details: None,
                        });
                    }
                }
            }
        }
    }

    profiles
}

fn read_codex_email() -> Option<String> {
    let home = dirs::home_dir()?;
    let auth_path = home.join(".codex").join("auth.json");
    let content = std::fs::read_to_string(auth_path).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&content).ok()?;
    // The id_token is a JWT; the email is in the payload.  For robustness
    // we look at the `tokens` object and try to find the email embedded.
    if let Some(tokens) = parsed.get("tokens") {
        if let Some(id_token) = tokens.get("id_token").and_then(|t| t.as_str()) {
            // JWT has 3 dot-separated parts; payload is the second.
            let parts: Vec<&str> = id_token.splitn(3, '.').collect();
            if parts.len() >= 2 {
                // base64url decode the payload
                use base64_decode::decode_jwt_payload;
                if let Some(email) = decode_jwt_payload(parts[1]) {
                    return Some(email);
                }
            }
        }
    }
    None
}

fn extract_codex_auth_method(status_text: &str) -> String {
    let lower = status_text.to_lowercase();
    if lower.contains("chatgpt") {
        "chatgpt_oauth".to_string()
    } else if lower.contains("api key") {
        "api_key".to_string()
    } else {
        "unknown".to_string()
    }
}

/// Gemini: reads `~/.gemini/google_accounts.json` which has
/// `{ "active": "<email>", "old": [...] }`.
fn enumerate_gemini_profiles() -> Vec<CliProfile> {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return vec![],
    };

    let accounts_path = home.join(".gemini").join("google_accounts.json");
    let content = match std::fs::read_to_string(&accounts_path) {
        Ok(c) => c,
        Err(_) => return vec![],
    };

    let parsed: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return vec![],
    };

    let mut profiles = Vec::new();

    if let Some(active) = parsed.get("active").and_then(|v| v.as_str()) {
        if !active.is_empty() {
            profiles.push(CliProfile {
                id: active.to_string(),
                auth_method: "google_oauth".to_string(),
                active: true,
                details: None,
            });
        }
    }

    if let Some(old) = parsed.get("old").and_then(|v| v.as_array()) {
        for entry in old {
            if let Some(email) = entry.as_str() {
                profiles.push(CliProfile {
                    id: email.to_string(),
                    auth_method: "google_oauth".to_string(),
                    active: false,
                    details: None,
                });
            }
        }
    }

    profiles
}

/// OpenCode: reads `~/.local/share/opencode/auth.json` which has provider
/// entries like `{ "openai": { "type": "oauth", ... }, "zai-coding-plan": { "type": "api", ... } }`.
/// Also checks env-var-based providers.
fn enumerate_opencode_profiles() -> Vec<CliProfile> {
    let data_dir = match dirs::data_dir() {
        Some(d) => d,
        None => return vec![],
    };

    let auth_path = data_dir.join("opencode").join("auth.json");
    let content = match std::fs::read_to_string(&auth_path) {
        Ok(c) => c,
        Err(_) => return vec![],
    };

    let parsed: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return vec![],
    };

    let mut profiles = Vec::new();

    if let Some(obj) = parsed.as_object() {
        for (provider_name, provider_data) in obj {
            let auth_type = provider_data
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();

            profiles.push(CliProfile {
                id: provider_name.clone(),
                auth_method: auth_type,
                active: true, // all stored credentials are considered active
                details: None,
            });
        }
    }

    profiles
}

// ---------------------------------------------------------------------------
// JWT payload helper (minimal, no external crate)
// ---------------------------------------------------------------------------

mod base64_decode {
    /// Decode the payload section of a JWT (base64url) and extract the
    /// `email` field.  Returns `None` on any failure — this is best-effort.
    pub fn decode_jwt_payload(payload_b64: &str) -> Option<String> {
        // base64url → standard base64
        let b64: String = payload_b64
            .chars()
            .map(|c| match c {
                '-' => '+',
                '_' => '/',
                other => other,
            })
            .collect();

        // Add padding
        let padded = match b64.len() % 4 {
            2 => format!("{b64}=="),
            3 => format!("{b64}="),
            _ => b64,
        };

        // We use a tiny inline base64 decoder to avoid pulling in a crate.
        let bytes = simple_b64_decode(&padded)?;
        let text = String::from_utf8(bytes).ok()?;
        let parsed: serde_json::Value = serde_json::from_str(&text).ok()?;
        parsed
            .get("email")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    }

    fn simple_b64_decode(input: &str) -> Option<Vec<u8>> {
        const TABLE: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

        fn val(c: u8) -> Option<u8> {
            TABLE.iter().position(|&b| b == c).map(|p| p as u8)
        }

        let bytes: Vec<u8> = input.bytes().filter(|&b| b != b'=').collect();
        let mut out = Vec::with_capacity(bytes.len() * 3 / 4);

        for chunk in bytes.chunks(4) {
            let mut buf: u32 = 0;
            let mut count = 0u8;
            for &b in chunk {
                buf = (buf << 6) | val(b)? as u32;
                count += 1;
            }
            match count {
                4 => {
                    out.push((buf >> 16) as u8);
                    out.push((buf >> 8) as u8);
                    out.push(buf as u8);
                }
                3 => {
                    buf <<= 6;
                    out.push((buf >> 16) as u8);
                    out.push((buf >> 8) as u8);
                }
                2 => {
                    buf <<= 12;
                    out.push((buf >> 16) as u8);
                }
                _ => {}
            }
        }

        Some(out)
    }
}

// ---------------------------------------------------------------------------
// OS detection
// ---------------------------------------------------------------------------

fn detect_os() -> OsInfo {
    OsInfo {
        os_type: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Wrapper scanning
// ---------------------------------------------------------------------------

fn scan_wrappers() -> Vec<WrapperInfo> {
    let mut wrappers = Vec::new();

    if let Some(home) = dirs::home_dir() {
        let bin_dir = home.join(".local").join("bin");
        if bin_dir.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&bin_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_file() {
                        if let Some(target) = identify_wrapper(&path) {
                            let name = path
                                .file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or("")
                                .to_string();
                            wrappers.push(WrapperInfo {
                                name,
                                path: path.clone(),
                                target_cli: Some(target),
                            });
                        }
                    }
                }
            }
        }
    }

    wrappers
}

fn identify_wrapper(path: &PathBuf) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    let lower = content.to_lowercase();

    for (cli, _) in KNOWN_CLIS {
        if lower.contains(cli) {
            return Some(cli.to_string());
        }
    }

    None
}

// ---------------------------------------------------------------------------
// Summarize
// ---------------------------------------------------------------------------

pub fn summarize(report: &DetectionReport) -> Vec<super::actions::CliSummaryItem> {
    report
        .clis
        .iter()
        .map(|cli| {
            let wrapper_count = report
                .wrappers
                .iter()
                .filter(|w| w.target_cli.as_deref() == Some(&cli.name))
                .count();
            super::actions::CliSummaryItem {
                name: cli.name.clone(),
                installed: cli.installed,
                version: cli.version.clone(),
                authenticated: cli.authenticated,
                wrapper_count,
                profiles: cli.profiles.clone(),
                version_changed: cli.version_changed,
                previous_version: cli.previous_version.clone(),
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_os_info() {
        let info = detect_os();
        assert!(!info.os_type.is_empty());
        assert!(!info.arch.is_empty());
    }

    #[test]
    fn detection_report_serializes() {
        let report = DetectionReport {
            clis: vec![],
            os: detect_os(),
            wrappers: vec![],
        };
        let json = serde_json::to_string(&report).unwrap();
        assert!(json.contains("clis"));
    }

    #[test]
    fn version_tracker_records_and_detects_changes() {
        let tracker = VersionTracker::open(Path::new(":memory:")).unwrap();

        // First record — no change (no prior version)
        let changed = tracker.record("claude", "2.1.49", Some("/usr/bin/claude")).unwrap();
        assert!(!changed);

        // Same version — no change
        let changed = tracker.record("claude", "2.1.49", Some("/usr/bin/claude")).unwrap();
        assert!(!changed);

        // Different version — change detected
        let changed = tracker.record("claude", "2.2.0", Some("/usr/bin/claude")).unwrap();
        assert!(changed);

        // Current should be 2.2.0
        let current = tracker.get_current("claude").unwrap().unwrap();
        assert_eq!(current.version, "2.2.0");
    }

    #[test]
    fn version_tracker_history() {
        let tracker = VersionTracker::open(Path::new(":memory:")).unwrap();
        tracker.record("codex", "0.100.0", None).unwrap();
        tracker.record("codex", "0.101.0", None).unwrap();
        tracker.record("codex", "0.104.0", None).unwrap();

        let history = tracker.history("codex", 10).unwrap();
        assert_eq!(history.len(), 3);
        // Newest first
        assert_eq!(history[0].version, "0.104.0");
        assert_eq!(history[2].version, "0.100.0");
    }

    #[test]
    fn version_tracker_missing_cli_returns_none() {
        let tracker = VersionTracker::open(Path::new(":memory:")).unwrap();
        assert!(tracker.get_current("nonexistent").unwrap().is_none());
    }

    #[test]
    fn base64_jwt_email_extraction() {
        // A minimal JWT payload: {"email":"test@example.com","sub":"123"}
        // base64url of that is: eyJlbWFpbCI6InRlc3RAZXhhbXBsZS5jb20iLCJzdWIiOiIxMjMifQ
        let payload = "eyJlbWFpbCI6InRlc3RAZXhhbXBsZS5jb20iLCJzdWIiOiIxMjMifQ";
        let email = base64_decode::decode_jwt_payload(payload);
        assert_eq!(email.as_deref(), Some("test@example.com"));
    }

    #[test]
    fn base64_jwt_no_email_returns_none() {
        // {"sub":"123"}
        let payload = "eyJzdWIiOiIxMjMifQ";
        let email = base64_decode::decode_jwt_payload(payload);
        assert!(email.is_none());
    }

    #[test]
    fn cli_profile_serializes() {
        let profile = CliProfile {
            id: "test@example.com".to_string(),
            auth_method: "oauth".to_string(),
            active: true,
            details: Some(r#"{"plan":"pro"}"#.to_string()),
        };
        let json = serde_json::to_string(&profile).unwrap();
        assert!(json.contains("test@example.com"));
        assert!(json.contains("oauth"));
    }

    #[test]
    fn detect_cli_without_tracker_has_no_version_change() {
        let info = detect_cli("nonexistent_cli_xyz", &[], None);
        assert!(!info.installed);
        assert!(info.version_changed.is_none());
        assert!(info.previous_version.is_none());
        assert!(info.profiles.is_empty());
    }
}
