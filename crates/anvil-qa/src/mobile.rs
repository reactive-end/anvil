//! Android `uiautomator` fast linear XML scan, `logcat` scanner, and Roborazzi diff detector.

mod logcat;
mod roborazzi;

use crate::model::{QaViolation, QaViolationKind, Rect};

pub use logcat::scan_logcat;
pub use roborazzi::scan_roborazzi_diffs;

#[derive(Debug, Clone, Copy)]
struct Bounds {
    x1: i32,
    y1: i32,
    x2: i32,
    y2: i32,
}

impl Bounds {
    fn to_rect(self) -> Rect {
        Rect {
            x: self.x1,
            y: self.y1,
            width: (self.x2 - self.x1).max(0),
            height: (self.y2 - self.y1).max(0),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct AncestorFrame {
    scrollable: bool,
    bounds: Option<Bounds>,
    clickable_idx: Option<usize>,
}

struct ClickableNode {
    selector: String,
    bounds: Bounds,
    enter_seq: usize,
    exit_seq: usize,
}

struct XmlScanState {
    viewport_width: i32,
    viewport_height: i32,
    seq: usize,
    stack: Vec<AncestorFrame>,
    clickables: Vec<ClickableNode>,
    violations: Vec<QaViolation>,
}

impl XmlScanState {
    fn new(viewport_width: i32, viewport_height: i32) -> Self {
        Self {
            viewport_width,
            viewport_height,
            seq: 0,
            stack: Vec::with_capacity(16),
            clickables: Vec::new(),
            violations: Vec::new(),
        }
    }

    fn handle_close_node(&mut self) {
        self.seq += 1;
        if let Some(frame) = self.stack.pop()
            && let Some(idx) = frame.clickable_idx
        {
            self.clickables[idx].exit_seq = self.seq;
        }
    }

    fn handle_open_node(&mut self, tag: &str) {
        self.seq += 1;
        let self_closing = tag.ends_with('/');
        let scrollable = extract_attr(tag, "scrollable") == Some("true");
        let has_scrollable_ancestor = scrollable || self.stack.iter().any(|f| f.scrollable);
        let bounds = extract_attr(tag, "bounds").and_then(parse_bounds);
        let selector = node_selector(tag);

        if let Some(b) = bounds {
            let parent_bounds = self.stack.last().and_then(|f| f.bounds);
            self.check_text_clipping(tag, &selector, b, has_scrollable_ancestor, parent_bounds);
        }

        let clickable_idx = register_clickable(tag, selector, bounds, self.seq, &mut self.clickables);
        if self_closing {
            self.seq += 1;
            if let Some(idx) = clickable_idx {
                self.clickables[idx].exit_seq = self.seq;
            }
        } else {
            self.stack.push(AncestorFrame { scrollable, bounds, clickable_idx });
        }
    }

    fn check_text_clipping(&mut self, tag: &str, selector: &str, b: Bounds, has_scrollable_ancestor: bool, parent_bounds: Option<Bounds>) {
        let text = extract_attr(tag, "text").unwrap_or("").trim();
        if text.is_empty() || has_scrollable_ancestor {
            return;
        }
        let collapsed = b.x2 <= b.x1 || b.y2 <= b.y1;
        let out_of_viewport = b.x1 < 0 || b.y1 < 0 || b.x2 > self.viewport_width || b.y2 > self.viewport_height;
        let clipped_by_parent = parent_bounds.is_some_and(|p| b.x1 < p.x1 || b.y1 < p.y1 || b.x2 > p.x2 || b.y2 > p.y2);

        if collapsed || out_of_viewport || clipped_by_parent {
            self.violations.push(QaViolation {
                kind: QaViolationKind::MobileTruncatedOrOutOfViewport,
                selector: selector.to_owned(),
                message: format!("text `{text}` clipped/out-of-bounds [{},{}][{},{}] without scrollable ancestor", b.x1, b.y1, b.x2, b.y2),
                rect: Some(b.to_rect()),
                viewport: Some("android".to_owned()),
            });
        }
    }
}

/// Scans Android `uiautomator dump` XML in a single linear pass for clipped/out-of-viewport text
/// and overlapping clickable controls.
pub fn scan_uiautomator_xml(xml: &str, viewport_width: i32, viewport_height: i32) -> Vec<QaViolation> {
    let mut state = XmlScanState::new(viewport_width, viewport_height);
    let mut pos = 0usize;

    while let Some(rel_lt) = xml[pos..].find('<') {
        let start = pos + rel_lt;
        let Some(rel_gt) = xml[start..].find('>') else { break; };
        let end = start + rel_gt;
        let tag = &xml[start + 1..end];
        pos = end + 1;

        if tag.starts_with("/node") {
            state.handle_close_node();
        } else if tag.starts_with("node") && tag.as_bytes().get(4).is_none_or(|b| b.is_ascii_whitespace()) {
            state.handle_open_node(tag);
        }
    }

    detect_clickable_overlaps(&state.clickables, &mut state.violations);
    state.violations
}

fn register_clickable(
    tag: &str,
    selector: String,
    bounds: Option<Bounds>,
    enter_seq: usize,
    clickables: &mut Vec<ClickableNode>,
) -> Option<usize> {
    let is_clickable = extract_attr(tag, "clickable") == Some("true");
    let is_enabled = extract_attr(tag, "enabled") != Some("false");
    let b = bounds?;
    if !is_clickable || !is_enabled || b.x2 <= b.x1 || b.y2 <= b.y1 {
        return None;
    }
    let idx = clickables.len();
    clickables.push(ClickableNode {
        selector,
        bounds: b,
        enter_seq,
        exit_seq: enter_seq,
    });
    Some(idx)
}

fn detect_clickable_overlaps(clickables: &[ClickableNode], violations: &mut Vec<QaViolation>) {
    for (i, a) in clickables.iter().enumerate() {
        for b in &clickables[i + 1..] {
            if a.enter_seq < b.enter_seq && b.enter_seq < a.exit_seq {
                continue;
            }
            let ix = a.bounds.x2.min(b.bounds.x2) - a.bounds.x1.max(b.bounds.x1);
            let iy = a.bounds.y2.min(b.bounds.y2) - a.bounds.y1.max(b.bounds.y1);
            if ix > 1 && iy > 1 {
                violations.push(QaViolation {
                    kind: QaViolationKind::MobileOverlappingInteractive,
                    selector: format!("{} <-> {}", a.selector, b.selector),
                    message: format!("clickable Android bounds overlap by {ix}x{iy}px"),
                    rect: Some(a.bounds.to_rect()),
                    viewport: Some("android".to_owned()),
                });
            }
        }
    }
}

fn extract_attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let bytes = tag.as_bytes();
    let mut search_from = 0usize;
    while let Some(rel) = tag[search_from..].find(name) {
        let start = search_from + rel;
        let preceded_by_ws = start > 0 && bytes[start - 1].is_ascii_whitespace();
        let after_name = start + name.len();
        if preceded_by_ws && tag[after_name..].starts_with("=\"") {
            let val_start = after_name + 2;
            if let Some(val_end) = tag[val_start..].find('"') {
                return Some(&tag[val_start..val_start + val_end]);
            }
            return None;
        }
        search_from = start + name.len();
    }
    None
}

fn parse_bounds(raw: &str) -> Option<Bounds> {
    let inner = raw.strip_prefix('[')?.strip_suffix(']')?;
    let (left, right) = inner.split_once("][")?;
    let (x1, y1) = left.split_once(',')?;
    let (x2, y2) = right.split_once(',')?;
    Some(Bounds {
        x1: x1.parse().ok()?,
        y1: y1.parse().ok()?,
        x2: x2.parse().ok()?,
        y2: y2.parse().ok()?,
    })
}

fn node_selector(tag: &str) -> String {
    if let Some(res_id) = extract_attr(tag, "resource-id").filter(|s| !s.is_empty()) {
        return res_id.to_owned();
    }
    if let Some(desc) = extract_attr(tag, "content-desc").filter(|s| !s.is_empty()) {
        return desc.to_owned();
    }
    extract_attr(tag, "class")
        .unwrap_or("node")
        .rsplit('.')
        .next()
        .unwrap_or("node")
        .to_owned()
}
