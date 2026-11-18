// Stile der Oberfläche von nrzX (Tailwind v4: Thema, Variablen, eigene Klassen) als Rust.
// `cargo xtask ui` schreibt sie vor dem Bauen nach `tailwind.css` (nicht im Repository) – daraus
// erzeugt `dx` das CSS der Oberfläche. Hier ändern, nicht in der erzeugten Datei.

pub const TAILWIND_CSS: &str = r####"/* nrzGit – Oberfläche auf Basis von rust-ui (https://github.com/rust-ui/ui, MIT).
   Wie nrzPaper: Grautöne, dunkles Thema von rust-ui, Akzente grau statt blau.
   Ein festes, dunkles Thema. */
@import "tailwindcss";

@source "./src/**/*.rs";

@custom-variant dark (&:is(.dark *));

:root {
  --radius: 0.625rem;
  --font-sans: "Inter", ui-sans-serif, system-ui, sans-serif;
  --font-mono: ui-monospace, "SF Mono", "JetBrains Mono", Menlo, Consolas, monospace;
  --background: oklch(0.22 0.005 270);
  --foreground: oklch(0.985 0 0);
  --card: oklch(0.31 0.005 270);
  --card-foreground: oklch(0.985 0 0);
  --popover: oklch(0.3 0.006 270);
  --popover-foreground: oklch(0.985 0 0);
  --primary: oklch(0.922 0 0);
  --primary-foreground: oklch(0.205 0 0);
  --secondary: oklch(0.269 0 0);
  --secondary-foreground: oklch(0.985 0 0);
  --muted: oklch(0.269 0 0);
  --muted-foreground: oklch(0.76 0.004 270);
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
}


/* Terminal: die 16 Grundfarben (`vt::css`) und der Cursor. Fest dunkel; das eingefärbte Terminal
   (natives CLI, `.nrz-term-themed`) nimmt die Schriftfarbe als Cursor. */
:root {
  --term-0: #3a3a3a;
  --term-1: #f07178;
  --term-2: #a6d189;
  --term-3: #e5c890;
  --term-4: #8caaee;
  --term-5: #ca9ee6;
  --term-6: #81c8be;
  --term-7: #c6c6c6;
  --term-8: #6c6c6c;
  --term-9: #ff8b92;
  --term-10: #b9e0a0;
  --term-11: #f1d8a8;
  --term-12: #a4bdf5;
  --term-13: #dab4f0;
  --term-14: #99d6cd;
  --term-15: #f5f5f5;
  --term-cursor: #e5e5e5;
  --term-cursor-text: #141414;
}

.nrz-term-themed {
  --term-cursor: var(--foreground);
}


/* Milchglas (nur in der App, wenn das System es kann: macOS Vibrancy, Windows Acryl bzw. Weichzeichnen): Das
   Fenster ist durchsichtig, das System weichzeichnet den Schreibtisch dahinter. Die Oberfläche tönt
   nur noch – Seitenleisten leicht, der Inhalt stärker; Füllungen werden durchscheinend. Schwebendes
   (Dialoge, Canvas-Fenster: `.nrz-pane`) zeichnet zusätzlich selbst weich, damit es über Inhalt
   lesbar bleibt. Menüs und Popups (Rechtsklick, Auswahl, Hinweise: `bg-popover`) sind wie native
   Menüs durchscheinend und stark weichgezeichnet – nur in einer schon weichgezeichneten Fläche
   (Dialog, Blase der Canvas) fast deckend, weil dort kein zweiter Weichzeichner wirkt.
   Abschaltbar unter Einstellungen → Erscheinungsbild. */
