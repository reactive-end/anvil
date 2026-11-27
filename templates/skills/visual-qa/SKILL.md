---
name: visual-qa
description: Hybrid programmatic DOM/View-Tree checks first, interactive driver onboarding, and before/after visual inspection.
---

# Hybrid Visual QA Workflow

Run visual QA whenever a change touches UI (`Ui` or `StyleOnly` diff category) or `anvil check --full` is requested.

## 1. Interactive Driver Onboarding

Check `.harness/config.json` (`qa_driver.web` and `qa_driver.mobile`). If the relevant driver is `unconfigured`, **ask the user interactively** which driver to enable before proceeding:
- **Web**: `chrome_dev_tools` (Chrome DevTools MCP) | `playwright` | `browser` (built-in browser) | `skip`
- **Mobile**: `adb_mobile_mcp` (Android Emulator `adb` + `mobile-mcp`) | `roborazzi` (Kotlin/Compose screenshot diffs) | `maestro` | `skip`

Persist the chosen driver in `.harness/config.json` so subsequent runs are automatic.

## 2. Mandatory Programmatic Checks (Before Screenshots)

Never rely on screenshots alone. Run objective checks first via `anvil qa`:
- **Web (DOM/CDP)**:
  - `console.error` and `pageerror` count == `0`
  - No unexpected horizontal scroll (`scrollWidth <= clientWidth`)
  - No clipped/overflowing text outside scrollable containers without intentional `text-overflow: ellipsis`
  - No overlapping interactive bounding boxes (`button`, `a`, `input`, `[role="button"]`)
  - No collapsed `0x0` visible controls and no broken images (`naturalWidth === 0`)
- **Mobile (View-Tree / Logcat / Permissions)**:
  - `adb logcat` contains zero `FATAL EXCEPTION` or `ANR`
  - `uiautomator dump` / `mobile-mcp` shows no truncated labels or off-viewport interactive nodes
  - No overlapping interactive bounds; verify Roborazzi screenshot diffs when configured
  - **Never assume runtime permissions (`POST_NOTIFICATIONS`, `CAMERA`, `LOCATION`, etc.) are pre-granted**: never mask missing prompts with `adb pm grant` before verifying that cold start requests the permission *after* window focus and handles denial/settings fallback (`Settings.ACTION_APP_NOTIFICATION_SETTINGS`)
  - When bridging or inspecting webviews/SPAs, inspect actual DOM & open `shadowRoot` trees first instead of assuming light-DOM `document.body.innerText`

## 3. Behavioural Verification of Web Changes

**Never verify UI by reading code alone.** Open the running app and confirm what it does:
- **Interactive checks** (`qa_driver.web` is `chrome_dev_tools`; Anvil registers the `chrome-devtools` MCP server and grants it to executors): call `take_snapshot` (accessibility tree) before any screenshot, then `take_screenshot`; read `list_console_messages` for errors; use `list_network_requests` / `get_network_request` to confirm the page made the expected calls (method, URL, status, payload) and none it should not; record a performance trace for Core Web Vitals when the change affects loading or layout.
- **Repeatable checks**: describe the journey as a flow in `.harness/qa/flows/<name>.json` and run `anvil qa run --flow <name>` (or `--all-flows`, `--start` to boot the app from `qa.web.start_command`). A flow asserts requests (`expect_request` with method, URL part, and status; `expect_no_request`), text, visibility, counts, and screenshots. `anvil qa flows` validates the files; `anvil qa baseline` stores screenshots as baselines and later runs report pixel diffs above `qa.web.diff_threshold`. Exit code 1 means a failed expectation, page error, console error, or visual diff: fix the cause, not the flow, unless the behaviour change is intended.
- If the app is not running, start it (`qa.web.start_command`) before declaring the UI verified; say so explicitly when it cannot be run.

## 4. Directed Visual Inspection

Capture **before/after** screenshots across desktop and mobile viewports and evaluate them alongside the compressed JSON assertion report from `anvil qa`.
