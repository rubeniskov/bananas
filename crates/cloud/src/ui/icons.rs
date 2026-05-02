//! Lucide icons rendered as inline SVG. We embed each icon's path data
//! verbatim from lucide.dev — keeps it dependency-free and tree-shaken
//! to exactly what the UI uses.
//!
//! Usage: `Icon { name: "pencil" }` — defaults to a 16px stroke icon
//! that inherits the surrounding text color via `currentColor`.

#![allow(non_snake_case)]

use dioxus::prelude::*;

#[derive(Props, Clone, PartialEq)]
pub struct IconProps {
    pub name: &'static str,
    #[props(default = 16)]
    pub size: u32,
    /// Extra CSS class to apply to the <svg>.
    #[props(default = "")]
    pub class: &'static str,
}

#[component]
pub fn Icon(props: IconProps) -> Element {
    let class = if props.class.is_empty() {
        format!("lucide lucide-{}", props.name)
    } else {
        format!("lucide lucide-{} {}", props.name, props.class)
    };
    rsx! {
        svg {
            xmlns: "http://www.w3.org/2000/svg",
            width: "{props.size}",
            height: "{props.size}",
            "viewBox": "0 0 24 24",
            fill: "none",
            stroke: "currentColor",
            "stroke-width": "2",
            "stroke-linecap": "round",
            "stroke-linejoin": "round",
            class: "{class}",
            "aria-hidden": "true",
            {paths_for(props.name)}
        }
    }
}

