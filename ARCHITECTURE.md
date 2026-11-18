# Architecture

nrzX is one Rust program: a web client for coding agents. It starts the agents' command line
programs – Claude Code, Codex, Gemini CLI, OpenCode, Kilo Code and Grok CLI – in workspaces that
are checked out from nrzGit, streams their output into a chat, and lets several agents (and several
subscriptions of the same agent) work side by side. Agents get nrzGit as an MCP server, so they can
create repositories, push, open pull requests and releases, build and push images, and check which
ports are taken.

```
┌──────────────────────────── Browser ─────────────────────────────┐
│ Dioxus app (WASM, ui/) – sidebar · chat · composer · right panel │
└──────────────┬──────────────────────────────┬────────────────────┘
               │ /api/v1 + /api/v1/events (SSE)│ :9301 preview proxy
┌──────────────▼──────────────────────────────▼────────────────────┐
│ Rocket 0.5 (src/)                                                 │
│  core: config · db · auth · hub (live events) · git · markdown    │
│  agents/     accounts · tools · threads · runs · subagents ·      │
│              MCP server for subagents · terminals (PTY)           │
│  workspace/  nrzGit connection · projects · worktrees · git ·     │
│              files · preview proxy · users · settings             │
│  voice/      Whisper speech recognition (candle)                  │
└───────┬───────────────────────┬───────────────────────┬──────────┘
        │ spawn (own HOME each) │ git / HTTP + token     │ MCP (Bearer token)
┌───────▼─────────┐     ┌───────▼──────────────────────────────────▼───┐
│ claude · codex  │     │ nrzGit: repos, issues, PRs, releases, ports,  │
│ gemini · opencode│    │ packages (container registry), MCP at /mcp    │
│ kilocode · grok │     └───────────────────────────────────────────────┘
└─────────────────┘
```

## Layout

| Path | What |
|---|---|
| `shared/` | Crate `nrzx-shared`: `common.rs` (agent kinds, thread/project summaries, messages, live events, diffs) plus `agents.rs`, `workspace.rs`, `voice.rs` and `team.rs` |
| `src/` | Server. Core modules at the top, one folder per area (`agents`, `workspace`, `voice`, `team`) |
| `migrations/` | `0001_init.sql` is the whole schema; areas may add `0002_agents.sql`, `0003_workspace.sql` |
| `ui/` | Dioxus 0.7 web app with rust-ui components; pages per area in `ui/src/pages/<area>/` |
| `tests/` | Integration tests; `tests/common/mod.rs` has helpers (users, projects with a git repo, threads) |

## Data folder

| Path | What |
|---|---|
| `nrzx.db` | SQLite: users, sessions, nrzGit connections, agent accounts, projects, threads, runs, messages, usage log (`usage_log`, kept when threads are deleted) |
| `accounts/<id>/home` | HOME of one agent account – `HOME`, `XDG_*`, `CLAUDE_CONFIG_DIR`, `CODEX_HOME` … point here, so two Claude subscriptions never share a login |
| `shims/security` | macOS only: first in the `PATH` of account processes. An account HOME has no keychain, so writes to the default keychain (Claude Code saving its login) would make macOS show “Keychain Not Found”; the shim fails them quietly (exit 37, as without a keychain) and the CLIs keep using their files in the account HOME. Everything else goes to `/usr/bin/security` |
| `workspaces/<project>/repo` | main checkout of a project |
| `workspaces/<project>/worktrees/<thread>` | git worktree of a thread (branch `nrz/<slug>`) |
| `scratch/<thread>` | working folder of threads without a project |
| `bots/<bot>` | working folder (“computer”) of a bot, shared by all its conversations |
| `tools/` | agent CLIs installed by nrzX (`tools/npm`, npm prefix) |
| `attachments/` | files attached in the composer |
| `models/whisper-<size>/` | Whisper models for voice control (`config.json`, `tokenizer.json`, `model.safetensors` from `openai/whisper-<size>`), downloaded on demand |

## Conventions

