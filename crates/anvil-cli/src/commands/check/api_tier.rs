//! Raises `anvil check` to Tier 2 when a change touches the API under test.

use anvil_core::config::AnvilConfig;
use anvil_core::traits::Tier;

/// `true` when the scenarios should run although the diff only classifies below Tier 2: the API
/// is configured, the change is under `qa.api.paths`, and the user gave no `--tier`. An explicit
/// `--tier 0|1` wins, so the pre-commit hook and the Stop gate stay fast.
pub(super) fn raises_to_tier2(
    config: &AnvilConfig,
    tier_override: Option<u8>,
    tier: Tier,
    changed: &[String],
) -> bool {
    tier_override.is_none()
        && tier < Tier::Tier2
        && config.qa.api.url.is_some()
        && config.qa.api.touches(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(paths: &[&str]) -> AnvilConfig {
        let mut config = AnvilConfig::default();
        config.qa.api.url = Some("http://localhost:3100".to_owned());
        config.qa.api.paths = paths.iter().map(|p| (*p).to_owned()).collect();
        config
    }

    fn changed(files: &[&str]) -> Vec<String> {
        files.iter().map(|f| (*f).to_owned()).collect()
    }

    #[test]
    fn a_change_under_the_api_paths_raises_tier1_to_tier2() {
        let config = config(&["orders-api"]);
        let files = changed(&["orders-api/src/main.ts"]);
        assert!(raises_to_tier2(&config, None, Tier::Tier1, &files));
        assert!(raises_to_tier2(&config, None, Tier::Tier0, &files));
    }

    #[test]
    fn an_explicit_tier_unrelated_changes_and_a_missing_url_do_not() {
        let config = config(&["orders-api"]);
        let files = changed(&["orders-api/src/main.ts"]);
        assert!(!raises_to_tier2(&config, Some(0), Tier::Tier0, &files));
        assert!(!raises_to_tier2(&config, Some(1), Tier::Tier1, &files));
        assert!(!raises_to_tier2(&config, None, Tier::Tier2, &files));
        assert!(!raises_to_tier2(&config, None, Tier::Tier1, &changed(&["orders-web/a.ts"])));
        assert!(!raises_to_tier2(&AnvilConfig::default(), None, Tier::Tier1, &files));
    }
}
