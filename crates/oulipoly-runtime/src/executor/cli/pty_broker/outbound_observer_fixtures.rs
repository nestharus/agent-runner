//! Disposable provider fixture and virtual scheduler clock; never opens a PTY.
use super::*;
use crate::provider_registry::ProviderRegistryOptions;
use oulipoly_config::{
    ModelConfig, PromptMode, ProviderConfig, ProviderEndpointConfig, ProviderEntry,
    ProvidersConfig, provider_implementation_ref::ProviderImplementationRef,
};
use std::fs;
use std::os::unix::fs::PermissionsExt;

pub(crate) struct ObserverFixture {
    _dir: tempfile::TempDir,
    mode: PathBuf,
    calls: PathBuf,
    source: OutboundObserverSource,
    pub(crate) worker: OutboundObserverWorker,
    now: Instant,
    next_scan: Instant,
}
impl ObserverFixture {
    pub(crate) fn new(mode: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mode_path = dir.path().join("mode");
        let calls = dir.path().join("calls");
        fs::write(&mode_path, mode).unwrap();
        fs::write(dir.path().join("native.jsonl"), "").unwrap();
        let script = dir.path().join("provider.py");
        let text = r#"#!/usr/bin/env python3
import json,sys,pathlib,hashlib
root=pathlib.Path(__file__).parent
r=json.load(sys.stdin)
cmd=sys.argv[1]
result={"contract":r["contract"],"request_id":r["request_id"],"ok":True}
if cmd=="describe":
 result["result"]={"provider_id":"synthetic","display_name":"Synthetic","contract_versions":[r["contract"]],"preferred_contract":r["contract"],"capabilities":{"launch":False,"policy":False,"quota":False,"session":True,"session_enumerate":False,"session_turn_pages_v1":True,"terminal":False,"rotation":False,"discovery":False,"settings":False,"setup_brain":False,"setup":False,"migration":False}}
else:
 with (root/"calls").open("a") as f: f.write(json.dumps(r)+"\n")
 mode=(root/"mode").read_text()
 if mode=="native-pages":
  # Synthetic append-only JSONL. One complete native record per page;
  # non-user records consume a page without producing a projected turn.
  p=r["params"]
  records=[json.loads(line) for line in (root/"native.jsonl").read_text().splitlines()]
  tail=p.get("start_mode")=="tail"
  if p.get("page_token"):
   offset,end,index,sequence=json.loads(p["page_token"])
  else:
   offset=len(records) if tail else int(p.get("after_token") or "0")
   end,index,sequence=len(records),0,0
  turns=[]
  if not tail and offset<end:
   record=records[offset]
   if record["role"]=="user":
    turns=[{"session_id":"synthetic-session","turn_id":"native-"+str(offset),"snapshot_sequence":sequence,"timestamp":"2026-05-01T00:00:01Z","role":"user","is_sidechain":False,"is_compaction_boundary":False,"parent_turn_id":None,"body":None,"body_bytes":len(record["body"].encode()),"body_sha256":None,"body_state":"omitted_oversize","canonical_text_sha256":hashlib.sha256(record["body"].encode()).hexdigest()}]
   offset+=1
  complete=offset==end
  result["result"]={"read_protocol":"oulipoly.session_turn_pages/v1","provider_instance_id":"synthetic-instance","settings_id":"synthetic-settings","session_id":"synthetic-session","turn_projection":"user_observation","snapshot_id":"native-snapshot-"+str(end),"page_index":index,"page_start_sequence":sequence,"turns":turns,"page_turn_count":len(turns),"source_bytes_examined":1,"scan_progress":not complete and not turns,"snapshot_complete":complete,"next_page_token":None if complete else json.dumps([offset,end,index+1,sequence+len(turns)]),"resume_token":str(offset) if complete else None,"source_final":False,"warnings":[]}
 elif mode!="restored":
  result.update(ok=False,error={"category":"failed","code":mode,"message":"untrusted provider message","retryable":False})
 else:
  p=r["params"]
  tail=p.get("start_mode")=="tail"
  continuing=bool(p.get("page_token"))
  complete=tail or continuing
  result["result"]={"read_protocol":"oulipoly.session_turn_pages/v1","provider_instance_id":"synthetic-instance","settings_id":"synthetic-settings","session_id":"synthetic-session","turn_projection":"user_observation","snapshot_id":"synthetic-snapshot","page_index":1 if continuing else 0,"page_start_sequence":0,"turns":[],"page_turn_count":0,"source_bytes_examined":1,"scan_progress":not complete,"snapshot_complete":complete,"next_page_token":None if complete else "synthetic-page-1","resume_token":"synthetic-anchor" if complete else None,"source_final":False,"warnings":[]}
print(json.dumps(result))
"#;
        let text = text.replace(
            "root=pathlib.Path(__file__).parent",
            &format!(
                "root=pathlib.Path({})",
                serde_json::to_string(&dir.path().display().to_string()).unwrap()
            ),
        );
        fs::write(&script, text).unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        let model = ModelConfig {
            name: "synthetic-model".into(),
            prompt_mode: PromptMode::Arg,
            providers: vec![ProviderConfig::model_provider("synthetic-account", vec![])],
            inputs: vec![],
            provider: Some(ProviderImplementationRef {
                path: Some(script.display().to_string()),
                crate_name: None,
                version: None,
                binary: None,
                script: None,
            }),
        };
        let providers = ProvidersConfig {
            entries: [(
                "synthetic-account".into(),
                ProviderEntry {
                    implementation: Some(ProviderEndpointConfig {
                        family: "synthetic".into(),
                        executable: script.display().to_string(),
                    }),
                    settings_id: Some("synthetic-settings".into()),
                    ..ProviderEntry::default()
                },
            )]
            .into(),
        };
        let registry = ProviderRegistry::from_configs(
            &[model],
            &providers,
            ProviderRegistryOptions::default()
                .with_config_root(dir.path().join("config"))
                .with_data_root(dir.path().join("data")),
        )
        .unwrap();
        let source = OutboundObserverSource::Provider(Box::new(ProviderSessionTurnSource::new(
            Arc::new(registry),
            SessionProviderIdentity {
                model_name: "synthetic-model".into(),
                provider_name: "synthetic-account".into(),
                provider_instance_id: Some("synthetic-instance".into()),
                settings_id: "synthetic-settings".into(),
            },
            "synthetic-session".into(),
            "synthetic-invocation".into(),
            None,
        )));
        let now = Instant::now();
        Self {
            _dir: dir,
            mode: mode_path,
            calls,
            source,
            worker: OutboundObserverWorker {
                shared: Arc::new(ObserverShared::new()),
                join: None,
            },
            now,
            next_scan: now,
        }
    }
    pub(crate) fn append_native(&self, role: &str, body: &str) {
        use std::io::Write;
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(self._dir.path().join("native.jsonl"))
            .unwrap();
        writeln!(file, "{}", serde_json::json!({"role": role, "body": body})).unwrap();
    }

