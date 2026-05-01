//! Wasm-side runtime helpers shared by every plugin SPA.
//!
//! The host webadmin SPA and each plugin SPA both run as separate
//! dioxus-web roots inside the same browser document. Dioxus
//! attaches its delegated event listener to each root's mount
//! element, walks `data-dioxus-id` up the DOM, and dispatches the
//! event to the matching vdom node.
//!
//! The host's mount (`#main`) is an *ancestor* of every plugin's
//! mount (e.g. `#cloud-mfe-root`), so events bubble from plugin
//! children through the host's listener too. The host then walks
//! up, lands on a plugin element with `data-dioxus-id="N"`, and
//! looks up node `N` in its own vdom — which is a different node
//! entirely. Visible symptom: clicking a plugin form input pops
//! the host's user-menu dropdown (because some host node happens
//! to share the plugin's id and has an onclick handler).
//!
//! `isolate_root` prevents this by attaching `stopPropagation`
//! listeners to the plugin's mount element for the event types
//! dioxus delegates. Plugin-side listeners (which fire FIRST as
//! the event bubbles past the inner mount) still see the event;
//! the stop-propagation just keeps it from leaking out.

#![cfg(target_arch = "wasm32")]

use wasm_bindgen::prelude::*;

/// Attach `stopPropagation` listeners on `mount` for every event
/// type dioxus-web delegates. Call once after wiping the mount's
/// children; the closure is forgotten so the listeners persist
/// for the document's lifetime (which matches the SPA's lifetime).
pub fn isolate_root(mount: &web_sys::Element) {
    // Same delegated-event set dioxus 0.7 sets up via
    // `createListener(...)` for bubbling event types. Keeping this
    // narrow to events that actually bubble (no resize/visible —
    // those are non-bubbling observer-driven).
    const EVENTS: &[&str] = &[
        "click",
        "input",
        "change",
        "submit",
        "keydown",
        "keyup",
        "keypress",
        "mousedown",
        "mouseup",
        "focusin",
        "focusout",
        "wheel",
        "contextmenu",
        "dblclick",
    ];

    let stopper = Closure::<dyn FnMut(web_sys::Event)>::new(|e: web_sys::Event| {
        e.stop_propagation();
    });
    let f = stopper.as_ref().unchecked_ref::<js_sys::Function>();
    for evt in EVENTS {
        let _ = mount.add_event_listener_with_callback(evt, f);
    }
    stopper.forget();
}