Same as nrzGit: Rust only (no JavaScript/TypeScript/Python files; browser APIs through `web-sys`),
German source strings with English catalogs, camelCase JSON, Unix seconds, `AppResult`/`AppError`,
`sqlx::AssertSqlSafe` for dynamic SQL, Markdown rendered on the server (`markdown::render_plain`).
Everything the UI shows live goes through `hub::publish(state, user_id, HubEvent::…)`; the UI listens
on `GET /api/v1/events` (`ui/src/events.rs` keeps `state::THREADS`, `PROJECTS`, `MESSAGES`,
`CHANGES`, `PREVIEW` current). Spinners: `util::install_spin_sync` gives every spinner that enters the DOM a negative `animation-delay` matching the page clock (re-inserted ones – e.g. after the sidebar re-sorts – keep spinning instead of restarting), and they get `will-change: transform`. Do not tick a whole view every second: only leaf components read `parts::NOW` (e.g. the chat's status line), never `ChatBody`. These handlers run outside any component: use `spawn_forever`,
never `spawn` (no current scope – it panics and the UI reloads).

New thread from the home page: the composer sets `state::PENDING_THREAD` before `POST /threads`;
home shows `agents::PendingChat` and the sidebar a placeholder row until the response, then the
history is loaded and the route switches to the thread. File drops: `util::install_file_drop`
accepts files anywhere in the window (the WebView never opens them itself) and hands drops outside
a drop zone to the nearest visible composer (`data-composer`, `util::FILE_DROP`).

## Contracts between the areas

Server functions (signatures fixed):

| Function | Owner | Used by |
|---|---|---|
| `workspace::nrzgit_access(state, user) -> Option<NrzGitAccess{url, username, token}>` | workspace | agents (MCP config of runs) |
| `workspace::thread_workdir(state, thread) -> PathBuf` | workspace | agents (cwd of runs and terminals) |
| `workspace::agent_env(state, user) -> Vec<(String, String)>` | workspace | agents (git auth, docker registry) |
| `workspace::after_run(state, thread)` | workspace | agents (after each run) |
| `workspace::forget_thread(state, thread)` | workspace | agents (before deleting a thread: remove its worktree, keep the branch) |
| `agents::stop_thread(state, thread)` | agents | workspace (delete thread/project) |
| `agents::publish_thread(state, thread)` | agents | workspace (branch/worktree changed) |

UI components (names and props fixed): `agents::{ChatView{thread_id}, Composer{project_id, thread_id},
Lineage{thread_id}, AccountsSettings, ToolsSettings, TerminalView{id}, AgentIcon{kind, size}}` are
embedded by `workspace` (`AppLayout`, `Home`, `ThreadPage`, `Settings`).

API paths (under `/api/v1`):

| Area | Paths |
|---|---|
| core | `GET /health`, `POST /markdown`, `GET /events` (SSE) |
| agents | `/agents…` (kinds, models), `/accounts…`, `/tools…`, `/threads…` (incl. messages, runs, stop, subagents), `/attachments…`, `/terminals…`, `/bots…` (conversation, routines) |
| team | `/projects/<id>/team` (view, switch on/off), `/projects/<id>/team/messages`, `/projects/<id>/team/sync` |
| workspace | `/auth/…`, `/me…`, `/users/…`, `/settings/…`, `/nrzgit…`, `/projects…`, `/workspace/<thread>/…` (worktree, branch, changes, diff, commit, push, pull request, files), `/previews…` |
| voice | `/voice/settings`, `/voice/models…` (list, download, delete), `/voice/transcribe` (WAV → text), `/voice/command` (sentence + grid windows → reply and actions) |

Outside the API: `/mcp` (MCP server for subagents, agents), `/attachments/<uuid>` (agents), the
preview proxy on `preview_port` (workspace).

## nrzGit

The user creates an access token in nrzGit (Settings → Applications, scope `mcp` or `all`) and pastes
URL and token into nrzX (Settings → nrzGit). nrzX then

- lists and creates repositories through the nrzGit REST API (`/api/v1/me/repos`, `POST /api/v1/repos`),
- clones and pushes with the token (HTTP Basic, user name + token),
- gives every agent run the MCP server `<nrzgit>/mcp` (Streamable HTTP, `Authorization: Bearer <token>`)
  next to its own MCP server (`/mcp` of nrzX, a token per run) for subagents and previews.

nrzX connects to nrzGit only (no luGit, no GitHub): `nrzgit_connections` keeps one row per user
(`kind` = `nrzgit`; rows of other kinds from older databases are ignored), `PUT /nrzgit?kind=` only
accepts `nrzgit`, and agents get nrzGit as their only git MCP server (`NRZGIT_*` in the environment).

Every main thread's system hint (`runs::system_hint`, not subagents or read-only threads) tells the
agent to ship finished code changes without being asked (`runs::SHIP_HINT`) – unless the user
explicitly says not to: bump the version, commit and push, publish a release the way the project
does (on another branch than the default: push and open a pull request instead), and update the
user's nrzStore if the project is an app there.

## Building

Everything is Rust – also the build. `cargo xtask` (`xtask/src/main.rs`, alias in `.cargo/config.toml`)
replaces shell scripts:

```bash
cargo xtask ui           # writes ui/tailwind.css from ui/theme.rs, dx bundle → ui/dist/public
cargo build --release    # server; embeds UI and static files
cargo xtask macos        # nrzX.app + DMG in dist-macos/ (signed with "nrzX Local Signing")
cargo xtask release      # DMG as release v<version> on nrzGit (NRZGIT_TOKEN)
```

The styles live in `ui/theme.rs` (`TAILWIND_CSS`, Tailwind v4 input) – the generated `tailwind.css`
is not in the repository. The database schema lives in `src/migrations.rs` (`MIGRATIONS`, run by
`db::migrator` through sqlx with the usual `_sqlx_migrations` checksums).

## Speech

The UI records with `getUserMedia` + Web Audio (`ui/src/mic.rs`; microphone chosen per device in
Settings → Sprechen, kept in `localStorage`, falling back to the system default), cuts sentences at
speech pauses, resamples to 16 kHz and posts each sentence as WAV to `/voice/transcribe`. The server
recognizes it with Whisper in pure Rust (`src/voice/whisper.rs`, candle; on macOS through Accelerate).
Used by dictation in the composer (microphone chip, `mic::dictate_start/stop`). The interface is German only (`i18n::tr` returns the
German source text); there are no translation catalogs.

Bots (persistent AI teammates, `src/agents/bots.rs`) still exist on the server, but nrzX has no UI for
them.

## Referencing other repositories (`@@`)

Typing `@@` in the composer lists the user's other projects (and, below them, repositories of
the connected platforms that are not projects yet – choosing one imports it, cloning in the
background). The message keeps the plain token, e.g. `@@payment-api`. When a run starts,
`workspace::referenced_repos` resolves every `@@name` (project name with spaces as `-`, `owner/name`
or just the repository name; only the user's own projects, not the thread's own one, at most 8,
waiting briefly for a clone) to the project's main checkout. The CLI gets a short "Referenced
repositories" section with path and branch after the prompt – read for reference, e.g. to integrate
two projects, not to modify – and access to the folders (`--add-dir` for Claude Code,
`--include-directories` for Gemini; the other CLIs read absolute paths anyway).