html.nrz-glass {
  --background: oklch(0.17 0.002 270 / 30%);
  --card: oklch(1 0 0 / 5%);
  --popover: oklch(0.19 0.002 270 / 88%);
  --secondary: oklch(1 0 0 / 8%);
  --muted: oklch(1 0 0 / 7%);
  --accent: oklch(1 0 0 / 9%);
  --surface: oklch(0.13 0 0 / 38%);
  --sidenav: oklch(0.17 0 0 / 22%);
  --sidenav-accent: oklch(1 0 0 / 8%);
  --border: oklch(1 0 0 / 9%);
  --pane: oklch(0.19 0.002 270 / 88%);
}



/* Schwebende Flächen über Inhalt (Dialoge, Menüs, Canvas-Fenster): im Milchglas eigener
   Weichzeichner und fast deckend. Ausserhalb von @layer, damit es die Farbe aus den Utilities
   (`bg-background`) schlägt. */
html.nrz-glass .nrz-pane {
  background-color: var(--pane);
  -webkit-backdrop-filter: blur(28px) saturate(160%);
  backdrop-filter: blur(28px) saturate(160%);
}
html.nrz-glass .bg-popover {
  -webkit-backdrop-filter: blur(30px) saturate(180%);
  backdrop-filter: blur(30px) saturate(180%);
}
html.nrz-glass .nrz-pane .bg-popover {
  background-color: oklch(0.21 0.002 270 / 96%);
}

@theme inline {
  --font-sans: var(--font-sans);
  --font-mono: var(--font-mono);
  --radius-sm: calc(var(--radius) - 4px);
  --radius-md: calc(var(--radius) - 2px);
  --radius-lg: var(--radius);
  --radius-xl: calc(var(--radius) + 4px);
  --color-background: var(--background);
  --color-foreground: var(--foreground);
  --color-card: var(--card);
  --color-card-foreground: var(--card-foreground);
  --color-popover: var(--popover);
  --color-popover-foreground: var(--popover-foreground);
  --color-primary: var(--primary);
  --color-primary-foreground: var(--primary-foreground);
  --color-secondary: var(--secondary);
  --color-secondary-foreground: var(--secondary-foreground);
  --color-muted: var(--muted);
  --color-muted-foreground: var(--muted-foreground);
  --color-accent: var(--accent);
  --color-accent-foreground: var(--accent-foreground);
  --color-destructive: var(--destructive);
  --color-border: var(--border);
  --color-input: var(--input);
  --color-ring: var(--ring);
  --color-sidenav: var(--sidenav);
  --color-sidenav-foreground: var(--sidenav-foreground);
  --color-sidenav-accent: var(--sidenav-accent);
  --color-sidenav-accent-foreground: var(--sidenav-accent-foreground);
  --color-sidenav-border: var(--sidenav-border);
  --color-success: var(--success);
  --color-success-foreground: var(--success-foreground);
  --color-warning: var(--warning);
  --color-warning-foreground: var(--warning-foreground);
  --color-info: var(--info);
  --color-merged: var(--merged);
  --color-surface: var(--surface);
  --color-diff-add: var(--diff-add);
  --color-diff-add-strong: var(--diff-add-strong);
  --color-diff-del: var(--diff-del);
  --color-diff-del-strong: var(--diff-del-strong);
  --animate-in: enter 150ms ease-out;
  --animate-toast-in: toast-in 260ms cubic-bezier(0.16, 1, 0.3, 1);
  --animate-toast-out: toast-out 220ms ease-in forwards;
}

@keyframes toast-in {
  from { opacity: 0; transform: translateX(24px) scale(0.97); }
  to { opacity: 1; transform: none; }
}

@keyframes toast-out {
  from { opacity: 1; transform: none; }
  to { opacity: 0; transform: translateX(24px) scale(0.97); }
}

@media (prefers-reduced-motion: reduce) {
  .animate-toast-in, .animate-toast-out { animation-duration: 1ms; }
}

/* „nrzX ist aktuell“ unten in der Seitenleiste: kurz stehen lassen, dann ausblenden */
@keyframes nrz-fade-out {
  0%, 70% { opacity: 1; }
  100% { opacity: 0; }
}
.nrz-fade-out { animation: nrz-fade-out 2.5s ease-in forwards; }

