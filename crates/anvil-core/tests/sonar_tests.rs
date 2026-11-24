use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;

use anvil_core::sonar::{
    check_cognitive_complexity, check_new_code_duplication, fetch_new_code_issues,
    fetch_quality_gate, parse_sonar_properties,
};
use tempfile::TempDir;

#[test]
fn parses_sonar_project_properties() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(
        tmp.path().join("sonar-project.properties"),
        "# comment\nsonar.projectKey=demo-app\nsonar.host.url=http://localhost:9000/\nsonar.sources=src\n",
    )
    .unwrap();
    let props = parse_sonar_properties(tmp.path()).unwrap();
    assert_eq!(props.project_key.as_deref(), Some("demo-app"));
    assert_eq!(props.host_url.as_deref(), Some("http://localhost:9000"));
    assert_eq!(props.sources.as_deref(), Some("src"));
}

#[test]
fn cognitive_complexity_flags_deeply_nested_functions_and_honors_allow() {
    let noisy = r#"
fn deepCheck(a: bool, b: bool, c: bool) -> i32 {
    if a {
        for i in 0..3 {
            if b && c {
                while i > 0 {
                    if a || b {
                        return 1;
                    }
                }
            }
        }
    }
    0
}
"#;
    let diags = check_cognitive_complexity("src/logic.rs", noisy, 5);
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].rule, "sonar:cognitive-complexity");

    let allowed = format!("// anvil-allow(sonar:cognitive-complexity): legacy\n{noisy}");
    assert!(check_cognitive_complexity("src/logic.rs", &allowed, 5).is_empty());

    // Python indentation-based nesting must score > 5 instead of staying at nesting=0.
    let py_nested = "\ndef deep_py(a, b, c):\n    if a:\n        for x in b:\n            if x and c:\n                while x > 0:\n                    if c or a:\n                        return 1\n    return 0\n";
    let py_diags = check_cognitive_complexity("src/worker.py", py_nested, 5);
    assert_eq!(py_diags.len(), 1, "python nesting must be scored accurately");

    // TS exported arrow function must be detected, while strings must NOT inflate complexity.
    let ts_arrow = "\nexport const deepArrow = (a: boolean, b: boolean) => {\n    const s = \"{ if (a && b) || c ? 1 : 2 }\";\n    if (a) {\n        for (let i = 0; i < 3; i++) {\n            if (b && s.length > 0) {\n                return 1;\n            }\n        }\n    }\n    return 0;\n};\n";
    assert_eq!(check_cognitive_complexity("src/arrow.ts", ts_arrow, 5).len(), 1);
    let ts_clean_str = "\nexport const okFn = () => {\n    const text = \"{ if (a && b) { for (x) { if (y) { ? : } } } }\";\n    return text;\n};\n";
    assert!(check_cognitive_complexity("src/clean.ts", ts_clean_str, 1).is_empty());
}

#[test]
fn duplication_detects_repeated_block_across_changed_files() {
    let block = (0..6)
        .map(|i| format!("let v{i} = compute(step_{i}) + offset;"))
        .collect::<Vec<_>>()
        .join("\n");
    let files = vec![
        ("src/a.ts".to_owned(), format!("export function f1() {{\n{block}\n}}")),
        ("src/b.ts".to_owned(), format!("export function f2() {{\n{block}\n}}")),
    ];
    let diags = check_new_code_duplication(&files, 6);
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].rule, "sonar:duplication");
}

#[test]
fn web_api_client_parses_quality_gate_and_issues_with_basic_auth() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = thread::spawn(move || {
        for i in 0..2 {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 1024];
            let n = stream.read(&mut buf).unwrap();
            let req = String::from_utf8_lossy(&buf[..n]);
            assert!(req.contains("Authorization: Basic "));
            let body = if i == 0 {
                r#"{"projectStatus":{"status":"ERROR","conditions":[{"status":"ERROR","metricKey":"new_bugs","comparator":"GT","errorThreshold":"0","actualValue":"2"}]}}"#
            } else {
                r#"{"issues":[{"key":"AX1","rule":"rust:S3776","severity":"CRITICAL","component":"demo:src/lib.rs","line":42,"message":"Refactor this function"}]}"#
            };
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(resp.as_bytes()).unwrap();
        }
    });

    let host = format!("http://{addr}");
    let gate = fetch_quality_gate(&host, "demo", Some("main"), Some("tok123")).unwrap();
    assert_eq!(gate.status, "ERROR");
    assert_eq!(gate.conditions[0].metric_key, "new_bugs");

    let issues = fetch_new_code_issues(&host, "demo", None, Some("tok123"), 10).unwrap();
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].rule, "rust:S3776");
    handle.join().unwrap();
}