## Interjections

"Interject" (⌘/Ctrl+Enter, or "send now" on a queued message) puts the message into the queue with
`interrupt: true` and signals a soft stop (`runs::signal_soft_stop`). The run ends at the next
suitable moment – a tool or an answer just finished and no tool is open – or after
`runs::SOFT_STOP_GRACE` (20 s), as `Outcome::Interrupted`: no "Stopped" line, subagents keep
working, and the queue goes out right away. The CLI gets `messages::INTERJECTION_NOTE` in front of
the text; the chat shows only the text (`UserMessageData.interjection`, older messages are stripped
when loaded). "MCP servers not connected" is reported only when the list changes in a thread.

## Agent steps in the chat

Tool steps of a run are shown as they happen, as quiet lines between the answers (no "worked for"
header): reads, searches and thoughts collapse into one line ("2 files read · 1 search",
"Thought"), consecutive commands into "Ran 3 commands" – a click lists every `$ cmd` with its
output –, edits become code blocks with line numbers, red/green lines and syntax colours
(`ui/src/components/highlight.rs`). For edits the run
writer remembers the file when the tool is announced and cuts the changed sections with three lines of
context once it is done (`src/agents/edits.rs`, Myers diff in `shared/src/diff.rs`); agents that only
report afterwards (Codex) fall back to the old/new text, the last content seen in the run or `HEAD`.

## Frame (nrzX)

nrzX is a fork of nrzCode without the canvas mode. The interface is panels with a clear edge over the
window ground (`.nrx-backdrop`, `.nrx-panel` in `ui/theme.rs`, inside `@layer components` so Tailwind
utilities still win). In the app everything is see-through: the ground tints the system's glass
a dark neutral gray (`--nrx-ground`), panels a little more (`--nrx-surf`); the edges keep the
shapes clear. Because two translucent surfaces must not
overlap, a panel draws its edge with `::after` and cuts it out (`--nrx-notch`, `clip-path`) where a
tab joins it; tabs have fixed widths for that (`--nrx-tab`, `--nrx-task`), leave their bottom pixel
row empty and draw their own edge (`.nrx-line`) only down to the rounded fillets (`.nrx-fl`/`.nrx-fr`).

