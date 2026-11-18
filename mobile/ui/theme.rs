// Stile der Oberfläche von nrzXMobile (Tailwind v4: Thema, Variablen, eigene Klassen) als Rust.
// `cargo xtask ui` schreibt sie vor dem Bauen nach `tailwind.css` (nicht im Repository) – daraus
// erzeugt `dx` das CSS der Oberfläche. Hier ändern, nicht in der erzeugten Datei.

pub const TAILWIND_CSS: &str = r####"/* nrzXMobile – gleiche Grautöne wie nrzX (Variablen aus ui/tailwind.css), dazu die Flächen
   einer iOS-App: Seite, gefüllte Felder, Blasen, Blätter, Trennlinien und Link-Blau. */
@import "tailwindcss";

@source "./src/**/*.rs";

:root {
  --radius: 0.625rem;
  --font-sans: -apple-system, BlinkMacSystemFont, "SF Pro Text", "Inter", ui-sans-serif, system-ui, sans-serif;
  --font-mono: ui-monospace, "SF Mono", "JetBrains Mono", Menlo, Consolas, monospace;
  --background: oklch(0.18 0 0);
  --foreground: oklch(0.985 0 0);
  --card: oklch(0.205 0 0);
  --card-foreground: oklch(0.985 0 0);
  --popover: oklch(0.235 0 0);
  --popover-foreground: oklch(0.985 0 0);
  --primary: oklch(0.922 0 0);
  --primary-foreground: oklch(0.205 0 0);
  --secondary: oklch(0.269 0 0);
  --secondary-foreground: oklch(0.985 0 0);
  --muted: oklch(0.269 0 0);
  --muted-foreground: oklch(0.708 0 0);
  --accent: oklch(0.3 0 0);
  --accent-foreground: oklch(0.985 0 0);
  --destructive: oklch(0.704 0.191 22.216);
  --border: oklch(1 0 0 / 10%);
  --input: oklch(1 0 0 / 15%);
  --ring: oklch(0.556 0 0);
  --sidenav: oklch(0.2 0 0);
  --sidenav-foreground: oklch(0.985 0 0);
  --sidenav-accent: oklch(0.269 0 0);
  --sidenav-accent-foreground: oklch(0.985 0 0);
  --sidenav-border: oklch(1 0 0 / 8%);
  --success: oklch(0.65 0.16 145);
  --success-foreground: oklch(0.205 0 0);
  --warning: oklch(0.72 0.15 70);
  --warning-foreground: oklch(0.205 0 0);
  --info: oklch(0.68 0.13 250);
  --merged: oklch(0.65 0.17 300);
  --surface: oklch(0.22 0 0);
  /* Diff: hinzugefügt / entfernt */
  --diff-add: oklch(0.65 0.16 145 / 14%);
  --diff-add-strong: oklch(0.65 0.16 145 / 28%);
  --diff-del: oklch(0.704 0.191 22.216 / 14%);
  --diff-del-strong: oklch(0.704 0.191 22.216 / 28%);
  /* Syntax-Farben (Code-Blöcke im Chat) */
  --tok-kw: oklch(0.72 0.14 305);
  --tok-str: oklch(0.78 0.13 140);
  --tok-num: oklch(0.78 0.13 60);
  --tok-com: oklch(0.6 0.02 260);
  --tok-fn: oklch(0.82 0.12 85);
  --tok-ty: oklch(0.78 0.1 200);
  /* iOS-Flächen – nrzXMobile ist immer dunkel */
  --page: oklch(0.16 0 0);
  --fill: oklch(0.27 0 0 / 70%);
  --bubble: oklch(0.29 0 0);
  --elevated: oklch(0.21 0 0);
  --separator: oklch(1 0 0 / 9%);
  --link: oklch(0.68 0.16 252);
  --glass: oklch(0.18 0 0 / 72%);
}

@theme inline {
  --font-sans: var(--font-sans);
  --font-mono: var(--font-mono);
  --color-background: var(--background);
  --color-foreground: var(--foreground);
  --color-card: var(--card);
  --color-card-foreground: var(--card-foreground);
  --color-popover: var(--popover);
  --color-primary: var(--primary);
  --color-primary-foreground: var(--primary-foreground);
  --color-muted: var(--muted);
  --color-muted-foreground: var(--muted-foreground);
  --color-accent: var(--accent);
  --color-destructive: var(--destructive);
  --color-border: var(--border);
  --color-input: var(--input);
  --color-success: var(--success);
  --color-warning: var(--warning);
  --color-info: var(--info);
  --color-surface: var(--surface);
  --color-page: var(--page);
  --color-fill: var(--fill);
  --color-bubble: var(--bubble);
  --color-elevated: var(--elevated);
  --color-separator: var(--separator);
  --color-link: var(--link);
  --radius-lg: var(--radius);
}

/* Ladekreise auf eigener Ebene (Compositor): drehen ruhig weiter, auch wenn der Hauptthread arbeitet */
.animate-spin, [class*="animate-[spin"] { will-change: transform; }

@layer base {
  * { border-color: var(--border); }
  html, body { background: var(--background); color: var(--foreground); font-family: var(--font-sans); -webkit-tap-highlight-color: transparent; }
  body { overscroll-behavior-y: none; -webkit-text-size-adjust: 100%; }
  button, a, input, textarea, select { touch-action: manipulation; }
  button { cursor: pointer; }
  button:disabled { cursor: default; }
}

