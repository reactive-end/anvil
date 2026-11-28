//! The checks this repository really has: `anvil check` always, API scenarios and web flows when
//! their files exist. Named in the coach note and in `anvil prompt improve`, so advice never points
//! at a command that has nothing to run.

use std::path::Path;

use anvil_core::config::AnvilConfig;
use anvil_qa::api::scenarios_dir;
use anvil_qa::api::spec::load_scenarios;
use anvil_qa::flows::flows_dir;
use anvil_qa::flows::spec::load_flows;

/// Names (file stems) of the QA scenarios and flows the repository defines.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QaTargets {
    pub scenarios: Vec<String>,
    pub flows: Vec<String>,
}

impl QaTargets {
    /// Reads the scenario and flow directories. Unreadable ones are reported in `problems`.
    pub fn load(root: &Path, config: &AnvilConfig, problems: &mut Vec<String>) -> Self {
        let scenarios = match load_scenarios(&scenarios_dir(root, &config.qa.api)) {
            Ok(entries) => entries.into_iter().map(|e| e.id).collect(),
            Err(err) => {
                problems.push(format!("API scenarios unreadable: {err:#}"));
                Vec::new()
            }
        };
        let flows = match load_flows(&flows_dir(root, &config.qa.web)) {
            Ok(entries) => entries.into_iter().map(|e| e.id).collect(),
            Err(err) => {
                problems.push(format!("web flows unreadable: {err:#}"));
                Vec::new()
            }
        };
        Self { scenarios, flows }
    }

    /// The commands worth naming to an agent: `anvil check` plus one per kind of QA that exists.
    pub fn commands(&self) -> Vec<String> {
        let mut commands = vec!["anvil check".to_owned()];
        if !self.scenarios.is_empty() {
            commands.push("anvil qa api".to_owned());
        }
        if !self.flows.is_empty() {
            commands.push("anvil qa run --all-flows".to_owned());
        }
        commands
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_qa_files_only_anvil_check_is_named() {
        assert_eq!(QaTargets::default().commands(), ["anvil check"]);
    }

    #[test]
    fn qa_commands_appear_only_for_the_kinds_that_exist() {
        let t = QaTargets { scenarios: vec!["a".into()], flows: Vec::new() };
        assert_eq!(t.commands(), ["anvil check", "anvil qa api"]);
        let t = QaTargets { scenarios: vec!["a".into()], flows: vec!["f".into()] };
        assert_eq!(t.commands(), ["anvil check", "anvil qa api", "anvil qa run --all-flows"]);
    }

    #[test]
    fn a_repository_without_qa_directories_has_no_targets() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut problems = Vec::new();
        let t = QaTargets::load(dir.path(), &AnvilConfig::default(), &mut problems);
        assert_eq!(t, QaTargets::default());
        assert!(problems.is_empty(), "{problems:?}");
    }
}
