//! Ingress quota and outbound feature policy from resolved configuration.

use crate::config::ResolvedConfig;

/// Per-ingress policy for receipt and outbound quotas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IngressPolicy {
    pub per_user_daily_receipts: u64,
    pub outbound_enabled: bool,
    pub zalo_monthly_messages: u64,
    pub insights_llm_enabled: bool,
    pub monthly_insight_narratives: u64,
}

impl Default for IngressPolicy {
    fn default() -> Self {
        Self {
            per_user_daily_receipts: 20,
            outbound_enabled: true,
            zalo_monthly_messages: 3000,
            insights_llm_enabled: false,
            monthly_insight_narratives: 30,
        }
    }
}

impl IngressPolicy {
    pub fn from_config(config: &ResolvedConfig) -> Self {
        Self {
            per_user_daily_receipts: config.per_user_daily_receipts,
            outbound_enabled: config.outbound_enabled,
            zalo_monthly_messages: config.zalo_monthly_messages,
            insights_llm_enabled: config.insights_llm_enabled,
            monthly_insight_narratives: config.monthly_insight_narratives,
        }
    }
}
