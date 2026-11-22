---
name: visual-qa
description: Hybrid programmatic DOM/View-Tree checks first, interactive driver onboarding, and before/after visual inspection.
---

# Hybrid Visual QA Workflow

Run visual QA whenever a change touches UI (`Ui` or `StyleOnly` diff category) or `anvil check --full` is requested.

## 1. Interactive Driver Onboarding

Check `.harness/config.json` (`qa_driver.web` and `qa_driver.mobile`). If the relevant driver is `unconfigured`, **ask the user interactively** which driver to enable before proceeding:
- **Web**: `chrome_devtools` (Chrome DevTools MCP) | `playwright` | `browser` (built-in browser) | `skip`
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

## 3. Directed Visual Inspection

Capture **before/after** screenshots across desktop and mobile viewports and evaluate them alongside the compressed JSON assertion report from `anvil qa`.
