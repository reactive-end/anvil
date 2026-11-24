//! Pure-Rust isolated JavaScript Code Mode sandbox built on `boa_engine`.
//! Allows LLMs to compose multi-step read-only/allowlisted tool calls inside strict call, loop,
//! recursion, timeout, and output budgets without flooding the prompt context window.

use std::cell::RefCell;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use boa_engine::property::Attribute;
use boa_engine::{Context, JsArgs, JsError, JsNativeError, JsResult, JsValue, NativeFunction, Source, js_string};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Trait implemented by host tool routers (e.g. `anvil-mcp` or `anvil-mcp` proxy gateway).
pub trait ToolDispatcher {
    fn call(&mut self, name: &str, args: Value) -> Result<Value>;
}

/// Execution guardrails enforced on every Code Mode script.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeModePolicy {
    pub max_calls: usize,
    pub max_loop_iterations: u64,
    pub max_recursion: usize,
    pub timeout_ms: u64,
    pub max_output_bytes: usize,
}

impl Default for CodeModePolicy {
    fn default() -> Self {
        Self {
            max_calls: 32,
            max_loop_iterations: 100_000,
            max_recursion: 128,
            timeout_ms: 5_000,
            max_output_bytes: 8_192,
        }
    }
}

/// Result and savings accounting produced by `execute_script`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CodeModeResult {
    pub value: Value,
    pub calls_executed: usize,
    pub intermediate_bytes: usize,
    pub output_bytes: usize,
    pub estimated_tokens_saved: u64,
}

struct BridgeCall {
    tool: String,
    args: Value,
}

struct ActiveBridge {
    req_tx: Sender<BridgeCall>,
    resp_rx: Receiver<Result<Value>>,
    allowed_tools: Vec<String>,
    calls_executed: usize,
    intermediate_bytes: usize,
    max_calls: usize,
    deadline: Instant,
}

thread_local! {
    static ACTIVE_BRIDGE: RefCell<Option<ActiveBridge>> = const { RefCell::new(None) };
}

struct BridgeResetGuard;

impl Drop for BridgeResetGuard {
    fn drop(&mut self) {
        ACTIVE_BRIDGE.with(|cell| {
            *cell.borrow_mut() = None;
        });
    }
}

/// Executes `script` inside a zero-FS, zero-network Boa JS sandbox exposing only `allowed_tools`.
pub fn execute_script(
    script: &str,
    allowed_tools: &[&str],
    dispatcher: &mut dyn ToolDispatcher,
    policy: &CodeModePolicy,
) -> Result<CodeModeResult> {
    if script.trim().is_empty() {
        bail!("Code Mode script cannot be empty");
    }
    let (req_tx, req_rx) = mpsc::channel::<BridgeCall>();
    let (resp_tx, resp_rx) = mpsc::channel::<Result<Value>>();
    let tools: Vec<String> = allowed_tools.iter().map(|s| (*s).to_owned()).collect();
    let policy_cloned = policy.clone();
    let script_owned = script.to_owned();

    let vm_outcome = thread::scope(|scope| {
        let handle = scope.spawn(move || {
            run_js_vm(&script_owned, tools, req_tx, resp_rx, &policy_cloned)
        });
        while let Ok(call) = req_rx.recv() {
            let res = dispatcher.call(&call.tool, call.args);
            if resp_tx.send(res).is_err() {
                break;
            }
        }
        handle
            .join()
            .map_err(|_| anyhow::anyhow!("Code Mode VM thread panicked"))?
    })?;

    let (value, calls_executed, intermediate_bytes) = vm_outcome;
    let output_bytes = serde_json::to_string(&value).map(|s| s.len()).unwrap_or(0);
    if output_bytes > policy.max_output_bytes {
        bail!(
            "Code Mode return payload ({output_bytes} bytes) exceeds max_output_bytes ({}); filter or summarize inside the script",
            policy.max_output_bytes
        );
    }
    let estimated_tokens_saved =
        u64::try_from(intermediate_bytes.saturating_sub(output_bytes)).unwrap_or(0).div_ceil(4);
    Ok(CodeModeResult {
        value,
        calls_executed,
        intermediate_bytes,
        output_bytes,
        estimated_tokens_saved,
    })
}

