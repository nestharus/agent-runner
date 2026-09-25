use crate::usage::row::{RowState, UsageRow, UsageWindow};
use crate::usage::{accessor, filter, renderer};
use oulipoly_config::{ModelConfig, ProvidersConfig};
use oulipoly_kernel_broker::protocol::{self, ManualQuotaReadback, ManualQuotaRequest};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    operation_id: String,
    model: String,
    account: String,
    config_sha256: String,
    environment_sha256: String,
}

fn environment_digest(environment: &[(String, String)]) -> Result<String, String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(environment).map_err(|e| e.to_string())?)
    ))
}

impl Journal {
    fn request(&self, environment: Vec<(String, String)>) -> ManualQuotaRequest {
        ManualQuotaRequest {
            operation_id: self.operation_id.clone(),
            model: self.model.clone(),
            account: self.account.clone(),
            config_sha256: self.config_sha256.clone(),
            environment,
        }
    }
}

fn socket() -> PathBuf {
    if unsafe { libc::geteuid() } == 0
        && std::fs::read_to_string("/proc/self/uid_map")
            .ok()
            .is_some_and(|map| map.split_ascii_whitespace().nth(2) == Some("1"))
        && let Some(path) = std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1")
    {
        return PathBuf::from(path);
    }
    PathBuf::from(protocol::INSTALLED_FRESH_V30_SOCKET)
}

fn journal_path(physical_id: &str) -> Result<PathBuf, String> {
    let root = oulipoly_state::paths::data_dir()?.join("manual-quota-operations");
    fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let hash = format!("{:x}", Sha256::digest(physical_id.as_bytes()));
    Ok(root.join(format!("{hash}.json")))
}

fn durable_request(path: &Path, request: &ManualQuotaRequest) -> io::Result<()> {
    let journal = Journal {
        operation_id: request.operation_id.clone(),
        model: request.model.clone(),
        account: request.account.clone(),
        config_sha256: request.config_sha256.clone(),
        environment_sha256: environment_digest(&request.environment).map_err(io::Error::other)?,
    };
    // Publish a fully synced journal atomically. Another --usage process may
    // then read the same ID without observing an incomplete JSON document.
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)?;
    let publish = (|| {
        serde_json::to_writer(&mut file, &journal).map_err(io::Error::other)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        fs::hard_link(&temporary, path)?;
        File::open(
            path.parent()
                .ok_or_else(|| io::Error::other("manual quota journal parent absent"))?,
        )?
        .sync_all()
    })();
    let _ = fs::remove_file(&temporary);
    publish
}