* **Tabs** (`ui/src/pages/workspace/tabs.rs`, `views.rs`): as in the sketch. Right of the traffic
  lights the sidebar toggle; "+ Neue Aufgabe" hangs flush right on the task panel. Above the chat
  panel the tabs of the open thread: first the chats of its row (each one a thread), then a new chat
  not yet sent, the built-in browser and any number of terminals in the working folder. "+" opens a
  new chat right away ("Neuer Chat": a composer for a new thread in the same project,
  `views::NewChatPane`); once sent, the new thread stands right of the others as a chat tab and
  opens. A click on another chat opens its thread (the route changes, the row stays); X takes a chat
  out of the row (the thread stays; your own chat only while another one is left). Which chats share
  a row is kept in `localStorage` (`nrz-chat-tabs`). Terminals open with the terminal icon in the
  chat header. Tabs only appear once there is a second one; with just the chat the panel reaches the
  top and "+" sits in the chat header. The agent's own CLI is not a tab but a mode of the chat (see
  Native CLI). All open views stay loaded on top of each other (only the chosen one is visible). Every tab has
  the same width; the chosen one joins the panel (at its top-left corner, otherwise with fillets on
  both sides – `--nrx-ai` tells the cut-out where). Collapsed, the task panel is gone completely and the
  chat panel takes the full width: the traffic lights, the sidebar toggle (exactly the same spot) and
  "new task" sit free on the window ground, and right of them the chat panel's surface reaches up to
  the top (`.nrx-tab-big` with the header inside, `.nrx-notch-big`); with tabs of the thread those
  follow instead (`--nrx-x0`). ⌘K
  opens the task search. Free space in the rows, the rail and the chat header drag the window.
* **Bots** (`ui/src/pages/workspace/bots.rs`, `src/agents/bots.rs`): the bot button next to the
  sidebar toggle switches to bot mode (`/bots`, `/bots/<id>`): the sidebar lists the bots (pixel-cat
  avatars `px:<hue>` or an emoji, last answer below), the tab becomes "Neuer Bot". A bot is an expert
  for one thing (`role`) with instructions, its own memory (it saves facts with nrzx `remember`, the
  user can edit them), its own folder, routines (time + weekdays), its own conversation threads
  (`bot_id`) and its own MCP servers (`bots.mcp`, `BotMcp`: URL with optional token or a command) –
  added only to its runs (Claude `mcpServers`, Codex `mcp_servers.*`, OpenCode `mcp`). Open
  conversations stand as tabs in the top row like a thread's chats (`bots::conversations`,
  `tabs::BotTabs`, labelled with their start time): a click makes one the bot's current conversation
  (`POST /bots/<id>/conversation/<tid>`, set here first so the switch is instant), "+" starts a new one
  next to the others (`POST /bots/<id>/conversation`), X closes a tab (the conversation stays under
  "Gespräche" in the menu; the last tab stays). Which conversations are open is kept in
  `localStorage` (`nrz-bot-tabs`); the bot's current conversation is always one of them.
* **Deep Research** (`ui/src/pages/workspace/research.rs`, `src/agents/research.rs`): the telescope
  next to the bot button switches to research mode (`/research`, `/research/<id>`; the sidebar lists
  the researches, the tab becomes "Neue Recherche"). The start page looks like "Neue Aufgabe": the
  composer with `research: true` creates a thread with `threads.research` (no project, no questions,
  folder `research/<id>`) whose system hint makes it a deep-research agent – plan, many searches,
  read and cross-check sources – that writes a Markdown research paper to `paper.md`, created early
  and updated as it goes. Instead of a chat the page shows that paper (`GET /threads/<id>/paper`,
  Markdown and safe HTML, polled every 3 s while it runs) with progress above it (searches and read
  sources from the run's `web` tool calls), stop, and on the right a conversation with a bot that sees
  the paper (`POST /threads/<id>/discuss`: a thread with `discuss_of`, same folder and agent, the paper
  in its system hint). Deleting a research removes its conversation and folder.
* **Feels like an app**: `AppLayout` keeps the last four opened threads mounted (`KEPT`, only the
  shown one is visible; `SHOWN_THREAD` keeps hidden ones from marking messages read or opening the
  side column), so switching back reloads nothing – browser pages, terminals, CLI and the scroll
  position stay. `overlay::NATIVE_SCRIPT` blocks the browser context menu, reload/back shortcuts,
  dragging, rubber-band scrolling and the hand cursor. In the background the interface stays as it
  is: the app's own web views are neither throttled nor unloaded (`with_background_throttling(Disabled)`,
  WebKit's `inactiveSchedulingPolicy` none, macOS 14+ – otherwise WebKit suspends a hidden view after
  a few minutes and it comes back reloaded); should WebKit's content process still end (crash,
  memory pressure), the window reloads instead of staying empty (`UserEvent::WebContentTerminated`).
