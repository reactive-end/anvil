use std::fs;

use anvil_qa::QaEngine;
use anvil_qa::mobile::{scan_logcat, scan_roborazzi_diffs, scan_uiautomator_xml};
use anvil_qa::model::{QaViolationKind, UiPlatform};
use anvil_qa::report::ReportBudget;

#[test]
fn uiautomator_detects_clipped_text_and_overlapping_clickables() {
    let xml = r#"<?xml version='1.0' encoding='UTF-8' standalone='yes' ?>
<hierarchy rotation="0">
  <node index="0" text="" class="android.widget.FrameLayout" clickable="false" scrollable="false" bounds="[0,0][1080,2400]">
    <node index="0" text="Header" resource-id="com.centinela:id/header" class="android.widget.TextView" clickable="false" scrollable="false" bounds="[0,0][1080,120]" />
    <node index="1" text="Clipped outside viewport" resource-id="com.centinela:id/bad_text" class="android.widget.TextView" clickable="false" scrollable="false" bounds="[900,150][1180,210]" />
    <node index="2" text="" resource-id="com.centinela:id/feed" class="androidx.recyclerview.widget.RecyclerView" clickable="false" scrollable="true" bounds="[0,220][1080,1200]">
      <node index="0" text="Scrollable row text" resource-id="com.centinela:id/scroll_row" class="android.widget.TextView" clickable="false" scrollable="false" bounds="[0,1150][1080,1320]" />
    </node>
    <node index="3" text="Confirm" resource-id="com.centinela:id/confirm_btn" class="android.widget.Button" clickable="true" enabled="true" bounds="[100,1400][500,1520]" />
    <node index="4" text="Cancel" resource-id="com.centinela:id/cancel_btn" class="android.widget.Button" clickable="true" enabled="true" bounds="[420,1450][820,1570]" />
    <node index="5" text="" resource-id="com.centinela:id/card" class="android.view.ViewGroup" clickable="true" enabled="true" bounds="[100,1650][980,1900]">
      <node index="0" text="Inner action" resource-id="com.centinela:id/card_cta" class="android.widget.Button" clickable="true" enabled="true" bounds="[700,1720][920,1820]" />
    </node>
  </node>
</hierarchy>"#;

    let violations = scan_uiautomator_xml(xml, 1080, 2400);
    assert_eq!(violations.len(), 2, "expected exact 2 violations: {violations:?}");
    assert_eq!(
        violations[0].kind,
        QaViolationKind::MobileTruncatedOrOutOfViewport
    );
    assert_eq!(violations[0].selector, "com.centinela:id/bad_text");
    assert_eq!(
        violations[1].kind,
        QaViolationKind::MobileOverlappingInteractive
    );
    assert!(violations[1].selector.contains("confirm_btn"));
    assert!(violations[1].selector.contains("cancel_btn"));
}

#[test]
fn logcat_detects_fatal_exception_and_anr() {
    let logcat = "\
11-21 01:40:00.123  1010  1010 I ActivityManager: Displayed com.centinela.app/.MainActivity
11-21 01:40:02.456  2401  2401 E AndroidRuntime: FATAL EXCEPTION: main
11-21 01:40:02.457  2401  2401 E AndroidRuntime: Process: com.centinela.app, PID: 2401
11-21 01:40:10.789  1010  1055 E ActivityManager: ANR in com.centinela.app (com.centinela.app/.MainActivity)
";

    let violations = scan_logcat(logcat);
    assert_eq!(violations.len(), 2);
    assert_eq!(violations[0].kind, QaViolationKind::MobileFatalException);
    assert_eq!(violations[1].kind, QaViolationKind::MobileAnr);
    assert!(violations[1].message.contains("com.centinela.app"));
}

#[test]
fn roborazzi_diff_and_qa_engine_compressed_report_work_end_to_end() {
    let dir = tempfile::tempdir().expect("create tempdir");
    let roborazzi_dir = dir.path().join("app/build/outputs/roborazzi");
    fs::create_dir_all(&roborazzi_dir).expect("create roborazzi output dir");
    fs::write(roborazzi_dir.join("LoginScreen_compare.png"), b"png-diff")
        .expect("write compare png");
    fs::write(roborazzi_dir.join("HomeScreen.png"), b"png-baseline").expect("write normal png");

    let diffs = scan_roborazzi_diffs(dir.path()).expect("scan roborazzi diffs");
    assert_eq!(diffs.len(), 1);
    assert_eq!(diffs[0].kind, QaViolationKind::RoborazziDiff);

    let engine = QaEngine::new(dir.path());
    let mobile_violations = engine
        .scan_mobile_artifacts(
            Some(r#"<hierarchy><node text="Cut" bounds="[-10,10][100,50]" /></hierarchy>"#),
            (1080, 2400),
            Some("E AndroidRuntime: FATAL EXCEPTION: main\nE ActivityManager: ANR in com.centinela"),
        )
        .expect("scan mobile artifacts");

    assert_eq!(mobile_violations.len(), 4);
    let report = engine.finalize_report(UiPlatform::Mobile, vec![], vec![], mobile_violations, vec![]);
    assert!(!report.passed);

    let tight = engine.format_report(
        &report,
        ReportBudget {
            max_lines: 3,
            max_tokens: 200,
        },
    );
    assert!(tight.contains("QA [FAIL] platform=mobile"));
    assert!(tight.contains("truncated)"));
}

#[test]
fn uiautomator_ignores_hidden_zero_bounds_and_root_webview() {
    let xml = r#"<?xml version='1.0' encoding='UTF-8' standalone='yes' ?>
<hierarchy rotation="0">
  <node index="0" text="" class="android.widget.FrameLayout" clickable="false" bounds="[0,0][1080,2400]">
    <node index="0" text="" class="android.webkit.WebView" clickable="true" enabled="true" bounds="[0,0][1080,2400]">
      <node index="0" text="Hidden aria node" class="android.view.View" clickable="false" bounds="[0,0][0,0]" />
    </node>
    <node index="1" text="Action" resource-id="com.app:id/btn" class="android.widget.Button" clickable="true" enabled="true" bounds="[800,2000][1020,2140]" />
  </node>
</hierarchy>"#;

    let violations = scan_uiautomator_xml(xml, 1080, 2400);
    assert!(violations.is_empty(), "expected 0 false positives, got: {violations:?}");
}
