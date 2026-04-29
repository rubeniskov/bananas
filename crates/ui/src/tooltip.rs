//! JS-positioned tooltips. Replaces the pure-CSS `[data-tip]::after`
//! approach because pseudo-elements can't see the viewport — they get
//! clipped at the top of the window with no fallback.
//!
//! Pattern: a singleton `<div class="tip-floating">` lives at the end of
//! `<body>`. Document-level `mouseover` / `mouseout` / `focusin` /
//! `focusout` listeners pick up any element with a `data-tip` attribute,
//! compute the element's bounding rect, and place the tooltip with
//! `position: fixed` after picking the side with the most room. The
//! arrow is shifted with a CSS custom property so it still points at
//! the anchor even when the bubble is clamped to the viewport edge.

use std::sync::OnceLock;

use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use web_sys::{Element, HtmlElement};

/// Idempotent: safe to call multiple times. Subsequent calls are no-ops.
pub fn init() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        if let Err(e) = install() {
            tracing::warn!(?e, "tooltip listener install failed");
        }
    });
}

fn install() -> Result<(), JsValue> {
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("no window"))?;
    let document = window
        .document()
        .ok_or_else(|| JsValue::from_str("no document"))?;
    let body = document
        .body()
        .ok_or_else(|| JsValue::from_str("no body"))?;

    // Singleton tooltip element.
    let tip = document.create_element("div")?;
    tip.set_class_name("tip-floating");
    tip.set_attribute("aria-hidden", "true")?;
    body.append_child(&tip)?;

    let tip_for_show = tip.clone();
    let on_enter = Closure::<dyn FnMut(web_sys::Event)>::new(move |evt: web_sys::Event| {
        if let Some(target) = evt.target() {
            if let Ok(el) = target.dyn_into::<Element>() {
                if let Some(anchor) = find_data_tip(&el) {
                    let _ = show_tip(&tip_for_show, &anchor);
                }
            }
        }
    });
    document.add_event_listener_with_callback("mouseover", on_enter.as_ref().unchecked_ref())?;
    document.add_event_listener_with_callback("focusin", on_enter.as_ref().unchecked_ref())?;
    on_enter.forget();

    let tip_for_hide = tip.clone();
    let on_leave = Closure::<dyn FnMut(web_sys::Event)>::new(move |_evt: web_sys::Event| {
        let _ = tip_for_hide.set_attribute("data-show", "0");
    });
    document.add_event_listener_with_callback("mouseout", on_leave.as_ref().unchecked_ref())?;
    document.add_event_listener_with_callback("focusout", on_leave.as_ref().unchecked_ref())?;
    on_leave.forget();

    // Hide on scroll/resize too — the cached bounding rect goes stale otherwise.
    let tip_for_scroll = tip.clone();
    let on_scroll = Closure::<dyn FnMut(web_sys::Event)>::new(move |_evt: web_sys::Event| {
        let _ = tip_for_scroll.set_attribute("data-show", "0");
    });
    window.add_event_listener_with_callback("scroll", on_scroll.as_ref().unchecked_ref())?;
    window.add_event_listener_with_callback("resize", on_scroll.as_ref().unchecked_ref())?;
    on_scroll.forget();

    Ok(())
}

/// Walk up from `el` until we find an ancestor with a `data-tip`
/// attribute, or run out of ancestors. None means "not over a
/// tooltipped element".
fn find_data_tip(el: &Element) -> Option<Element> {
    let mut cur = Some(el.clone());
    while let Some(e) = cur {
        if e.has_attribute("data-tip") {
            return Some(e);
        }
        cur = e.parent_element();
    }
    None
}

fn show_tip(tip: &Element, anchor: &Element) -> Result<(), JsValue> {
    let text = anchor.get_attribute("data-tip").unwrap_or_default();
    if text.is_empty() {
        return Ok(());
    }
    tip.set_text_content(Some(&text));
    tip.set_attribute("data-show", "1")?;

    // Reset transform so getBoundingClientRect reports the natural size.
    let html_tip: HtmlElement = tip.clone().dyn_into()?;
    html_tip.style().set_property("left", "-9999px")?;
    html_tip.style().set_property("top", "-9999px")?;

    let html_anchor: HtmlElement = anchor.clone().dyn_into()?;
    let anchor_rect = html_anchor.get_bounding_client_rect();
    let tip_rect = html_tip.get_bounding_client_rect();
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("no window"))?;
    let vw = window
        .inner_width()
        .ok()
        .and_then(|v| v.as_f64())
        .unwrap_or(1024.0);
    let vh = window
        .inner_height()
        .ok()
        .and_then(|v| v.as_f64())
        .unwrap_or(768.0);

    let tip_w = tip_rect.width();
    let tip_h = tip_rect.height();
    let arrow = 5.0;
    let gap = 8.0;
    let margin = 8.0;
    let needed_v = tip_h + arrow + gap;

    let space_above = anchor_rect.top();
    let space_below = vh - anchor_rect.bottom();

    let pos = if space_above >= needed_v {
        "top"
    } else if space_below >= needed_v {
        "bottom"
    } else if space_above >= space_below {
        // Neither fits perfectly — pick the larger gap.
        "top"
    } else {
        "bottom"
    };

    let center_x = anchor_rect.left() + anchor_rect.width() / 2.0;
    let mut left = center_x - tip_w / 2.0;
    if left < margin {
        left = margin;
    }
    if left + tip_w > vw - margin {
        left = vw - margin - tip_w;
    }
    if left < 0.0 {
        left = 0.0;
    }

    let top = if pos == "top" {
        anchor_rect.top() - tip_h - arrow - gap
    } else {
        anchor_rect.bottom() + arrow + gap
    };

    // arrow_x is the X offset *within* the tooltip where the arrow lives —
    // so it always points at the anchor, even when the tooltip is shifted
    // sideways to fit on-screen.
    let mut arrow_x = center_x - left;
    let arrow_min = arrow + 4.0;
    let arrow_max = tip_w - arrow_min;
    if arrow_x < arrow_min {
        arrow_x = arrow_min;
    }
    if arrow_x > arrow_max {
        arrow_x = arrow_max;
    }

    tip.set_attribute("data-pos", pos)?;
    let style = format!("left: {left:.1}px; top: {top:.1}px; --tip-arrow-x: {arrow_x:.1}px;");
    tip.set_attribute("style", &style)?;
    Ok(())
}