* **Traffic lights** (`macos/src/lights.rs`): an empty native toolbar (style unified; compact in the
  settings window) makes AppKit place them lower and further in itself – centre 26 pt, close button
  from x 19 – and keep them there while resizing, zooming, after title changes and full screen
  (setting them by hand fought AppKit's layout and made them jump). Clicks there still reach the
  web view. The app reports the measured spot (`--nrz-tl-y`, `--nrz-tl-r`); the top row and the rail
  follow it. The main window is at least 820 px wide, so the phone layout (< 768 px) never shows.
* **Only errors bottom right**: there are no info or confirmation toasts (queued message, copied,
  saved, notices …) and no update box – only `toast_error`. Its timer starts with `spawn_forever`:
  errors triggered by server events run outside any component, where `spawn` panicked and the
  interface reloaded. Notifications (`ui/src/notify.rs`, off or "Im Hintergrund") only go to the
  system while nrzX is in the background; in the foreground the tone and the row's colour tell.
* **No crash, no reload**: listeners on `window` belong to their component and leave with it
  (`util::WindowKeys`, removed in `use_drop`) – a keyup listener of the composer used to outlive it
  and read its dropped signals on every key press (`ValueDroppedError`), so the interface crashed
  and reloaded after opening a thread. If the interface still panics, `install_panic_hook`
  (`ui/src/main.rs`) reports it to the app log (`POST /api/v1/client/error` via `sendBeacon`,
  "Oberfläche abgestürzt: …" in `~/Library/Logs/nrzX/nrzx.log`) and restarts silently on the same
  page; only three crashes within a minute leave a small notice with "Neu starten".
* **Dictation** (`DictateButton` in `composer.rs`, `mic.rs`): live – while speaking, the sentence so
  far goes to Whisper every ~0.4 s (`mic::Event::Partial`, one request at a time,
  `/voice/transcribe?fast=true` → `Engine::transcribe_fast`: one pass over a shorter window, ~0.2 s
  with the base model instead of 0.5–0.7 s) and is typed into the field character by character; at
  the pause the exact recognition of the whole sentence replaces the draft.
  Whisper re-reads the whole sentence each time and likes to change earlier words; `mic::Steady`
  keeps words that two partials agreed on fixed (LocalAgreement), and corrections to text already
  shown happen in place – only new characters are typed, nothing starts over. The field grows with
  its text (up to about twelve lines, then it scrolls); the chat above does not move while typing –
  measuring sets the field to `auto` height for a moment, so `composer::grow` keeps the scroll
  position of every chat (`data-chat-scroll`), and the chat follows only new messages of its own
  thread (a memo over its messages, not every change of `MESSAGES`).
* **Claude Code in terminals** (`accounts::prepare_claude`): before the CLI or a thread terminal
  starts, the account's Claude config marks the working folder (and nrzX's work folders) as trusted
  and the bypass warning as accepted, and makes bypass the default mode – no "Yes, I trust this
  folder", no confirmation. Chat runs and the CLI pass their mode explicitly (read-only: default).
* **Popups** (`.nrx-pop`, `Dropdown`/`MenuItem`, `Modal`, `Confirm` in `components/overlay.rs`):
  the same dark gray glass as the panels with its own blur, clear edge and shadow – project picker,
  context menus, modals and the confirmation ("Thread löschen?", `.nrx-dialog`, buttons `.nrx-btn`,
  `.nrx-btn-danger`, `.nrx-btn-main`); the selected entry carries a check. The usage popup shows a
  card per account with each limit (5 hours, week, special limits): percent used, a bar and when it
  resets; the trigger in the sidebar shows "5 h … % · Woche … %" of the most used account
  (`usage::headline`).
