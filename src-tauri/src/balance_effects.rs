use agent_runner_balancer::BalanceEffects;
use agent_runner_config::{ProvidersConfig, SessionsConfig};
use agent_runner_executor::ProcessRunner;
use agent_runner_quota::{InFlight, RefreshOutcome, is_stale, refresh_provider};
use agent_runner_session::scan_provider_with_runner_and_chain;
use agent_runner_state::{QuotaRepository, SessionChainRepository, SessionTurnRepository};

pub struct BalanceContext<'a> {
    pub providers_cfg: &'a ProvidersConfig,
    pub sessions_cfg: &'a SessionsConfig,
    pub in_flight: &'a InFlight,
    pub quota_repo: &'a dyn QuotaRepository,
    pub turn_repo: &'a dyn SessionTurnRepository,
    pub chain_repo: Option<&'a dyn SessionChainRepository>,
    pub runner: &'a dyn ProcessRunner,
}

impl BalanceEffects for BalanceContext<'_> {
    fn refresh_quota_if_stale(&self, provider_name: &str) {
        if is_stale(self.quota_repo, provider_name) {
            let _: RefreshOutcome = refresh_provider(
                provider_name,
                self.providers_cfg,
                self.in_flight,
                self.quota_repo,
                self.runner,
            );
        }
    }

    fn scan_provider_sessions(&self, provider_name: &str) {
        let _ = scan_provider_with_runner_and_chain(
            provider_name,
            self.sessions_cfg,
            self.turn_repo,
            self.chain_repo,
            self.runner,
        );
    }
}