@keyframes enter {
  from { opacity: 0; transform: translateY(4px) scale(0.98); }
  to { opacity: 1; transform: none; }
}

@font-face {
  font-family: "Inter";
  font-style: normal;
  font-display: swap;
  font-weight: 100 900;
  src: url("/static/fonts/inter-latin.woff2") format("woff2-variations");
  unicode-range: U+0000-00FF, U+0131, U+0152-0153, U+02BB-02BC, U+02C6, U+02DA, U+02DC, U+2000-206F, U+2074, U+20AC, U+2122, U+2191, U+2193, U+2212, U+2215, U+FEFF, U+FFFD;
}

@font-face {
  font-family: "Inter";
  font-style: normal;
  font-display: swap;
  font-weight: 100 900;
  src: url("/static/fonts/inter-latin-ext.woff2") format("woff2-variations");
  unicode-range: U+0100-02AF, U+0304, U+0308, U+0329, U+1E00-1E9F, U+1EF2-1EFF, U+2020, U+20A0-20AB, U+20AD-20C0, U+2113, U+2C60-2C7F, U+A720-A7FF;
}

/* Ladekreise auf eigener Ebene (Compositor): drehen weiter, auch wenn der Hauptthread arbeitet */
[class*="animate-spin"], [class*="animate-[spin"] { will-change: transform; }

@layer base {
  * { @apply border-border outline-ring/50; }
  html { color-scheme: dark; }
  body { @apply bg-background text-foreground antialiased; font-family: var(--font-sans); }
  /* Milchglas: Seite durchsichtig, getönt wird in den Flächen */
  html.nrz-glass, html.nrz-glass body { background: transparent; }
  /* Fenster der Menüleiste (macOS): Rand durchsichtig, der Inhalt hat runde Ecken */
  html.nrz-tray, html.nrz-tray body { background: transparent; overflow: hidden; }
  /* Kein Überscrollen (Nachfedern) über den Anfang oder das Ende hinaus – bei der Seite und in den
     grossen Bereichen mit eigenem Bildlauf (Listen, Chat, Spalten; WebKit federt sonst nach).
     Nicht auf allen Elementen: jedes abgeschnittene Element (`overflow: hidden`, z. B. `truncate`
     bei Thread-Titeln) ist ein Scroll-Container – mit `none` gäbe es das Rad nicht an die Liste
     weiter, über einem Titel liesse sich die Seitenleiste dann nicht scrollen. */
  html, body, main, .flex-1.overflow-y-auto, .flex-1.overflow-auto { overscroll-behavior: none; }
  ::selection { background: oklch(0.45 0 0); color: oklch(0.985 0 0); }
  ::-webkit-scrollbar { width: 10px; height: 10px; }
  ::-webkit-scrollbar-thumb { background: oklch(0.35 0 0); border-radius: 9999px; border: 2px solid transparent; background-clip: padding-box; }
  ::-webkit-scrollbar-track { background: transparent; }
}

