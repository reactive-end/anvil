// Anvil QA flow runner: executes one flow with Playwright and prints what the browser observed as JSON.
// The Rust side fills in the plan constant below and judges the expectations afterwards.
import fs from "node:fs";
import path from "node:path";
import { createRequire } from "node:module";

const plan = __ANVIL_FLOW_PLAN__;

async function loadPlaywright() {
  const misses = [];
  for (const dir of plan.searchDirs) {
    try {
      return createRequire(path.join(dir, "package.json"))("playwright");
    } catch (error) {
      misses.push(`${dir}: ${firstLine(error)}`);
    }
  }
  try {
    return await import("playwright");
  } catch (error) {
    misses.push(`default resolution: ${firstLine(error)}`);
    throw new Error(
      `Playwright was not found (${misses.join(" | ")}); run \`npm i -D playwright\` and \`npx playwright install chromium\``,
    );
  }
}

function firstLine(error) {
  return String(error && error.message ? error.message : error).split("\n")[0].trim();
}

const { chromium } = await loadPlaywright();
fs.mkdirSync(plan.outDir, { recursive: true });

const out = { steps: [], network: [], console: [], page_errors: [], screenshots: [], failure_screenshot: null };
const live = new Map();
let seq = 0;

const browser = await chromium.launch({ headless: true });
try {
  const context = await browser.newContext({ viewport: plan.viewport });
  const page = await context.newPage();
  page.setDefaultTimeout(plan.timeoutMs);

  page.on("request", (request) => {
    if (request.url().startsWith("data:")) return;
    const entry = {
      seq: ++seq,
      method: request.method(),
      url: request.url(),
      status: null,
      resource_type: request.resourceType(),
      duration_ms: null,
      failure: null,
    };
    live.set(request, { entry, started: Date.now(), done: false });
    out.network.push(entry);
  });
  page.on("response", (response) => {
    const tracked = live.get(response.request());
    if (tracked) tracked.entry.status = response.status();
  });
  const finish = (request, failure) => {
    const tracked = live.get(request);
    if (!tracked) return;
    tracked.done = true;
    tracked.entry.duration_ms = Date.now() - tracked.started;
    if (failure && tracked.entry.status === null) tracked.entry.failure = failure;
  };
  page.on("requestfinished", (request) => finish(request, null));
  page.on("requestfailed", (request) => finish(request, request.failure()?.errorText ?? "request failed"));
  page.on("console", (msg) => {
    if (msg.type() === "error") out.console.push({ text: msg.text(), url: msg.location().url || null });
  });
  page.on("pageerror", (err) => out.page_errors.push(String(err)));

  const settled = () => [...live.values()].every((tracked) => tracked.done);
  const quiet = () =>
    page
      .waitForLoadState("networkidle", { timeout: Math.min(plan.timeoutMs, 5000) })
      .then(() => true, () => false);
  const until = async (predicate) => {
    const end = Date.now() + plan.timeoutMs;
    while (Date.now() < end) {
      if (await predicate()) return true;
      await page.waitForTimeout(50);
    }
    return predicate();
  };
  const appears = async (locator) => {
    try {
      await locator.first().waitFor({ state: "visible", timeout: plan.timeoutMs });
      return true;
    } catch {
      return false;
    }
  };
  const requestSeen = (step, since) =>
    [...live.values()].some(
      (t) =>
        t.entry.seq >= since &&
        (!step.method || t.entry.method.toUpperCase() === step.method) &&
        t.entry.url.includes(step.urlContains) &&
        (t.done || t.entry.status !== null),
    );

  let since = 1;
  const startWindow = () => {
    since = seq + 1;
  };

  async function runStep(step, rec) {
    switch (step.kind) {
      case "goto":
        startWindow();
        rec.since = since;
        await page.goto(step.url, { waitUntil: "load" });
        await quiet();
        break;
      case "wait_for":
        await page.locator(step.selector).first().waitFor({ state: "visible" });
        break;
      case "fill":
        await page.locator(step.selector).fill(step.value);
        break;
      case "select":
        await page
          .locator(step.selector)
          .selectOption(step.index !== null ? { index: step.index } : step.value);
        break;
      case "click":
        startWindow();
        rec.since = since;
        await page.locator(step.selector).first().click();
        break;
      case "press":
        startWindow();
        rec.since = since;
        if (step.selector) await page.locator(step.selector).press(step.key);
        else await page.keyboard.press(step.key);
        break;
      case "wait_ms":
        await page.waitForTimeout(step.ms);
        break;
      case "expect_request":
        await until(() => requestSeen(step, since));
        break;
      case "expect_no_request":
        await quiet();
        await until(settled);
        break;
      case "expect_text":
        rec.found = await appears(page.getByText(step.text));
        break;
      case "expect_visible":
        rec.visible = await appears(page.locator(step.selector));
        break;
      case "expect_count": {
        const locator = page.locator(step.selector);
        await until(async () => (await locator.count()) === step.count);
        rec.count = await locator.count();
        break;
      }
      case "screenshot": {
        const file = path.join(plan.outDir, `${step.name}.png`);
        const mask = step.mask.map((selector) => page.locator(selector));
        const options = { path: file, mask, animations: "disabled", caret: "hide" };
        if (step.selector) await page.locator(step.selector).first().screenshot(options);
        else await page.screenshot({ ...options, fullPage: step.fullPage });
        out.screenshots.push({ name: step.name, path: file });
        break;
      }
      default:
        throw new Error(`unknown step kind ${step.kind}`);
    }
  }

  try {
    if (plan.startUrl) {
      await page.goto(plan.startUrl, { waitUntil: "load" });
      await quiet();
    }
  } catch (error) {
    out.steps.push({ index: 0, ok: false, error: `open ${plan.startUrl}: ${firstLine(error)}`, since });
    plan.steps = [];
  }

  for (const [index, step] of plan.steps.entries()) {
    const rec = { index, ok: true, since };
    out.steps.push(rec);
    try {
      await runStep(step, rec);
      rec.until = seq;
    } catch (error) {
      rec.ok = false;
      rec.error = `${step.kind}: ${firstLine(error)}`;
      const shot = path.join(plan.outDir, "failure.png");
      try {
        await page.screenshot({ path: shot });
        out.failure_screenshot = shot;
      } catch (shotError) {
        rec.error += ` (no failure screenshot: ${firstLine(shotError)})`;
      }
      break;
    }
  }
  await context.close();
} finally {
  await browser.close();
}
console.log(JSON.stringify(out));