fn run_js_vm(
    script: &str,
    allowed_tools: Vec<String>,
    req_tx: Sender<BridgeCall>,
    resp_rx: Receiver<Result<Value>>,
    policy: &CodeModePolicy,
) -> Result<(Value, usize, usize)> {
    let deadline = Instant::now() + Duration::from_millis(policy.timeout_ms.max(1));
    ACTIVE_BRIDGE.with(|cell| {
        *cell.borrow_mut() = Some(ActiveBridge {
            req_tx,
            resp_rx,
            allowed_tools: allowed_tools.clone(),
            calls_executed: 0,
            intermediate_bytes: 0,
            max_calls: policy.max_calls,
            deadline,
        });
    });
    let _guard = BridgeResetGuard;

    let mut ctx = Context::default();
    ctx.runtime_limits_mut()
        .set_loop_iteration_limit(policy.max_loop_iterations);
    ctx.runtime_limits_mut()
        .set_recursion_limit(policy.max_recursion);

    let call_fn = NativeFunction::from_copy_closure(host_call_tool).to_js_function(ctx.realm());
    ctx.register_global_property(js_string!("__anvil_call"), call_fn, Attribute::all())
        .map_err(|e| anyhow::anyhow!("register bridge failed: {e}"))?;

    let prelude = build_anvil_namespace_js(&allowed_tools);
    ctx.eval(Source::from_bytes(prelude.as_bytes()))
        .map_err(|e| anyhow::anyhow!("Code Mode prelude error: {e}"))?;

    let wrapped = format!("(function() {{\n{script}\n}})()");
    let result_val = ctx
        .eval(Source::from_bytes(wrapped.as_bytes()))
        .map_err(|e| anyhow::anyhow!("Code Mode execution error: {e}"))?;

    let json_val = result_val
        .to_json(&mut ctx)
        .map_err(|e| anyhow::anyhow!("Code Mode return serialization error: {e}"))?
        .unwrap_or(Value::Null);

    let (calls_executed, intermediate_bytes) = ACTIVE_BRIDGE.with(|cell| {
        cell.borrow()
            .as_ref()
            .map_or((0, 0), |b| (b.calls_executed, b.intermediate_bytes))
    });
    Ok((json_val, calls_executed, intermediate_bytes))
}

fn build_anvil_namespace_js(allowed_tools: &[String]) -> String {
    let mut entries = Vec::with_capacity(allowed_tools.len());
    for tool in allowed_tools {
        if let Ok(lit) = serde_json::to_string(tool)
            && is_valid_js_ident(tool)
        {
            entries.push(format!("{tool}: function(args) {{ return __anvil_call({lit}, args || {{}}); }}"));
        }
    }
    format!(
        "var anvil = Object.freeze({{\n  call: function(name, args) {{ return __anvil_call(String(name), args || {{}}); }},\n  {}\n}});",
        entries.join(",\n  ")
    )
}

fn is_valid_js_ident(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_' || first == '$')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}

fn host_call_tool(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let tool_name = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    let raw_arg = args.get_or_undefined(1);
    let json_arg = if raw_arg.is_undefined() || raw_arg.is_null() {
        Value::Object(Default::default())
    } else {
        raw_arg.to_json(ctx)?.unwrap_or(Value::Object(Default::default()))
    };

    let response = ACTIVE_BRIDGE.with(|cell| -> Result<Value, String> {
        let mut guard = cell.borrow_mut();
        let bridge = guard
            .as_mut()
            .ok_or_else(|| "Code Mode bridge is inactive".to_owned())?;
        if Instant::now() > bridge.deadline {
            return Err("Code Mode execution timed out".to_owned());
        }
        if !bridge.allowed_tools.iter().any(|t| t == &tool_name) {
            return Err(format!("tool `{tool_name}` is not allowlisted in Code Mode"));
        }
        if bridge.calls_executed >= bridge.max_calls {
            return Err(format!(
                "Code Mode call limit ({}) exceeded",
                bridge.max_calls
            ));
        }
        bridge.calls_executed += 1;
        bridge
            .req_tx
            .send(BridgeCall {
                tool: tool_name,
                args: json_arg,
            })
            .map_err(|_| "Code Mode host channel closed".to_owned())?;
        let val = bridge
            .resp_rx
            .recv()
            .map_err(|_| "Code Mode host response closed".to_owned())?
            .map_err(|e| format!("{e:#}"))?;
        let bytes = serde_json::to_string(&val).map(|s| s.len()).unwrap_or(0);
        bridge.intermediate_bytes = bridge.intermediate_bytes.saturating_add(bytes);
        Ok(val)
    });

    let payload = response.map_err(|msg| {
        JsError::from_native(JsNativeError::error().with_message(msg))
    })?;
    JsValue::from_json(&payload, ctx)
}
