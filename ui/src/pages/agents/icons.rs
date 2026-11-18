//! Eigene, schlichte Icons der Agenten als SVG (keine Markenlogos, sondern erkennbare Formen
//! und Farben): Claude orangefarbener Stern, Codex Spirale, Gemini blauer Funke, OpenCode
//! Klammern, Kilo „K“, Grok ein X.

use dioxus::prelude::*;
use nrzx_shared::AgentKind;

/// Icon eines Agenten (Claude, Codex, Gemini, OpenCode, Kilo, Grok).
#[component]
pub fn AgentIcon(kind: AgentKind, #[props(default = 16)] size: u32) -> Element {
    let title = kind.name();
    match kind {
        AgentKind::Claude => {
            // Stern aus zwölf Strahlen unterschiedlicher Länge
            let rays: Vec<(f64, f64, f64, f64)> = (0..12)
                .map(|i| {
                    let a = (i as f64) * std::f64::consts::PI / 6.0 + 0.13;
                    let r = if i % 2 == 0 { 10.0 } else { 7.6 };
                    (12.0 + 2.2 * a.cos(), 12.0 + 2.2 * a.sin(), 12.0 + r * a.cos(), 12.0 + r * a.sin())
                })
                .collect();
            rsx! {
                svg { width: "{size}", height: "{size}", view_box: "0 0 24 24", class: "shrink-0", role: "img", "aria-label": title,
                    for (x1 , y1 , x2 , y2) in rays {
                        line { x1: "{x1:.2}", y1: "{y1:.2}", x2: "{x2:.2}", y2: "{y2:.2}", stroke: "#d97757", stroke_width: "2.3", stroke_linecap: "round" }
                    }
                }
            }
        }
        AgentKind::Codex => rsx! {
            svg { width: "{size}", height: "{size}", view_box: "0 0 24 24", class: "shrink-0 text-foreground/80", role: "img", "aria-label": title,
                fill: "none", stroke: "currentColor", stroke_width: "1.9", stroke_linecap: "round",
                path { d: "M12 12.2c0-.9.9-1.5 1.7-1.1.9.4 1.1 1.6.4 2.3-.9 1-2.6.9-3.4-.2-1-1.3-.6-3.2.8-4 1.8-1 4.1-.3 5 1.5 1.1 2.2.1 4.9-2.1 5.8-2.6 1.1-5.6-.2-6.6-2.8-1.2-3 .4-6.4 3.4-7.5 3.4-1.2 7.1.6 8.2 4" }
            }
        },
        AgentKind::Gemini => rsx! {
            svg { width: "{size}", height: "{size}", view_box: "0 0 24 24", class: "shrink-0", role: "img", "aria-label": title,
                defs {
                    linearGradient { id: "nrz-gemini-spark", x1: "0", y1: "0", x2: "1", y2: "1",
                        stop { offset: "0", stop_color: "#4f8ef7" }
                        stop { offset: "1", stop_color: "#9b72e8" }
                    }
                }
                path { d: "M12 2c.6 5.1 4.9 9.4 10 10-5.1.6-9.4 4.9-10 10-.6-5.1-4.9-9.4-10-10 5.1-.6 9.4-4.9 10-10z", fill: "url(#nrz-gemini-spark)" }
            }
        },
        AgentKind::Opencode => rsx! {
            svg { width: "{size}", height: "{size}", view_box: "0 0 24 24", class: "shrink-0 text-foreground/75", role: "img", "aria-label": title,
                rect { x: "2.5", y: "2.5", width: "19", height: "19", rx: "5", fill: "currentColor", opacity: "0.16" }
                path { d: "M9.5 7H7v10h2.5M14.5 7H17v10h-2.5", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round" }
            }
        },
        AgentKind::Kilo => rsx! {
            svg { width: "{size}", height: "{size}", view_box: "0 0 24 24", class: "shrink-0", role: "img", "aria-label": title,
                rect { x: "2", y: "2", width: "20", height: "20", rx: "5", fill: "#facc15" }
                path { d: "M8.5 6.5v11M15.5 6.5 9 12l6.5 5.5", fill: "none", stroke: "#1c1c1c", stroke_width: "2.3", stroke_linecap: "round", stroke_linejoin: "round" }
            }
        },
        AgentKind::Grok => rsx! {
            svg { width: "{size}", height: "{size}", view_box: "0 0 24 24", class: "shrink-0 text-foreground/85", role: "img", "aria-label": title,
                fill: "none", stroke: "currentColor", stroke_width: "2.2", stroke_linecap: "round",
                path { d: "M5 4.5 19.5 19M19 4.5l-5.2 5.2M10.2 13.8 5 19" }
                circle { cx: "19", cy: "4.5", r: "0.6", fill: "currentColor" }
            }
        },
    }
}

/// Mehrere Agenten überlappend (z. B. „4 subagents“).
#[component]
pub fn AgentStack(kinds: Vec<AgentKind>, #[props(default = 18)] size: u32, #[props(default = 3)] max: usize) -> Element {
    let extra = kinds.len().saturating_sub(max);
    let shown: Vec<AgentKind> = kinds.into_iter().take(max).collect();
    rsx! {
        div { class: "flex items-center",
            for (i , k) in shown.into_iter().enumerate() {
                span {
                    key: "{i}",
                    class: "grid place-items-center rounded-full bg-background ring-2 ring-background",
                    style: if i > 0 { "margin-left:-6px" } else { "" },
                    AgentIcon { kind: k, size }
                }
            }
            if extra > 0 {
                span { class: "-ml-1 grid h-5 min-w-5 place-items-center rounded-full bg-muted px-1 text-[10px] font-medium text-muted-foreground ring-2 ring-background",
                    "+{extra}"
                }
            }
        }
    }
}