    pub(crate) fn set_mode(&self, mode: &str) {
        fs::write(&self.mode, mode).unwrap();
    }
    pub(crate) fn calls(&self) -> Vec<serde_json::Value> {
        fs::read_to_string(&self.calls)
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }
    pub(crate) fn cursor(&self) -> String {
        let OutboundObserverSource::Provider(source) = &self.source else {
            unreachable!()
        };
        format!("{:?}", *lock_or_recover(&source.cursor))
    }
    pub(crate) fn tick(&mut self) -> bool {
        self.now += OBSERVATION_INTERVAL;
        let read = lock_or_recover(&self.worker.shared.state).take_read(self.now, self.next_scan);
        let Some((generation, reset)) = read else {
            return false;
        };
        execute_read(&self.source, &self.worker.shared, generation, reset);
        self.next_scan = self.now + OBSERVATION_INTERVAL;
        true
    }
    pub(crate) fn stopped(&self) -> (u64, &'static str) {
        lock_or_recover(&self.worker.shared.state)
            .stopped
            .expect("stopped")
    }
}

#[test]
fn refused_anchor_refresh_preserves_cursor_and_latches_despite_inflight_refresh() {
    let mut fixture = ObserverFixture::new("restored");
    fixture.worker.set_demand(true);
    assert!(fixture.tick());
    let tail = fixture.worker.latest_result().unwrap();
    fixture.worker.acknowledge(&tail);
    fixture.worker.observe_after_anchor();
    assert!(fixture.tick());
    let cursor = fixture.cursor();
    let partial = fixture.worker.latest_result().unwrap();
    fixture.worker.acknowledge(&partial);
    fixture.worker.request_fresh_generation();
    fixture.set_mode("session_turn_paging_paused");
    // Admit the read, then race a routine refresh before its result publishes.
    let (generation, reset) = lock_or_recover(&fixture.worker.shared.state)
        .take_read(fixture.now, fixture.now)
        .unwrap();
    assert!(reset);
    // Replayed acknowledgements cannot release a newer in-flight reservation.
    fixture.worker.acknowledge(&partial);
    assert!(
        lock_or_recover(&fixture.worker.shared.state)
            .take_read(fixture.now, fixture.now)
            .is_none()
    );
    fixture.worker.request_fresh_generation();
    execute_read(&fixture.source, &fixture.worker.shared, generation, reset);
    assert_eq!(fixture.cursor(), cursor);
    assert_eq!(
        fixture.stopped(),
        (generation, "session_turn_paging_paused")
    );
    for _ in 0..400 {
        assert!(!fixture.tick());
    }
    assert_eq!(fixture.calls().len(), 3);
    assert!(
        fixture
            .worker
            .rearm_after_resolution(
                generation - 1,
                "session_turn_paging_paused",
                ObservationResolution::PagingRestored
            )
            .is_err()
    );
    fixture
        .worker
        .rearm_after_resolution(
            generation,
            "session_turn_paging_paused",
            ObservationResolution::PagingRestored,
        )
        .unwrap();
    fixture.set_mode("restored");
    assert!(fixture.tick());
    let calls = fixture.calls();
    assert_eq!(calls[3]["params"]["page_token"], "synthetic-page-1");
}