fn paths_for(name: &str) -> Element {
    // Each branch is the body of the corresponding icon on lucide.dev,
    // stripped of the wrapping <svg> attributes that Icon already emits.
    match name {
        "pencil" => rsx! {
            path { d: "M21.174 6.812a1 1 0 0 0-3.986-3.987L3.842 16.174a2 2 0 0 0-.5.83l-1.321 4.352a.5.5 0 0 0 .623.622l4.353-1.32a2 2 0 0 0 .83-.497z" }
            path { d: "m15 5 4 4" }
        },
        "trash-2" => rsx! {
            path { d: "M3 6h18" }
            path { d: "M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6" }
            path { d: "M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2" }
            line { x1: "10", x2: "10", y1: "11", y2: "17" }
            line { x1: "14", x2: "14", y1: "11", y2: "17" }
        },
        "plus" => rsx! {
            path { d: "M5 12h14" }
            path { d: "M12 5v14" }
        },
        "rotate-cw" => rsx! {
            path { d: "M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8" }
            path { d: "M3 3v5h5" }
        },
        "log-out" => rsx! {
            path { d: "M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4" }
            polyline { points: "16 17 21 12 16 7" }
            line { x1: "21", x2: "9", y1: "12", y2: "12" }
        },
        "download" => rsx! {
            path { d: "M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" }
            polyline { points: "7 10 12 15 17 10" }
            line { x1: "12", x2: "12", y1: "15", y2: "3" }
        },
        "upload" => rsx! {
            path { d: "M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" }
            polyline { points: "17 8 12 3 7 8" }
            line { x1: "12", x2: "12", y1: "3", y2: "15" }
        },
        "folder-open" => rsx! {
            path { d: "m6 14 1.45-2.9A2 2 0 0 1 9.24 10H20a2 2 0 0 1 1.94 2.5l-1.55 6a2 2 0 0 1-1.94 1.5H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h3.93a2 2 0 0 1 1.66.9l.82 1.2a2 2 0 0 0 1.66.9H18a2 2 0 0 1 2 2v2" }
        },
        "x" => rsx! {
            path { d: "M18 6 6 18" }
            path { d: "m6 6 12 12" }
        },
        "power" => rsx! {
            path { d: "M12 2v10" }
            path { d: "M18.4 6.6a9 9 0 1 1-12.77.04" }
        },
        "chart-bar" => rsx! {
            path { d: "M3 3v16a2 2 0 0 0 2 2h16" }
            path { d: "M7 16h8" }
            path { d: "M7 11h12" }
            path { d: "M7 6h3" }
        },
        "share-2" => rsx! {
            circle { cx: "18", cy: "5", r: "3" }
            circle { cx: "6", cy: "12", r: "3" }
            circle { cx: "18", cy: "19", r: "3" }
            line { x1: "8.59", x2: "15.42", y1: "13.51", y2: "17.49" }
            line { x1: "15.41", x2: "8.59", y1: "6.51", y2: "10.49" }
        },
        "hard-drive" => rsx! {
            line { x1: "22", x2: "2", y1: "12", y2: "12" }
            path { d: "M5.45 5.11 2 12v6a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-6l-3.45-6.89A2 2 0 0 0 16.76 4H7.24a2 2 0 0 0-1.79 1.11z" }
            line { x1: "6", x2: "6.01", y1: "16", y2: "16" }
            line { x1: "10", x2: "10.01", y1: "16", y2: "16" }
        },
        "users" => rsx! {
            path { d: "M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2" }
            circle { cx: "9", cy: "7", r: "4" }
            path { d: "M22 21v-2a4 4 0 0 0-3-3.87" }
            path { d: "M16 3.13a4 4 0 0 1 0 7.75" }
        },
        "key-round" => rsx! {
            path { d: "M2.586 17.414A2 2 0 0 0 2 18.828V21a1 1 0 0 0 1 1h3a1 1 0 0 0 1-1v-1a1 1 0 0 1 1-1h1a1 1 0 0 0 1-1v-1a1 1 0 0 1 1-1h.172a2 2 0 0 0 1.414-.586l.814-.814a6.5 6.5 0 1 0-4-4z" }
            circle { cx: "16.5", cy: "7.5", r: ".5", fill: "currentColor" }
        },
        "shield-check" => rsx! {
            path { d: "M20 13c0 5-3.5 7.5-7.66 8.95a1 1 0 0 1-.67-.01C7.5 20.5 4 18 4 13V6a1 1 0 0 1 1-1c2 0 4.5-1.2 6.24-2.72a1.17 1.17 0 0 1 1.52 0C14.51 3.81 17 5 19 5a1 1 0 0 1 1 1z" }
            path { d: "m9 12 2 2 4-4" }
        },
        "lock" => rsx! {
            rect { x: "3", y: "11", width: "18", height: "11", rx: "2", ry: "2" }
            path { d: "M7 11V7a5 5 0 0 1 10 0v4" }
        },
        "user-plus" => rsx! {
            path { d: "M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2" }
            circle { cx: "9", cy: "7", r: "4" }
            line { x1: "19", x2: "19", y1: "8", y2: "14" }
            line { x1: "22", x2: "16", y1: "11", y2: "11" }
        },
        "check" => rsx! {
            polyline { points: "20 6 9 17 4 12" }
        },
        "copy" => rsx! {
            rect { width: "14", height: "14", x: "8", y: "8", rx: "2", ry: "2" }
            path { d: "M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2" }
        },
        "cloud" => rsx! {
            path { d: "M17.5 19a4.5 4.5 0 1 0-1.41-8.775 5.5 5.5 0 0 0-10.836 1.913A4 4 0 0 0 6.5 19h11Z" }
        },
        "settings" => rsx! {
            path { d: "M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z" }
            circle { cx: "12", cy: "12", r: "3" }
        },
        "triangle-alert" => rsx! {
            path { d: "m21.73 18-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.73-3Z" }
            path { d: "M12 9v4" }
            path { d: "M12 17h.01" }
        },
        "circle-check" => rsx! {
            circle { cx: "12", cy: "12", r: "10" }
            path { d: "m9 12 2 2 4-4" }
        },
        "circle-x" => rsx! {
            circle { cx: "12", cy: "12", r: "10" }
            path { d: "m15 9-6 6" }
            path { d: "m9 9 6 6" }
        },
        "chevron-down" => rsx! {
            path { d: "m6 9 6 6 6-6" }
        },
        "sun" => rsx! {
            circle { cx: "12", cy: "12", r: "4" }
            path { d: "M12 2v2" }
            path { d: "M12 20v2" }
            path { d: "m4.93 4.93 1.41 1.41" }
            path { d: "m17.66 17.66 1.41 1.41" }
            path { d: "M2 12h2" }
            path { d: "M20 12h2" }
            path { d: "m6.34 17.66-1.41 1.41" }
            path { d: "m19.07 4.93-1.41 1.41" }
        },
        "moon" => rsx! {
            path { d: "M12 3a6 6 0 0 0 9 9 9 9 0 1 1-9-9Z" }
        },
        "monitor" => rsx! {
            rect { width: "20", height: "14", x: "2", y: "3", rx: "2" }
            line { x1: "8", x2: "16", y1: "21", y2: "21" }
            line { x1: "12", x2: "12", y1: "17", y2: "21" }
        },
        "circle-user-round" => rsx! {
            path { d: "M18 20a6 6 0 0 0-12 0" }
            circle { cx: "12", cy: "10", r: "4" }
            circle { cx: "12", cy: "12", r: "10" }
        },
        "package" => rsx! {
            path { d: "M11 21.73a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16V8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73z" }
            path { d: "M12 22V12" }
            path { d: "m3.3 7 8.7 5 8.7-5" }
            path { d: "m7.5 4.27 9 5.15" }
        },
        // Fallback: render an empty group so unknown names don't break layout.
        _ => rsx! { g {} },
    }
}
