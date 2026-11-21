(() => {
  "use strict";

  const win = typeof window !== "undefined" ? window : globalThis;
  const doc = typeof document !== "undefined" ? document : null;
  if (!doc) {
    return {
      passed: false,
      violations: [
        {
          kind: "page_error",
          selector: "document",
          message: "document is not available in execution context",
          rect: null,
        },
      ],
    };
  }

  const getStyle =
    typeof getComputedStyle === "function"
      ? getComputedStyle
      : win && typeof win.getComputedStyle === "function"
        ? win.getComputedStyle.bind(win)
        : () => ({});

  const violations = [];

  function selectorOf(el) {
    if (!el || !el.tagName) return "unknown";
    let sel = String(el.tagName).toLowerCase();
    if (el.id) {
      sel += "#" + String(el.id);
    }
    const cls = typeof el.className === "string" ? el.className.trim() : "";
    if (cls) {
      const parts = cls.split(/\s+/).slice(0, 2);
      if (parts.length > 0 && parts[0]) {
        sel += "." + parts.join(".");
      }
    }
    return sel;
  }

  function toRect(r) {
    if (!r) return null;
    const x = Math.round(r.left !== undefined ? r.left : r.x || 0);
    const y = Math.round(r.top !== undefined ? r.top : r.y || 0);
    const width = Math.round(r.width !== undefined ? r.width : (r.right || 0) - x);
    const height = Math.round(r.height !== undefined ? r.height : (r.bottom || 0) - y);
    return { x, y, width, height };
  }

  function boundsOf(el) {
    const r = typeof el.getBoundingClientRect === "function" ? el.getBoundingClientRect() : null;
    if (!r) {
      return { left: 0, top: 0, right: 0, bottom: 0, width: 0, height: 0 };
    }
    const left = r.left !== undefined ? r.left : r.x || 0;
    const top = r.top !== undefined ? r.top : r.y || 0;
    const width = r.width !== undefined ? r.width : (r.right || 0) - left;
    const height = r.height !== undefined ? r.height : (r.bottom || 0) - top;
    const right = r.right !== undefined ? r.right : left + width;
    const bottom = r.bottom !== undefined ? r.bottom : top + height;
    return { left, top, right, bottom, width, height };
  }

  function isHidden(style) {
    if (!style) return false;
    return (
      style.display === "none" ||
      style.visibility === "hidden" ||
      style.opacity === "0"
    );
  }

  function getAttr(el, name) {
    if (!el) return null;
    if (typeof el.getAttribute === "function") {
      const val = el.getAttribute(name);
      if (val !== null && val !== undefined) return String(val);
    }
    if (el.attributes && el.attributes[name] !== undefined) {
      return String(el.attributes[name]);
    }
    return null;
  }

  function hasHiddenAttr(el) {
    if (!el) return false;
    if (el.hidden === true) return true;
    if (typeof el.hasAttribute === "function" && el.hasAttribute("hidden")) return true;
    return Boolean(el.attributes && Object.prototype.hasOwnProperty.call(el.attributes, "hidden"));
  }

  function isEffectivelyVisible(el) {
    let cur = el;
    while (cur) {
      if (hasHiddenAttr(cur)) {
        return false;
      }
      if (isHidden(getStyle(cur) || {})) {
        return false;
      }
      cur = cur.parentElement || null;
    }
    return true;
  }

  function isOverlayRoot(el) {
    if (!el) return false;
    const tag = String(el.tagName || "").toLowerCase();
    if (tag === "html" || tag === "body") return false;
    if (tag === "dialog") return true;
    if (getAttr(el, "role") === "dialog" || el.role === "dialog") return true;
    if (getAttr(el, "aria-modal") === "true" || el.ariaModal === "true") return true;
    const style = getStyle(el) || {};
    return style.position === "fixed";
  }

  function overlayLayerOf(el) {
    let cur = el;
    while (cur && cur !== doc.documentElement) {
      if (isOverlayRoot(cur)) {
        return cur;
      }
      cur = cur.parentElement || null;
    }
    return null;
  }

  function hasScrollableAncestor(el, axis) {
    let cur = el.parentElement;
    while (cur && cur !== doc.documentElement) {
      const st = getStyle(cur) || {};
      const ov = axis === "x" ? st.overflowX || st.overflow : st.overflowY || st.overflow;
      if (ov === "auto" || ov === "scroll") {
        return true;
      }
      cur = cur.parentElement;
    }
    return false;
  }

  function hasOwnText(el) {
    if (el.childNodes && el.childNodes.length > 0) {
      for (let i = 0; i < el.childNodes.length; i++) {
        const child = el.childNodes[i];
        if (child && child.nodeType === 3 && String(child.nodeValue || "").trim().length > 0) {
          return true;
        }
      }
      return false;
    }
    const children = el.children || [];
    return children.length === 0 && String(el.textContent || "").trim().length > 0;
  }

  // 1. Horizontal overflow
  const docEl = doc.documentElement;
  if (docEl && win.innerWidth > 0 && docEl.scrollWidth > win.innerWidth) {
    violations.push({
      kind: "horizontal_overflow",
      selector: "html",
      message: `document scrollWidth (${docEl.scrollWidth}) exceeds innerWidth (${win.innerWidth})`,
      rect: {
        x: 0,
        y: 0,
        width: docEl.scrollWidth,
        height: docEl.scrollHeight || win.innerHeight || 0,
      },
    });
  }

  // 2 & 3. Interactive controls: zero-size & overlapping bounding boxes
  const interactiveSelector = 'button, a, input, [role="button"], select, textarea';
  const interactiveNodes = Array.from(doc.querySelectorAll(interactiveSelector));
  const visibleInteractive = [];

  for (const el of interactiveNodes) {
    if (String(el.tagName).toLowerCase() === "input" && String(el.type || "").toLowerCase() === "hidden") {
      continue;
    }
    if (!isEffectivelyVisible(el)) {
      continue;
    }
    const b = boundsOf(el);
    if (b.width <= 0 || b.height <= 0) {
      violations.push({
        kind: "zero_size_control",
        selector: selectorOf(el),
        message: `visible interactive control collapsed to ${b.width}x${b.height}`,
        rect: toRect(b),
      });
      continue;
    }
    visibleInteractive.push({ el, b, sel: selectorOf(el), layer: overlayLayerOf(el) });
  }

  for (let i = 0; i < visibleInteractive.length; i++) {
    const a = visibleInteractive[i];
    for (let j = i + 1; j < visibleInteractive.length; j++) {
      const b = visibleInteractive[j];
      if (a.layer !== b.layer) {
        continue;
      }
      if (
        (typeof a.el.contains === "function" && a.el.contains(b.el)) ||
        (typeof b.el.contains === "function" && b.el.contains(a.el))
      ) {
        continue;
      }
      const ix = Math.min(a.b.right, b.b.right) - Math.max(a.b.left, b.b.left);
      const iy = Math.min(a.b.bottom, b.b.bottom) - Math.max(a.b.top, b.b.top);
      if (ix > 1 && iy > 1) {
        violations.push({
          kind: "overlapping_interactive",
          selector: `${a.sel} <-> ${b.sel}`,
          message: `interactive controls overlap by ${Math.round(ix)}x${Math.round(iy)}px`,
          rect: toRect(a.b),
        });
      }
    }
  }

  // 4. Broken images
  const images = Array.from(doc.querySelectorAll("img"));
  for (const img of images) {
    if (!isEffectivelyVisible(img)) continue;
    if (img.complete && img.naturalWidth === 0) {
      violations.push({
        kind: "broken_image",
        selector: selectorOf(img),
        message: `broken image (${img.src || "no src"})`,
        rect: toRect(boundsOf(img)),
      });
    }
  }

  // 5. Clipped / overflowing text without scrollable ancestor or intentional ellipsis
  const skipTags = new Set(["html", "body", "script", "style", "svg", "img", "input", "select", "textarea"]);
  const allElements = Array.from(doc.querySelectorAll("*"));
  for (const el of allElements) {
    const tag = String(el.tagName || "").toLowerCase();
    if (!tag || skipTags.has(tag)) continue;
    if (!isEffectivelyVisible(el)) continue;
    const style = getStyle(el) || {};
    const b = boundsOf(el);
    if (b.width <= 0 || b.height <= 0) continue;
    if (!hasOwnText(el)) continue;

    const ox = style.overflowX || style.overflow || "visible";
    const oy = style.overflowY || style.overflow || "visible";
    const overflowX = el.clientWidth > 0 && el.scrollWidth > el.clientWidth + 1;
    const overflowY = el.clientHeight > 0 && el.scrollHeight > el.clientHeight + 1;
    const intentionalEllipsis =
      style.textOverflow === "ellipsis" && (ox === "hidden" || ox === "clip") && !overflowY;

    const xClipped =
      overflowX && ox !== "auto" && ox !== "scroll" && !hasScrollableAncestor(el, "x") && !intentionalEllipsis;
    const yClipped =
      overflowY && oy !== "auto" && oy !== "scroll" && !hasScrollableAncestor(el, "y");

    if (xClipped || yClipped) {
      violations.push({
        kind: "clipped_text",
        selector: selectorOf(el),
        message: `text overflows box (${el.scrollWidth}x${el.scrollHeight} > ${el.clientWidth}x${el.clientHeight})`,
        rect: toRect(b),
      });
    }
  }

  return {
    passed: violations.length === 0,
    violations,
  };
})()