fn read_request(path: &Path) -> Result<Option<Journal>, String> {
    match File::open(path) {
        Ok(file) => serde_json::from_reader(file)
            .map(Some)
            .map_err(|e| e.to_string()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

struct ManualCallError {
    text: String,
    no_intent: bool,
}

fn no_broker_intent(error: &io::Error) -> bool {
    error.to_string().contains("manual quota operation absent")
}

fn observe_or_begin(
    source: &File,
    request: &ManualQuotaRequest,
    may_begin: bool,
) -> Result<ManualQuotaReadback, ManualCallError> {
    let socket = socket();
    match protocol::private_manual_quota_observe_at(&socket, &request.operation_id) {
        Ok(result) => Ok(result),
        Err(error) if no_broker_intent(&error) => {
            if !may_begin {
                return Err(ManualCallError {
                    text: format!(
                        "manual quota request {} has no broker intent and its effect environment changed; no K was granted",
                        request.operation_id
                    ),
                    no_intent: true,
                });
            }
            match protocol::private_manual_quota_at(
                &socket,
                request,
                true,
                Some(source.as_raw_fd()),
            ) {
                Ok(result) => Ok(result),
                Err(begin_error) => {
                    match protocol::private_manual_quota_observe_at(&socket, &request.operation_id)
                    {
                        Ok(result) => Ok(result),
                        Err(read_error) => Err(ManualCallError {
                            no_intent: no_broker_intent(&read_error),
                            text: format!(
                                "manual quota begin/readback for {}: begin: {begin_error}; exact readback: {read_error}",
                                request.operation_id
                            ),
                        }),
                    }
                }
            }
        }
        Err(error) => Err(ManualCallError {
            text: format!(
                "manual quota readback unknown for {}: {error}",
                request.operation_id
            ),
            no_intent: false,
        }),
    }
}

fn wait_readback(
    request: &ManualQuotaRequest,
    mut result: ManualQuotaReadback,
) -> ManualQuotaReadback {
    let deadline = Instant::now() + Duration::from_secs(30);
    while result.state == "unknown" && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
        match protocol::private_manual_quota_observe_at(&socket(), &request.operation_id) {
            Ok(next) => result = next,
            Err(_) => break,
        }
    }
    result
}

fn row_from_result(label: &str, vendor: &str, result: &ManualQuotaReadback) -> UsageRow {
    let windows = result
        .windows
        .iter()
        .enumerate()
        .map(|(index, window)| UsageWindow {
            label: format!("window-{index}"),
            used_percent: window.used_percent / 100.0,
            resets_at: window.resets_at.clone(),
            limit: None,
            remaining: window.remaining,
            unit: None,
        })
        .collect();
    let row_state = match (result.state.as_str(), result.outcome.as_deref()) {
        ("unmetered", _) => RowState::Unmetered,
        ("drained", Some("valid_windows")) => RowState::HasWindows,
        ("drained", Some("empty")) => RowState::NoWindows,
        ("drained", Some(outcome)) => RowState::Error(outcome.into()),
        _ => RowState::Error(format!("unknown Q: {}", result.artifact)),
    };
    UsageRow {
        account_id: label.into(),
        vendor: vendor.into(),
        windows,
        row_state,
        cache_warning: None,
    }
}

pub(crate) fn run(
    providers: &ProvidersConfig,
    models: &[ModelConfig],
    writer: &mut impl Write,
) -> Result<i32, String> {
    let source_path = oulipoly_state::paths::config_dir()?;
    let source = File::open(&source_path)
        .map_err(|e| format!("manual quota config source unavailable: {e}"))?;
    let accounts = accessor::collect_accounts(providers, models);
    let mut rows = Vec::new();
    let mut settled_journals = Vec::new();
    let mut incomplete = !accounts.warnings.is_empty();
    for warning in accounts.warnings {
        eprintln!("{warning}");
    }
    let filtered = filter::filter_routing_pool(accounts.accounts);
    let mut sources = std::collections::HashMap::<String, (Option<String>, Option<String>)>::new();
    let mut conflicts = std::collections::HashSet::new();
    for account in &filtered {
        if let Some(id) = &account.provider_entry.quota_account_id {
            let effects = (
                account.provider_entry.quota_script.clone(),
                account.provider_entry.auth_refresh_command.clone(),
            );
            if sources.get(id).is_some_and(|prior| prior != &effects) {
                conflicts.insert(id.clone());
            }
            sources.entry(id.clone()).or_insert(effects);
        }
    }
    let mut seen = std::collections::HashSet::new();
    for account in filtered {
        let Some(physical_id) = account.provider_entry.quota_account_id.as_deref() else {
            incomplete = true;
            rows.push(UsageRow {
                account_id: account.account_id,
                vendor: account.vendor,
                windows: Vec::new(),
                row_state: RowState::Error("physical account ID absent".into()),
                cache_warning: None,
            });
            continue;
        };
        if !seen.insert(physical_id.to_string()) {
            continue;
        }
        if conflicts.contains(physical_id) {
            incomplete = true;
            rows.push(UsageRow {
                account_id: physical_id.into(),
                vendor: account.vendor,
                windows: Vec::new(),
                row_state: RowState::Error("physical account has conflicting quota sources".into()),
                cache_warning: None,
            });
            continue;
        }
        let Some(model) = models.iter().find(|model| {
            model
                .providers
                .iter()
                .any(|member| member.name == account.account_id)
        }) else {
            incomplete = true;
            continue;
        };
        let pool = oulipoly_runtime::executor::cli::fresh_remote::load_fresh_headless_pool(
            &source_path,
            &model.name,
        )?;
        let path = journal_path(physical_id)?;
        let index = pool
            .model
            .providers
            .iter()
            .position(|provider| provider.name == account.account_id)
            .ok_or("manual quota provider absent from pinned model")?;
        // Use the same assembled effect environment as the fresh route. It
        // includes config overrides, removals, and the pinned data directory.
        let environment = oulipoly_runtime::executor::cli::fresh_remote::prepare_fresh_headless(
            &pool.model,
            index,
            "",
            &std::env::current_dir().map_err(|e| e.to_string())?,
        )?
        .plan
        .environment;
        let journal = match read_request(&path)? {
            Some(prior) => prior,
            None => {
                let request = ManualQuotaRequest {
                    operation_id: uuid::Uuid::new_v4().to_string(),
                    model: model.name.clone(),
                    account: account.account_id.clone(),
                    config_sha256: pool.config_sha256,
                    environment: environment.clone(),
                };
                match durable_request(&path, &request) {
                    Ok(()) => {
                        read_request(&path)?.ok_or("manual quota request journal unavailable")?
                    }
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                        read_request(&path)?
                            .ok_or("manual quota concurrent request journal unavailable")?
                    }
                    Err(error) => return Err(format!("manual quota journal unavailable: {error}")),
                }
            }
        };
        let may_begin = journal.environment_sha256 == environment_digest(&environment)?;
        let request = journal.request(environment);
        let result = observe_or_begin(&source, &request, may_begin)
            .map(|result| wait_readback(&request, result))
            .and_then(|result| {
                if result.operation_id != request.operation_id
                    || result.physical_account_id != physical_id
                    || (result.state == "drained" && result.effect_id.is_none())
                {
                    Err(ManualCallError {
                        text: format!(
                            "manual quota exact account/Q readback mismatch for {}",
                            request.operation_id
                        ),
                        no_intent: false,
                    })
                } else {
                    Ok(result)
                }
            });
        let row = match result {
            Ok(result) => {
                if matches!(result.state.as_str(), "drained" | "unmetered") {
                    settled_journals.push(path);
                }
                row_from_result(physical_id, &account.vendor, &result)
            }
            Err(error) => {
                if error.no_intent {
                    // The broker's exact observation found no durable intent.
                    // Since K is published only after intent, this journal
                    // cannot denote a spent effect and a later call may retry.
                    settled_journals.push(path);
                }
                UsageRow {
                    account_id: physical_id.into(),
                    vendor: account.vendor.clone(),
                    windows: Vec::new(),
                    row_state: RowState::Error(error.text),
                    cache_warning: None,
                }
            }
        };
        incomplete |= matches!(row.row_state, RowState::Error(_) | RowState::NoWindows);
        rows.push(row);
    }
    renderer::render(&rows, writer).map_err(|e| e.to_string())?;
    writer.flush().map_err(|e| e.to_string())?;
    for path in settled_journals {
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(i32::from(incomplete))
}