* **Tasks** (`ui/src/pages/workspace/shell.rs`): "+ New task", the threads with a state ring, a
  heading – the project's name for threads in a project (the request shows on hover), else the
  thread's title – and one line below: the start of the last answer (`ThreadSummary.snippet`, cut on
  the server), what is happening or "Wartet auf deine Antwort". Rows are tinted by outcome
  (`state::tone`, `.nrx-tone-*` in `ui/theme.rs`): green done – only until you open the chat (read
  turns `Done` into `Idle`, which has no green) –, red error or stopped – stays after reading because
  the server sends the last run's status (`ThreadSummary.last_run`) –, yellow and gently blinking
  while the agent really waits for an answer (`ThreadSummary.asking`: unanswered, not expired and no
  own message after it; a question that runs out (`QuestionData.expired`, the agent decides itself
  and goes on) and a working agent are never yellow). Changing to done, question or error plays a short Web Audio tone
  (`ui/src/sound.rs`, no files) – in the foreground; in the background the system notification
  sounds instead (only with notifications off the app plays it then too, never twice). At the bottom project and
  the most used subscription limit – a click shows the limits per account. Search: magnifier in the
  tab row. The project for new tasks is chosen left above the composer on the home page
  (`shell::ProjectChooser`), opposite the model picker.
* **Chat** (`thread.rs`, `layout::CenterBar`): at the top "+" (new chat), the CLI toggle, a new
  terminal, the built-in browser and the environment column (panel icon). Switching to a chat
  (sidebar or tab) jumps to its newest message and follows again. Work between two
  answers folds behind "3 steps so far" (open while the agent works) or "Worked for 12s"; pages the
  agent wrote (`.html`) appear as chips. The composer is one line ("› Schreib an Claude …") with only the
  microphone chip (mic + ⌥; tapping ⌥ alone toggles dictation): Enter sends, ⌘Enter interjects,
  files come by drop or a double-click into empty space (a double-click on a word just selects it,
  `composer::word_selected`), stopping is in the state line above. Above it the state (thread)
  or the project (home) on the left and the model picker on the right.
* **Browser** (`browser.rs`): address line (globe, path or URL, editable), "Open in browser", close.
  Agent servers go through the preview proxy, workspace files through `/raw/<token>/<path>`
  (`POST /workspace/<id>/raw-token`, 12 h): the page gets a sandboxed, empty origin
  (`Content-Security-Policy: sandbox …`), so it never sees the session, while relative links still
  work. "Open in browser" uses the app's `open-external:` message (http(s) and local `.html` files only).

## Team

Several people – each with their own nrzX on any platform – work on the same repository and
their agents talk to each other (`src/team/mod.rs`, `shared/src/team.rs`, `ui/src/pages/workspace/team.rs`,
tab "Team" in the right column). There is no server for it: the shared state (`TeamDoc`: members with
their agents – thread number, title, status, branch – and the messages) is `team.json` in the ref
`refs/nrzx/team` of the shared repository. Every instance polls it with `git ls-remote` (5 s while
agents work or the tab is open, else 40 s), fetches, merges and pushes a new commit on top; a
rejected push (somebody else wrote) is retried with the new state. Access = access to the repository;
the member key is the user name on the git platform. `team_projects` keeps the last known state per
project, `team_outbox` unsent messages, `team_seen` what each thread already got. Agents of main
threads get `team_status` / `team_send` / `team_read` and a `TEAM` section in the system hint (their
address `name#thread`, teammates, new messages). With "agents answer on their own" (`auto_reply`, off
by default) a message to `name#thread` starts that thread (at most 8 times per hour); team messages
are always framed as coming from collaborators, not from the user. Works for local-only users of one
server too (two projects on the same remote). On the phone the team API passes the nrzXMobile
proxy (`/projects/…`). API: [docs/api-team.md](docs/api-team.md).

## AI chains

Settings → AI chains: an order of accounts, e.g. Claude subscription 1 → Claude subscription 2 →
Codex (`src/agents/chains.rs`, stored per user in `settings`). When a run stops at a rate limit the
thread switches right away to the next step that is not at its limit itself and continues with the
history (a new session, since sessions belong to CLI and account); `preflight` switches before a run
when the account is known to be at its limit. Without a matching chain the thread waits for the reset
as before.

## Native CLI

