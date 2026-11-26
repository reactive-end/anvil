use anvil_codemode::{CodeModePolicy, ToolDispatcher, execute_script};
use anyhow::{Result, bail};
use serde_json::{Value, json};

struct MockDispatcher {
    records: Vec<Value>,
}

impl ToolDispatcher for MockDispatcher {
    fn call(&mut self, name: &str, args: Value) -> Result<Value> {
        match name {
            "list_items" => Ok(json!(self.records)),
            "inspect_item" => {
                let id = args.get("id").and_then(Value::as_u64).unwrap_or(0);
                Ok(json!({ "id": id, "detail": format!("detail-payload-for-{id}-verbose-text") }))
            }
            other => bail!("unexpected tool: {other}"),
        }
    }
}

#[test]
fn executes_multistep_filtering_and_tracks_tokens_saved() {
    let records = (0..40)
        .map(|i| json!({ "id": i, "blocked": i % 10 == 0, "padding": "x".repeat(40) }))
        .collect();
    let mut disp = MockDispatcher { records };
    let script = r#"
        const rows = anvil.list_items({});
        const blocked = rows.filter(r => r.blocked);
        return blocked.map(r => anvil.inspect_item({ id: r.id }).id);
    "#;
    let res = execute_script(
        script,
        &["list_items", "inspect_item"],
        &mut disp,
        &CodeModePolicy::default(),
    )
    .unwrap();
    assert_eq!(res.value, json!([0, 10, 20, 30]));
    assert_eq!(res.calls_executed, 5);
    assert!(res.intermediate_bytes > res.output_bytes);
    assert!(res.estimated_tokens_saved > 100);
}

#[test]
fn enforces_loop_call_output_and_allowlist_limits() {
    let mut disp = MockDispatcher { records: vec![] };
    let loop_err = execute_script(
        "while (true) {}",
        &["list_items"],
        &mut disp,
        &CodeModePolicy { max_loop_iterations: 500, ..Default::default() },
    );
    assert!(loop_err.is_err());

    let call_err = execute_script(
        "for (let i = 0; i < 5; i++) { anvil.list_items({}); } return 1;",
        &["list_items"],
        &mut disp,
        &CodeModePolicy { max_calls: 3, ..Default::default() },
    );
    assert!(call_err.is_err());

    let denied_err = execute_script(
        "return anvil.call('secret_delete', {});",
        &["list_items"],
        &mut disp,
        &CodeModePolicy::default(),
    );
    assert!(denied_err.is_err());
}