@layer components {
  /* ---------------------------------------------------------------- Markdown (README, Issues, Wiki) */
  .markdown { @apply text-sm leading-relaxed break-words; }
  .markdown > :first-child { margin-top: 0 !important; }
  .markdown > :last-child { margin-bottom: 0 !important; }
  .markdown h1, .markdown h2, .markdown h3, .markdown h4, .markdown h5, .markdown h6 { @apply relative mt-6 mb-3 font-semibold leading-tight scroll-mt-20; }
  .markdown h1 { @apply text-2xl border-b pb-2; }
  .markdown h2 { @apply text-xl border-b pb-1.5; }
  .markdown h3 { @apply text-lg; }
  .markdown h4 { @apply text-base; }
  .markdown h5, .markdown h6 { @apply text-sm; }
  .markdown h6 { @apply text-muted-foreground; }
  .markdown .anchor { @apply absolute -left-5 pr-1 text-muted-foreground no-underline opacity-0 transition-opacity; }
  .markdown h1:hover .anchor, .markdown h2:hover .anchor, .markdown h3:hover .anchor, .markdown h4:hover .anchor { opacity: 1; }
  .markdown p, .markdown ul, .markdown ol, .markdown blockquote, .markdown table, .markdown pre, .markdown details, .markdown dl { @apply my-3; }
  .markdown a { @apply text-info underline-offset-2 hover:underline; }
  .markdown a.mention, .markdown a.ref-issue { @apply font-medium text-foreground hover:underline; }
  .markdown ul { @apply list-disc pl-6; }
  .markdown ol { @apply list-decimal pl-6; }
  .markdown li { @apply my-1; }
  .markdown li > ul, .markdown li > ol { @apply my-1; }
  .markdown ul:has(> li > input[type=checkbox]) { @apply list-none pl-1; }
  .markdown li > input[type=checkbox] { @apply mr-2 align-middle accent-[var(--primary)]; }
  .markdown blockquote { @apply border-l-4 pl-4 text-muted-foreground; }
  .markdown code { @apply rounded-md bg-muted px-1.5 py-0.5 font-mono text-[0.85em]; }
  .markdown pre { @apply overflow-x-auto rounded-lg border bg-surface p-4 text-[13px] leading-normal; }
  .markdown pre code { @apply bg-transparent p-0 text-[13px]; }
  .markdown table { @apply block w-max max-w-full overflow-auto border-collapse; }
  .markdown th, .markdown td { @apply border px-3 py-1.5; }
  .markdown th { @apply bg-muted/50 font-semibold; }
  .markdown tr:nth-child(2n) td { @apply bg-muted/20; }
  .markdown img { @apply inline-block max-w-full rounded; }
  .markdown hr { @apply my-6 border-t; }
  .markdown kbd { @apply rounded border border-b-2 bg-muted px-1.5 py-0.5 font-mono text-xs; }
  .markdown details { @apply rounded-lg border px-4 py-2; }
  .markdown summary { @apply cursor-pointer font-medium; }
  .markdown .footnote-definition { @apply text-xs text-muted-foreground; }
  .markdown .markdown-alert { @apply rounded-lg border-l-4 px-4 py-2; }
  .markdown .markdown-alert-note { @apply border-info bg-info/10; }
  .markdown .markdown-alert-tip { @apply border-success bg-success/10; }
  .markdown .markdown-alert-important { @apply border-merged bg-merged/10; }
  .markdown .markdown-alert-warning { @apply border-warning bg-warning/10; }
  .markdown .markdown-alert-caution { @apply border-destructive bg-destructive/10; }

  /* ---------------------------------------------------------------- Code-Ansicht und Diffs */
  .code-table { @apply w-full border-collapse font-mono text-[12.5px] leading-5; }
  .code-table td { @apply align-top; }
  .code-table .ln { @apply w-[1%] min-w-12 select-none whitespace-nowrap px-3 text-right text-muted-foreground/70; }
  .code-table .ln a { @apply text-inherit hover:text-foreground; }
  .code-table .code { @apply whitespace-pre pl-3 pr-6; }
  .code-table tr:target, .code-table tr.selected { @apply bg-warning/15; }
  .diff-add { background: var(--diff-add); }
  .diff-add .ln { background: var(--diff-add-strong); }
  .diff-del { background: var(--diff-del); }
  .diff-del .ln { background: var(--diff-del-strong); }
  .diff-hunk { @apply bg-info/10 text-muted-foreground; }

  /* Syntax-Hervorhebung */
  .tok-kw { color: var(--tok-kw); font-style: italic; }
  .tok-str { color: var(--tok-str); }
  .tok-num { color: var(--tok-num); }
  .tok-com { color: var(--tok-com); font-style: italic; }
  .tok-fn { color: var(--tok-fn); }
  .tok-ty { color: var(--tok-ty); }

  /* Code-Blöcke der Arbeitsschritte im Chat */
  .work-code .row { @apply flex min-w-max; }
  .work-code .no { @apply w-11 shrink-0 select-none pr-2 text-right tabular-nums text-muted-foreground/60; }
  .work-code .sign { @apply w-5 shrink-0 select-none text-center; }
  .work-code .add { background: var(--diff-add); }
  .work-code .add .sign { color: var(--success); }
  .work-code .del { background: var(--diff-del); }
  .work-code .del .sign { color: var(--destructive); }
}