The CLI button left of the globe in the top bar (`cli::CliToggle` in
`ui/src/pages/workspace/cli.rs`) swaps this chat's interface for the agent's own interactive CLI and
back – only for this chat, without a tab: `POST /threads/<id>/cli` (`src/agents/terminals.rs`) starts the program directly in a
PTY (no shell; Windows npm shims resolved by `platform::resolve_program`) in the thread's worktree, with
the account's environment, the thread's model and `--resume <session>` (`codex resume`, `-s` …). The
view is a `TerminalPane` with `themed: true` – app colours, `--term-*` palette per theme, translucent
over the glass. Switching is instant: chat and CLI are stacked layers of the thread page (the hidden
one `invisible`, still laid out, so the terminal keeps its size) and the CLI keeps running while the
chat shows. It is started ahead in the background – on hovering the button, when opening a thread whose CLI
was used (`localStorage` `nrz-cli-threads`) and after every run. Only one program writes a session at
a time: a run starting closes the thread's CLI (`terminals::close_cli` in `runs.rs`, exit code `-2`, the
UI restarts it after the run with the new session state), opening it while a run works returns `409`;
at most four CLIs run per user (the oldest is closed). The next run resumes the same session and so
sees what happened in the CLI.

## App window

The native macOS app (`macos/`) shows the interface in a transparent window with the
system's frosted glass behind it – blurred, not just see-through (`window-vibrancy`, `apply_glass`):
macOS vibrancy `UnderWindowBackground`, applied after the webview because wry replaces tao's content
view with its own (the view `Window` still reports is detached); Windows 11 22H2+ its own acrylic
(`macos/src/glass.rs`: `SetWindowCompositionAttribute`, tinted like the theme – the system acrylic
`DWMSBT_TRANSIENTWINDOW` follows the system's light/dark and turns into a flat colour whenever the
window is not in front), older Windows 10/11 blur behind the window (their acrylic lags when
dragging). Without it the window stays opaque. `overlay::GLASS_SCRIPT` sets
`window.__nrzGlass` and `html.nrz-glass` before the first paint, so the interface only tints its
surfaces (`ui/tailwind.css`; off under Settings → Appearance, `localStorage` `nrz-glass`). The
interface talks to the app through `window.ipc` (`util::native`): `theme:<dark|light|system>` sets the
window appearance and with it the glass, `update:state` and `update:install` drive the update button
at the bottom of the sidebar (`ui/src/pages/workspace/update.rs` – only there while an update is
available, no box or message bottom right; the app answers with `window.__nrzUpdateState(status,
text)`: `available`, `progress`, `installing`, `failed`), and ⌘, (app menu) or Ctrl+, call
`window.__nrzOpenSettings()`.

Links never open a foreign page inside the app: `overlay::NATIVE_SCRIPT` catches clicks on links to
another origin (also `localhost` of agent servers, `mailto:`) and sends them through `window.open` to
the new-window handler, which opens everything but the built-in server (`is_own`: attachments → app
window) in the system browser. Should the main window still load a foreign page (link without
handler, redirect), the page-load handler – main frame only, so the built-in browser
(an iframe) stays – opens it in the system browser and loads the interface again (`UserEvent::Escaped`).

Adding a repository that already is a project never fails: `POST /projects/import` and
`POST /projects/create` return the existing project (200) and the interface selects it for the new
thread; creating a name that already exists on the platform imports that repository instead. Project
pickers (home, sidebar filter) have a search field (`overlay::MenuSearch`, Enter takes the first hit).
On the home page the search also finds nrzGit repositories that are not projects yet
(`shell::NrzGitHits`, `GET /nrzgit/repos` fresh each time the picker opens – new repositories show
up at once): a click imports the repository (cloned in the background) and selects it.

The right column of a thread (environment toggle in the chat bar) is one panel with quiet section
labels (`SectionLabel`): environment (changes, worktree, branch, commit & push), repository, pull
request, editor, actions (local servers) and usage (`agents::UsageSection`: per account the
remaining percent of the 5 h, weekly and special limits). It always starts collapsed. The chat and
the terminal follow new output only
while at the very bottom; scrolling up (wheel, touch) stops following at once. `overscroll-behavior:
none` sits only on the page and the large scroll areas – on every element it would stop the wheel
at each clipped (`overflow: hidden`) child, e.g. truncated thread titles.

## macOS permissions

The app is signed with a stable local certificate (`cargo xtask macos`, "nrzX Local Signing" in its
own keychain) so macOS keeps granted permissions across updates (ad-hoc signatures change with every
build). At start the app asks for missing folder permissions (Desktop, Documents, Downloads); the
microphone is asked for by WebKit on first use. A denied file access returns `403` with "Kein Zugriff:
macOS …" – the app then shows a dialog with the way into System Settings (`perm:hint`).

