//! Install hints shown next to a missing or outdated tool.

use crate::probe::canonical;

/// How to install `name` (any alias), mentioning `version` when the project pins one.
pub fn install_hint(name: &str, version: Option<&str>) -> String {
    let v = version.map_or(String::new(), |v| format!(" {v}"));
    let canon = canonical(name);
    match canon.as_str() {
        "node" => format!("Install Node.js{v} (nvm, fnm, volta or https://nodejs.org)"),
        "npm" => "npm ships with Node.js; reinstall Node or run `npm install -g npm`".to_owned(),
        "pnpm" => "Run `corepack enable pnpm` or `npm install -g pnpm`".to_owned(),
        "yarn" => "Run `corepack enable yarn` or `npm install -g yarn`".to_owned(),
        "bun" => "Install Bun from https://bun.sh".to_owned(),
        "python" => format!("Install Python{v} (https://www.python.org, pyenv or `uv python install`)"),
        "uv" => "Install uv: https://docs.astral.sh/uv/getting-started/installation/".to_owned(),
        "poetry" => "Install Poetry: https://python-poetry.org/docs/#installation".to_owned(),
        "java" => format!("Install a JDK{v} (e.g. Temurin) and point JAVA_HOME at it"),
        "rust" | "cargo" => "Install Rust with https://rustup.rs".to_owned(),
        "go" => format!("Install Go{v}: https://go.dev/dl/"),
        "terraform" => format!(
            "Install Terraform{v}: https://developer.hashicorp.com/terraform/install (inside WSL on Windows)"
        ),
        "docker" => "Install Docker: https://docs.docker.com/get-docker/".to_owned(),
        "docker compose" => "Install the Docker Compose v2 plugin (ships with Docker Desktop)".to_owned(),
        "dart" | "flutter" => "Install Flutter (includes Dart): https://docs.flutter.dev/get-started/install".to_owned(),
        "php" => format!("Install PHP{v}: https://www.php.net/downloads"),
        "composer" => "Install Composer: https://getcomposer.org/download/".to_owned(),
        "sonar-scanner" => "Install SonarScanner: https://docs.sonarsource.com/sonarqube/latest/analyzing-source-code/scanners/sonarscanner/".to_owned(),
        "gh" => "Install the GitHub CLI: https://cli.github.com".to_owned(),
        "git" => "Install Git: https://git-scm.com/downloads".to_owned(),
        "anvil" => "Install Anvil: see the README install section (scripts/install.sh or scripts/install.ps1)".to_owned(),
        "adb" => "Install Android platform-tools (Android Studio SDK Manager) and add them to PATH".to_owned(),
        "android-sdk" => "Install Android Studio, then set `sdk.dir` in local.properties or ANDROID_HOME".to_owned(),
        "xcodebuild" => "Install Xcode from the App Store (macOS only)".to_owned(),
        "pod" => "Install CocoaPods: `brew install cocoapods` (macOS only)".to_owned(),
        "psql" => "Install the PostgreSQL client tools".to_owned(),
        "gcloud" => "Install the Google Cloud CLI: https://cloud.google.com/sdk/docs/install".to_owned(),
        "aws" => "Install the AWS CLI: https://docs.aws.amazon.com/cli/latest/userguide/getting-started-install.html".to_owned(),
        "chrome" => "Install Google Chrome or set CHROME_BIN".to_owned(),
        other => format!("Install `{other}`{v} and make sure it is on PATH"),
    }
}