/* ------------------------------------------------------------------ nrzX: Rahmen
   Paneele (Aufgaben · Chat · Browser) mit klarer Kante über dem Fenstergrund. In der App ist alles
   durchsichtig: der Grund nur leicht getönt über dem Milchglas des Systems, die Paneele heller und
   halb durchsichtig. Reiter hängen an ihrem Paneel (eine Form mit runden Übergängen). */
/* In der Ebene „components“: Tailwind-Klassen (z. B. rounded-tl-none) gehen vor */
@layer components {
:root {
  --nrx-ground: oklch(0.15 0.002 270);
  --nrx-surf: oklch(0.21 0.002 270);
  --nrx-edge: oklch(1 0 0 / 0.12);
  --nrx-fillet: 12px;
  /* feste Breiten: der Umriss des Paneels spart dort aus, wo der Reiter ansetzt */
  --nrx-tab: 168px;
  --nrx-task: 152px;
  /* Reihe oben (Reiter, Knopf der Seitenleiste) und links vor dem Knopf */
  --nrx-row: 48px;
  --nrx-lead: 4px;
  /* welcher Reiter des Threads gewählt ist und wo die Reiter beginnen (`tabs::row_vars`) */
  --nrx-ai: 0;
  --nrx-x0: 0px;
}
/* In der App: Lage der Ampel (die App meldet die gemessene: Mitte und rechter Rand). Die Reihe ist
   so hoch, dass die Reiter auf ihrer Mitte liegen; die schmale Leiste so breit, dass die Ampel
   mittig darüber steht */
.nrz-mac {
  --nrz-tl-y: 26px;
  --nrz-tl-r: 79px;
  --nrx-row: calc(var(--nrz-tl-y) + 18px);
  --nrx-lead: calc(var(--nrz-tl-r) + 3px);
}
.nrx-row { height: var(--nrx-row); }
/* Kopfleiste oben am Fenster (ohne Reiter darüber): Mitte auf der Höhe der Ampel */
.nrz-mac .nrx-topbar { height: calc(2 * var(--nrz-tl-y) - 16px); }
html.nrz-glass {
  /* Grund fast ohne Tönung, Paneele nur leicht aufgehellt: der Schreibtisch scheint deutlich durch,
     die Kanten halten die Formen klar */
  --nrx-ground: oklch(0.12 0.002 270 / 0.1);
  --nrx-surf: oklch(0.2 0.002 270 / 0.24);
  --nrx-edge: oklch(1 0 0 / 0.15);
}
.nrx-backdrop { background: var(--nrx-ground); }
.nrx-lead { padding-left: var(--nrx-lead); }

/* Paneel: Fläche halb durchsichtig, die Kante zeichnet ::after – dort kann sie aussparen, wo ein
   Reiter ansetzt (`--nrx-notch`, siehe unten), ohne dass sich zwei durchsichtige Flächen überlagern */
.nrx-panel {
  position: relative;
  border-radius: 18px;
  background: var(--nrx-surf);
  box-shadow: 0 24px 48px -24px rgb(0 0 0 / 0.45);
}
.nrx-panel::after {
  content: "";
  position: absolute;
  inset: 0;
  z-index: 30;
  border: 1px solid var(--nrx-edge);
  border-radius: inherit;
  pointer-events: none;
  clip-path: var(--nrx-notch, none);
}
@media (min-width: 48rem) {
  /* aktiver Reiter an der linken Kante (Rechts: runder Übergang, 8 px davon trägt der Reiter) */
  .nrx-notch-tl { --nrx-notch: polygon(0 0, 1px 0, 1px 1px, calc(var(--nrx-tab) + 8px) 1px, calc(var(--nrx-tab) + 8px) 0, 100% 0, 100% 100%, 0 100%); }
  /* „Neue Aufgabe“ rechts bündig an der Aufgabenleiste */
  .nrx-notch-tr { --nrx-notch: polygon(0 0, calc(100% - var(--nrx-task) - 8px) 0, calc(100% - var(--nrx-task) - 8px) 1px, calc(100% - 1px) 1px, calc(100% - 1px) 0, 100% 0, 100% 100%, 0 100%); }
  /* eingeklappt ohne Reiter: die Fläche reicht rechts neben den Knöpfen bis oben (ein Reiter von
     --nrx-lead + 150 px – nach vier Knöpfen – bis zum rechten Rand, mit rundem Übergang links) */
  .nrx-notch-big { --nrx-notch: polygon(0 0, calc(var(--nrx-lead) + 142px) 0, calc(var(--nrx-lead) + 142px) 1px, calc(100% - 1px) 1px, calc(100% - 1px) 0, 100% 0, 100% 100%, 0 100%); }
  /* gewählter Reiter weiter rechts: runde Übergänge auf beiden Seiten */
  .nrx-notch-at {
    --nrx-ax: calc(var(--nrx-x0) + var(--nrx-ai) * var(--nrx-tab));
    --nrx-notch: polygon(0 0, calc(var(--nrx-ax) - 8px) 0, calc(var(--nrx-ax) - 8px) 1px, calc(var(--nrx-ax) + var(--nrx-tab) + 8px) 1px, calc(var(--nrx-ax) + var(--nrx-tab) + 8px) 0, 100% 0, 100% 100%, 0 100%);
  }
}

/* Reiter am Paneel: ragt 1 px in dessen Oberkante. Die unterste Zeile bleibt leer (dort liegt schon
   die Fläche des Paneels), die Kante zeichnet `.nrx-line` – an Seiten mit rundem Übergang nur bis
   zu dessen Beginn */
.nrx-tab {
  position: relative;
  z-index: 2;
  display: inline-flex;
  align-items: center;
  flex-shrink: 0;
  height: 37px;
  margin-bottom: -1px;
  border-radius: 13px 13px 0 0;
  background: linear-gradient(var(--nrx-surf), var(--nrx-surf)) top / 100% calc(100% - 1px) no-repeat;
}
.nrx-tab-a { width: var(--nrx-tab); }
/* der grosse Reiter (eingeklappt): bis zum rechten Rand, oben rechts so rund wie das Paneel */
.nrx-tab-big { flex: 1 1 0%; min-width: 0; border-top-right-radius: 18px; }
.nrx-tab-t { width: var(--nrx-task); }
.nrx-line {
  position: absolute;
  inset: 0;
  border: 1px solid var(--nrx-edge);
  border-bottom: 0;
  border-radius: inherit;
  pointer-events: none;
}
.nrx-fr > .nrx-line { clip-path: polygon(0 0, 100% 0, 100% calc(100% - var(--nrx-fillet)), calc(100% - 1px) calc(100% - var(--nrx-fillet)), calc(100% - 1px) 100%, 0 100%); }
.nrx-fl > .nrx-line { clip-path: polygon(0 0, 100% 0, 100% 100%, 1px 100%, 1px calc(100% - var(--nrx-fillet)), 0 calc(100% - var(--nrx-fillet))); }
.nrx-fl.nrx-fr > .nrx-line { clip-path: polygon(0 0, 100% 0, 100% calc(100% - var(--nrx-fillet)), calc(100% - 1px) calc(100% - var(--nrx-fillet)), calc(100% - 1px) 100%, 1px 100%, 1px calc(100% - var(--nrx-fillet)), 0 calc(100% - var(--nrx-fillet))); }
/* Runder Übergang vom Reiter in die Oberkante: Viertelkreis aus der Fläche ausgespart, auf dem
   Bogen die Linie; die unterste Zeile wie beim Reiter leer */
.nrx-fl::before, .nrx-fr::after {
  content: "";
  position: absolute;
  bottom: 0;
  width: var(--nrx-fillet);
  height: var(--nrx-fillet);
  pointer-events: none;
  background-size: 100% calc(100% - 1px);
  background-position: top;
  background-repeat: no-repeat;
}
.nrx-fl::before {
  right: calc(100% - 1px);
  background-image: radial-gradient(circle at 0 0, transparent calc(var(--nrx-fillet) - 1.6px), var(--nrx-edge) calc(var(--nrx-fillet) - 1px), var(--nrx-edge) calc(var(--nrx-fillet) - 0.2px), var(--nrx-surf) calc(var(--nrx-fillet) + 0.4px));
}
.nrx-fr::after {
  left: calc(100% - 1px);
  background-image: radial-gradient(circle at 100% 0, transparent calc(var(--nrx-fillet) - 1.6px), var(--nrx-edge) calc(var(--nrx-fillet) - 1px), var(--nrx-edge) calc(var(--nrx-fillet) - 0.2px), var(--nrx-surf) calc(var(--nrx-fillet) + 0.4px));
}
/* Übrige Reiter des Threads: gleiche Breite wie der gewählte (die Aussparung rechnet damit),
   ohne Fläche – nur beim Darüberfahren ein leiser Hintergrund */
.nrx-tab-i {
  position: relative;
  display: flex;
  align-items: center;
  flex-shrink: 0;
  width: var(--nrx-tab);
  height: 37px;
  margin-bottom: -1px;
  padding: 0 6px 1px;
}
/* Popups (Menüs, Auswahl, Nutzung): helles Glas mit eigenem Weichzeichner, klare Kante, Tiefe */
/* Aufgaben in der Seitenleiste nach Ausgang: grün fertig, rot Fehler/Abbruch, gelb Rückfrage */
.nrx-tone-done { background: oklch(0.74 0.18 146 / 0.17); box-shadow: inset 0 0 0 1px oklch(0.74 0.18 146 / 0.24); }
.nrx-tone-done:hover { background: oklch(0.74 0.18 146 / 0.23); }
.nrx-tone-done.nrx-tone-on { background: oklch(0.74 0.18 146 / 0.29); box-shadow: inset 0 0 0 1px oklch(0.74 0.18 146 / 0.45); }
.nrx-tone-fail { background: oklch(0.65 0.22 25 / 0.18); box-shadow: inset 0 0 0 1px oklch(0.65 0.22 25 / 0.26); }
.nrx-tone-fail:hover { background: oklch(0.65 0.22 25 / 0.24); }
.nrx-tone-fail.nrx-tone-on { background: oklch(0.65 0.22 25 / 0.3); box-shadow: inset 0 0 0 1px oklch(0.65 0.22 25 / 0.48); }
/* Rückfrage: gelb und sanft blinkend, bis sie beantwortet ist */
.nrx-tone-ask { background: oklch(0.86 0.17 90 / 0.19); box-shadow: inset 0 0 0 1px oklch(0.86 0.17 90 / 0.3); animation: nrx-ask 1.6s ease-in-out infinite; }
.nrx-tone-ask.nrx-tone-on { animation-name: nrx-ask-on; }
@keyframes nrx-ask {
  0%, 100% { background-color: oklch(0.86 0.17 90 / 0.12); box-shadow: inset 0 0 0 1px oklch(0.86 0.17 90 / 0.22); }
  50% { background-color: oklch(0.86 0.17 90 / 0.3); box-shadow: inset 0 0 0 1px oklch(0.86 0.17 90 / 0.55); }
}
@keyframes nrx-ask-on {
  0%, 100% { background-color: oklch(0.86 0.17 90 / 0.22); box-shadow: inset 0 0 0 1px oklch(0.86 0.17 90 / 0.4); }
  50% { background-color: oklch(0.86 0.17 90 / 0.38); box-shadow: inset 0 0 0 1px oklch(0.86 0.17 90 / 0.65); }
}
.nrx-ic-done { color: oklch(0.8 0.18 146); }
.nrx-ic-ask { color: oklch(0.88 0.17 90); animation: nrx-ask-ic 1.6s ease-in-out infinite; }
@keyframes nrx-ask-ic { 50% { opacity: 0.5; } }
@media (prefers-reduced-motion: reduce) { .nrx-tone-ask, .nrx-ic-ask { animation: none; } }

.nrx-pop {
  border-radius: 14px;
  border: 1px solid oklch(1 0 0 / 0.13);
  background: oklch(0.19 0.002 270 / 0.88);
  -webkit-backdrop-filter: blur(32px) saturate(140%);
  backdrop-filter: blur(32px) saturate(140%);
  box-shadow: 0 0 0 0.5px rgb(0 0 0 / 0.55), 0 22px 48px -12px rgb(0 0 0 / 0.65), inset 0 1px 0 oklch(1 0 0 / 0.05);
  color: var(--popover-foreground);
}
/* Dialoge (Bestätigen, Editoren): wie die Popups, etwas runder */
.nrx-dialog { border-radius: 18px; }
/* Knöpfe in Dialogen */
.nrx-btn {
  display: inline-flex; align-items: center; justify-content: center; gap: 0.4rem;
  height: 2.25rem; padding: 0 1rem; border-radius: 10px; font-size: 13.5px; font-weight: 500;
  border: 1px solid oklch(1 0 0 / 0.12); background: oklch(1 0 0 / 0.05); transition: background-color .12s;
}
.nrx-btn:hover { background: oklch(1 0 0 / 0.1); }
.nrx-btn-danger { border-color: transparent; background: color-mix(in oklch, var(--destructive) 85%, transparent); color: white; }
.nrx-btn-danger:hover { background: var(--destructive); }
.nrx-btn-main { border-color: transparent; background: oklch(0.96 0 0); color: oklch(0.15 0 0); }
.nrx-btn-main:hover { background: white; }
/* Kopf eines Popups (Suchfeld), bleibt beim Scrollen stehen: fast deckend, sonst schiene die Liste durch */
.nrx-pop-head { background: oklch(0.19 0.002 270 / 0.97); border-bottom: 1px solid oklch(1 0 0 / 0.08); }
/* Zeilen in Popups */
.nrx-item {
  display: flex;
  width: 100%;
  align-items: center;
  gap: 0.625rem;
  min-height: 2rem;
  border-radius: 8px;
  padding: 0.375rem 0.625rem;
  text-align: left;
  font-size: 13.5px;
  transition: background-color 0.12s;
}
.nrx-item:hover, .nrx-item[data-active="1"] { background: oklch(1 0 0 / 0.09); }
.nrx-sep { height: 1px; margin: 0.3rem 0.4rem; background: oklch(1 0 0 / 0.09); }
.nrx-head { padding: 0.45rem 0.625rem 0.25rem; font-size: 11.5px; font-weight: 500; color: var(--muted-foreground); }
/* Punktbild im Sprachmodus */
@keyframes nrx-breathe { 0%, 100% { opacity: .85 } 50% { opacity: 1 } }
.nrx-orb { animation: nrx-breathe 3.2s ease-in-out infinite; }
}
"####;