## Settings window, menu, taking over from nrzCode

Settings open in their own native window (`macos/src/main.rs::open_settings_window`, trusted like the
main window, glass, URL `/settings/<section>`): from the app menu "Einstellungen …" (⌘,) or the
UI's `settings:open`; its X sends `settings:close`. The macOS menu bar shows only the app menu (About,
"Nach Updates suchen …", "Update installieren …", "Einstellungen …", a nested "Bearbeiten" so ⌘C/⌘V
keep working, hide, quit) – no tray icon, no code lock, no view/window/help menus. The window is always
transparent (glass), the theme always dark.

Settings are minimal: Von nrzCode übernehmen, Profil, Nutzung, nrzGit, Agenten, KI-Ketten, Werkzeuge,
Sprechen. "Von nrzCode übernehmen" (`src/workspace/import.rs`, `GET/POST /api/v1/import/nrzcode`)
reads nrzCode's database read-only (`~/Library/Application Support/nrzCode/nrzcode.db`, falling back
to `/Users/$USER/…`, or `NRZX_NRZCODE_DIR`) for the user with the same name (else the first admin)
and takes over agent accounts with only their login files (`.claude/.credentials.json`,
`.codex/auth.json`, Gemini/OpenCode/Kilo/Grok config – not caches, toolchains or code), the nrzGit
connection and projects (nrzGit projects are cloned one after another, local ones copied). Imported
accounts stay linked (`agent_accounts.linked_home`): `accounts::sync_login` compares the credential
files with nrzCode's account home before runs, terminals and limit queries and every 30 s – the newer
file wins in both directions, because OAuth refresh tokens rotate and the app that renewed must pass
the new one on. Importing again refreshes and links existing accounts (same kind and label); existing
connections and projects are skipped. The model picker in the
composer can add accounts and sign in directly (`accounts::SignIn`).


## nrzXMobile

`mobile/` is a separate small program for the Umbrel (app `nrz-x-mobile` in the nrzStore): an
axum server with an embedded Dioxus UI for phones (`mobile/ui`). The PC never opens a port: after
pairing (Settings → Mobile: address and code from nrzXMobile, `POST <addr>/pair/claim` → token)
nrzX keeps an outgoing WebSocket to `<addr>/connect` (`src/mobile.rs`, reconnects with back-off).
nrzXMobile sends `{"t":"req",…}` for `/api/pc/<id>/v1/…` from the phone; nrzX runs it against
its own API on `127.0.0.1` with a session of the paired user (`Authorization: Bearer`, accepted only
from loopback; only threads, messages, usage, projects, agents and `me` are allowed) and answers
`{"t":"res",…}`. All hub events of the user go up as `{"t":"event",…}` and reach the phone as
Server-Sent Events. Accounts (`mobile/src/auth.rs`): username + password (Argon2id), the first
account is created on first visit and is admin (it can add more); sessions are random tokens in an
HttpOnly cookie `nrzm_session` (only the SHA-256 is stored in `store.json`, mode 0600, 30 days
sliding). Every PC has an `owner`; pairing codes belong to the account that created them, and
`/api/pcs`, `/events` and the proxy answer 404 for other accounts' PCs (admins included). Changing
requests need `X-Nrzm: 1` and a same-origin `Origin`; logins lock out after 5 failures per name
(20 per address); responses carry CSP/nosniff/DENY. In the UI, loads triggered from a view that
is about to unmount (opening a chat from the list) use `spawn_forever`, and the effect that opens the event stream and loads the chat list depends on both the sign-in state and the PC (reopening with a remembered PC and a valid session must load too). The phone UI is always dark. Updates from inside the app (`mobile/src/update.rs`, admins, Settings → App): latest tag `umbrel-X.Y.Z` of nrzXMobile on nrzGit (`NRZMOBILE_UPDATE_URL`, else the host the app is opened on, else the container gateway, port 8390), `SHA256SUMS` checked, the binary must run and report its version (`nrzx-mobile version`), it goes to `<data>/update/` and the running process `exec`s into it (same PID, no container restart); on start the store binary `exec`s into `<data>/update/` while that one is newer. The UI polls `/api/info` and offers „Neu laden“ when the server runs a newer version than the page. Build: `cargo xtask mobile` (UI styles from `mobile/ui/theme.rs`, server for this machine).
