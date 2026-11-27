//! Running one step: interpolate, call the API or the database, judge, capture.

use std::time::Instant;

use anyhow::{Context, Result, anyhow, bail};
use serde_json::Value;

use super::db::{DbClient, SELECT_TOOL, select};
use super::eval::{Mismatch, capture_values, check_db, check_http};
use super::http::{self, Payload, Request};
use super::interpolate::{self, Vars};
use super::report::StepReport;
use super::spec::{DbStep, Expect, HttpStep, Step};

/// What a step may use: the API base URL and the database.
pub struct StepContext<'a> {
    pub base_url: Option<&'a str>,
    pub db: &'a dyn DbClient,
}

/// Runs `step`; captured values are added to `vars` only when the step passes.
pub fn run_step(ctx: &StepContext<'_>, vars: &mut Vars, index: usize, step: &Step) -> StepReport {
    let started = Instant::now();
    let mut report = StepReport::start(index, step);
    let result = match (&step.http, &step.db) {
        (Some(call), _) => http_step(ctx, vars, &mut report, step, call),
        (_, Some(query)) => db_step(ctx, vars, &mut report, step, query),
        _ => Err(anyhow!("step has neither `http` nor `db`")),
    };
    match result {
        Ok(None) => {}
        Ok(Some(mismatch)) => report.fail(mismatch.message(), mismatch.actual),
        Err(err) => report.fail(format!("{err:#}"), None),
    }
    report.duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    report
}

fn http_step(
    ctx: &StepContext<'_>,
    vars: &mut Vars,
    report: &mut StepReport,
    step: &Step,
    call: &HttpStep,
) -> Result<Option<Mismatch>> {
    let request = build_request(ctx, vars, call)?;
    report.summary = format!("{} {}", request.method.to_ascii_uppercase(), call.path);
    let response = http::send(&request)?;
    report.summary = format!("{} -> {}", report.summary, response.status);
    let expect = expectation(step, vars)?;
    if let Some(mismatch) = check_http(&expect, &response) {
        return Ok(Some(mismatch));
    }
    capture(vars, report, step, response.json.as_ref(), "response body")?;
    Ok(None)
}

fn build_request(ctx: &StepContext<'_>, vars: &Vars, call: &HttpStep) -> Result<Request> {
    let url = http::resolve_url(ctx.base_url, &interpolate::text(&call.path, vars)?)?;
    let headers = call
        .headers
        .iter()
        .map(|(name, value)| Ok((name.clone(), interpolate::text(value, vars)?)))
        .collect::<Result<_>>()?;
    let payload = match (&call.json, &call.body) {
        (Some(json), _) => Payload::Json(interpolate::value(json, vars)?),
        (_, Some(body)) => Payload::Text(interpolate::text(body, vars)?),
        _ => Payload::None,
    };
    Ok(Request { method: call.method.clone(), url, headers, payload })
}

fn db_step(
    ctx: &StepContext<'_>,
    vars: &mut Vars,
    report: &mut StepReport,
    step: &Step,
    query: &DbStep,
) -> Result<Option<Mismatch>> {
    let rows = match (&query.tool, &query.sql) {
        (_, Some(sql)) => select(ctx.db, &interpolate::sql(sql, vars)?)?,
        (Some(tool), _) => invoke_tool(ctx, vars, tool, query.params.as_ref())?,
        _ => bail!("`db` needs a `tool` or a `sql`"),
    };
    let tool = query.tool.as_deref().unwrap_or(SELECT_TOOL);
    report.summary = format!("db {tool} -> {} row(s)", rows.len());
    let expect = expectation(step, vars)?;
    if let Some(mismatch) = check_db(&expect, &rows) {
        return Ok(Some(mismatch));
    }
    capture(vars, report, step, Some(&Value::Array(rows)), "rows")?;
    Ok(None)
}

fn invoke_tool(
    ctx: &StepContext<'_>,
    vars: &Vars,
    tool: &str,
    params: Option<&Value>,
) -> Result<Vec<Value>> {
    let params = params.map_or(Ok(Value::Null), |p| interpolate::value(p, vars))?;
    if tool == SELECT_TOOL {
        // Naming the managed tool directly must not be a way around the read-only rule.
        let sql = params.get("sql").and_then(Value::as_str);
        return select(ctx.db, sql.context("`anvil_select` needs `params.sql`")?);
    }
    ctx.db.invoke(tool, &params)
}

/// The step's expectations with every `{{variable}}` filled in.
fn expectation(step: &Step, vars: &Vars) -> Result<Expect> {
    let Some(expect) = &step.expect else { return Ok(Expect::default()) };
    let json = expect
        .json
        .iter()
        .map(|(pointer, want)| Ok((pointer.clone(), interpolate::value(want, vars)?)))
        .collect::<Result<_>>()?;
    let headers = expect
        .headers
        .iter()
        .map(|(name, part)| Ok((name.clone(), interpolate::text(part, vars)?)))
        .collect::<Result<_>>()?;
    let optional = |value: &Option<Value>| value.as_ref().map(|v| interpolate::value(v, vars));
    Ok(Expect {
        status: expect.status,
        json,
        body_contains: expect
            .body_contains
            .as_deref()
            .map(|t| interpolate::text(t, vars))
            .transpose()?,
        headers,
        rows: expect.rows,
        row: optional(&expect.row).transpose()?,
        contains: optional(&expect.contains).transpose()?,
    })
}

fn capture(
    vars: &mut Vars,
    report: &mut StepReport,
    step: &Step,
    source: Option<&Value>,
    what: &str,
) -> Result<()> {
    for (name, value) in capture_values(&step.capture, source, what)? {
        report.captured.insert(name.clone(), value.clone());
        vars.insert(name, value);
    }
    Ok(())
}