/* Milchglas für Kopf- und Eingabeleisten, Hinweise */
.glass, .glass-bar { background: var(--glass); -webkit-backdrop-filter: saturate(180%) blur(20px); backdrop-filter: saturate(180%) blur(20px); }
.glass-bar { border-color: var(--separator); }
header.glass-bar { border-bottom-width: 1px; }
div.glass-bar { border-top-width: 1px; }

/* Liste der Chats als Karte mit runden Ecken oben (wie T3 Code / iOS) */
.sheet { background: var(--card); border-top-left-radius: 28px; border-top-right-radius: 28px; box-shadow: 0 -1px 0 var(--separator); }
.row { position: relative; transition: background-color 0.12s; }
.row:active { background: var(--fill); }
.row + .row::before { content: ""; position: absolute; top: 0; left: 1.25rem; right: 1.25rem; height: 1px; background: var(--separator); }

/* Schwebender Knopf „Neuer Chat“ */
.fab { position: fixed; right: 1.25rem; bottom: max(env(safe-area-inset-bottom), 1.25rem); z-index: 30; display: flex; align-items: center; gap: 0.6rem; height: 3.5rem; padding: 0 1.4rem; border-radius: 1.25rem; background: var(--link); color: white; font-size: 16px; font-weight: 600; box-shadow: 0 10px 30px -8px color-mix(in oklab, var(--link) 70%, transparent), 0 2px 6px rgb(0 0 0 / 0.15); transition: transform 0.12s, opacity 0.12s; }
.fab:active { transform: scale(0.96); }
.fab:disabled { opacity: 0.45; }

/* Gruppierte Liste (Einstellungen, Formulare) */
.group-list { display: flex; flex-direction: column; overflow: hidden; border-radius: 14px; background: var(--card); }
.group-list > * + * { border-top: 1px solid var(--separator); }
.list-row { display: flex; min-height: 3rem; width: 100%; align-items: center; gap: 0.75rem; padding: 0.55rem 1rem; text-align: left; }
button.list-row:active { background: var(--fill); }
.field { display: block; width: 100%; height: 3rem; padding: 0 1rem; background: transparent; font-size: 16px; outline: none; }
.field::placeholder { color: var(--muted-foreground); }
.section-label { margin: 1.5rem 1rem 0.4rem; font-size: 13px; text-transform: uppercase; letter-spacing: 0.02em; color: var(--muted-foreground); }
.btn-primary { display: flex; height: 3.25rem; align-items: center; justify-content: center; gap: 0.5rem; border-radius: 14px; background: var(--link); color: white; font-size: 17px; font-weight: 600; transition: opacity 0.12s, transform 0.12s; }
.btn-primary:active { transform: scale(0.98); }
.btn-primary:disabled { opacity: 0.4; transform: none; }

/* Schalter wie in iOS */
.switch { appearance: none; -webkit-appearance: none; position: relative; width: 51px; height: 31px; flex-shrink: 0; border-radius: 999px; background: var(--fill); transition: background 0.2s; }
.switch::after { content: ""; position: absolute; top: 2px; left: 2px; width: 27px; height: 27px; border-radius: 999px; background: white; box-shadow: 0 2px 4px rgb(0 0 0 / 0.25); transition: transform 0.2s; }
.switch:checked { background: var(--success); }
.switch:checked::after { transform: translateX(20px); }

/* Bewegung */
@keyframes nrzm-sheet { from { transform: translateY(100%); } to { transform: none; } }
@keyframes nrzm-fade { from { opacity: 0; } to { opacity: 1; } }
@keyframes nrzm-drop { from { opacity: 0; transform: translateY(-12px); } to { opacity: 1; transform: none; } }
.animate-sheet { animation: nrzm-sheet 0.32s cubic-bezier(0.32, 0.72, 0, 1); }
.animate-fade { animation: nrzm-fade 0.2s ease-out; }
.animate-drop { animation: nrzm-drop 0.25s cubic-bezier(0.32, 0.72, 0, 1); }
@media (prefers-reduced-motion: reduce) { .animate-sheet, .animate-fade, .animate-drop { animation: none; } }

/* Antworten der Agenten (HTML vom Server) */
.md { font-size: 16px; line-height: 1.55; overflow-wrap: anywhere; }
.md p { margin: 0.4em 0; }
.md ul, .md ol { margin: 0.4em 0; padding-left: 1.3em; }
.md ul { list-style: disc; }
.md ol { list-style: decimal; }
.md li { margin: 0.15em 0; }
.md h1, .md h2, .md h3, .md h4 { font-weight: 600; margin: 0.8em 0 0.3em; line-height: 1.3; }
.md h1 { font-size: 1.25em; } .md h2 { font-size: 1.15em; } .md h3 { font-size: 1.05em; }
.md a { text-decoration: underline; text-underline-offset: 2px; }
.md code { font-family: var(--font-mono); font-size: 0.86em; background: var(--muted); padding: 0.1em 0.35em; border-radius: 5px; }
.md pre { background: var(--surface); border: 1px solid var(--border); border-radius: 10px; padding: 0.7em 0.85em; overflow-x: auto; margin: 0.5em 0; }
.md pre code { background: none; padding: 0; font-size: 12.5px; }
.md table { display: block; overflow-x: auto; border-collapse: collapse; font-size: 13px; margin: 0.5em 0; }
.md th, .md td { border: 1px solid var(--border); padding: 0.3em 0.6em; }
.md blockquote { border-left: 3px solid var(--border); padding-left: 0.8em; color: var(--muted-foreground); }
"####;
