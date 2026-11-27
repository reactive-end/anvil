//! Prerequisites of a project on a machine: detection from project files, and checks of the tools,
//! versions, env files, MCP servers and secret hygiene they imply.
//!
//! The data model lives in [`anvil_core::config::prereqs`] (it is part of `.harness/config.json`);
//! this crate detects it ([`detect`]) and checks it against a [`Host`] ([`check`]).

pub mod check;
pub mod creds;
pub mod detect;
pub mod envfile;
pub mod hints;
pub mod host;
pub mod probe;
pub mod project;
pub mod report;
pub mod version;

pub use check::{CheckOptions, check};
pub use detect::detect;
pub use host::{Host, HostOs, RunOutcome, ScriptedHost, SystemHost};
pub use project::{Project, projects_with, restrict_to_subrepo};
pub use report::{Item, Kind, Report, Status, Summary};
