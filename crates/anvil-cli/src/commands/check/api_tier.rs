//! Raises `anvil check` to Tier 2 when a change touches the API or the web app under test.

use anvil_core::config::{AnvilConfig, WebQaDriver};
use anvil_core::traits::Tier;

/// Why the scenarios or flows should run although the diff only classifies below Tier 2: the API
/// (`qa.api`) or web app (`qa.web`) is configured and the change is under its watched paths, and
/// the user gave no `--tier`. An explicit `--tier 0|1` wins, so the pre-commit hook and the Stop
/// gate stay fast. `None` when nothing raises the tier.
pub(super) fn raises_to_tier2(
    config: &AnvilConfig,
    tier_override: Option<u8>,
    tier: Tier,
    changed: &[String],
) -> Option<&'static str> {
    if tier_override.is_some() || tier >= Tier::Tier2 {
        return None;
    }
    if api_touched(config, changed) {
        return Some("qa.api.paths touched");
    }
    web_touched(config, changed).then_some("qa.web paths touched")
}

/// The change is under the API's watched paths and the API has a URL.
pub(super) fn api_touched(config: &AnvilConfig, changed: &[String]) -> bool {
    config.qa.api.url.is_some() && config.qa.api.touches(changed)
}

/// The change is under the web app's watched paths (code, not only markup and styles), the app
/// has a URL, and web QA is not switched off.
pub(super) fn web_touched(config: &AnvilConfig, changed: &[String]) -> bool {
    config.qa.web.url.is_some()
        && config.qa_driver.web != WebQaDriver::Skip
        && config.qa.web.touches(changed)
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
        assert!(raises_to_tier2(&config, None, Tier::Tier1, &files).is_some());
        assert!(raises_to_tier2(&config, None, Tier::Tier0, &files).is_some());
    }

    #[test]
    fn an_explicit_tier_unrelated_changes_and_a_missing_url_do_not() {
        let config = config(&["orders-api"]);
        let files = changed(&["orders-api/src/main.ts"]);
        assert!(raises_to_tier2(&config, Some(0), Tier::Tier0, &files).is_none());
        assert!(raises_to_tier2(&config, Some(1), Tier::Tier1, &files).is_none());
        assert!(raises_to_tier2(&config, None, Tier::Tier2, &files).is_none());
        let web_file = changed(&["orders-web/a.ts"]);
        assert!(raises_to_tier2(&config, None, Tier::Tier1, &web_file).is_none());
        assert!(raises_to_tier2(&AnvilConfig::default(), None, Tier::Tier1, &files).is_none());
    }

    #[test]
    fn front_end_code_under_the_web_paths_raises_unless_web_qa_is_skipped() {
        let mut config = AnvilConfig::default();
        config.qa.web.url = Some("http://localhost:5280".to_owned());
        config.qa.web.start_cwd = Some("inventory-web".to_owned());
        let files = changed(&["inventory-web/src/api.ts"]);
        assert_eq!(
            raises_to_tier2(&config, None, Tier::Tier1, &files),
            Some("qa.web paths touched")
        );
        assert!(raises_to_tier2(&config, None, Tier::Tier1, &changed(&["api/x.ts"])).is_none());
        config.qa_driver.web = WebQaDriver::Skip;
        assert!(raises_to_tier2(&config, None, Tier::Tier1, &files).is_none());
    }
}
