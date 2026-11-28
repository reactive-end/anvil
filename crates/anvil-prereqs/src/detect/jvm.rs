//! JVM, Android and iOS projects: JDK for Gradle, Android SDK and `adb`, Xcode and CocoaPods.

use std::sync::OnceLock;

use regex::Regex;

use super::Scan;

const GRADLE_FILES: &[&str] = &[
    "build.gradle",
    "build.gradle.kts",
    "app/build.gradle",
    "app/build.gradle.kts",
    "android/build.gradle",
    "android/app/build.gradle",
    "android/app/build.gradle.kts",
];

pub(super) fn run(scan: &mut Scan<'_>) {
    let gradle = scan.has("gradlew")
        || scan.has("settings.gradle")
        || scan.has("settings.gradle.kts")
        || GRADLE_FILES.iter().any(|f| scan.has(f));
    if gradle {
        java(scan);
    }
    if is_android(scan) {
        android(scan);
    }
    ios(scan);
}

/// The daemon JVM file says which JDK Gradle itself runs on; toolchain declarations only set a floor.
fn java(scan: &mut Scan<'_>) {
    let daemon = scan.read("gradle/gradle-daemon-jvm.properties").and_then(|text| {
        text.lines()
            .find_map(|l| l.trim().strip_prefix("toolchainVersion=").map(|v| v.trim().to_owned()))
    });
    if let Some(version) = daemon {
        let source = scan.src("gradle/gradle-daemon-jvm.properties", "toolchainVersion");
        scan.tool("java", Some(version), source, true);
        return;
    }
    for file in GRADLE_FILES {
        let Some(text) = scan.read(file) else { continue };
        if let Some(major) = declared_major(&text) {
            let source = scan.src(file, "java toolchain");
            scan.tool("java", Some(format!("{major}+")), source, true);
            return;
        }
    }
    let source = scan.src("gradlew", "");
    scan.tool("java", None, source, true);
}

/// `jvmToolchain(17)`, `JavaLanguageVersion.of(21)`, `JavaVersion.VERSION_17` / `VERSION_1_8`.
fn declared_major(gradle: &str) -> Option<u32> {
    static PATTERNS: OnceLock<[Regex; 3]> = OnceLock::new();
    let patterns = PATTERNS.get_or_init(|| {
        [
            r"jvmToolchain\(\s*(\d+)\s*\)",
            r"JavaLanguageVersion\.of\(\s*(\d+)\s*\)",
            r"JavaVersion\.VERSION_(?:1_)?(\d+)",
        ]
        .map(|p| Regex::new(p).expect("static pattern"))
    });
    patterns.iter().find_map(|re| re.captures(gradle)?[1].parse().ok())
}

fn is_android(scan: &Scan<'_>) -> bool {
    let plugin = GRADLE_FILES
        .iter()
        .filter_map(|f| scan.read(f))
        .any(|text| text.contains("com.android.") || text.contains("android {"));
    plugin
        || scan.has("app/src/main/AndroidManifest.xml")
        || scan.has("android/app/src/main/AndroidManifest.xml")
}

fn android(scan: &mut Scan<'_>) {
    let source = scan.src("local.properties", "sdk.dir / ANDROID_HOME");
    scan.tool("android-sdk", None, source, true);
    let source = scan.src("build.gradle", "android");
    scan.tool("adb", None, source, false);
}

fn ios(scan: &mut Scan<'_>) {
    let podfile = ["Podfile", "ios/Podfile"].into_iter().find(|f| scan.has(f));
    let xcode = ["ios", "."].iter().any(|dir| {
        std::fs::read_dir(scan.dir.join(dir))
            .into_iter()
            .flatten()
            .flatten()
            .any(|e| e.file_name().to_string_lossy().ends_with(".xcodeproj"))
    });
    if podfile.is_none() && !xcode {
        return;
    }
    let marker = podfile.unwrap_or("ios");
    let source = scan.src(marker, "iOS project (macOS only)");
    scan.tool("xcodebuild", None, source.clone(), true);
    if podfile.is_some() {
        scan.tool("pod", None, source, true);
    }
}
