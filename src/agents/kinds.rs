//! Agent-Arten: Programm, npm-Paket, Modelle, Denk-Stufen, Anmeldearten und Anmeldebefehl je
//! CLI. `GET /agents` liefert das mit Installations- und Kontostatus, `GET /agents/<kind>/models`
//! die Modelle (bei Codex, OpenCode und Kilo vom CLI selbst gelistet).

use super::{accounts, tools};
use crate::auth::CurrentUser;
use crate::error::{AppError, AppResult};
use crate::AppState;
use nrzx_shared::agents::{AgentInfo, AuthKindInfo, ModelList, ModelOption};
use nrzx_shared::AgentKind;
use rocket::serde::json::Json;
use rocket::{get, routes, Route, State};
use std::time::Duration;

pub fn routes() -> Vec<Route> {
    routes![list, models]
}

/// Anmeldeart einer Agent-Art.
pub struct AuthSpec {
    pub key: &'static str,
    pub label: &'static str,
    pub env_vars: &'static [&'static str],
    pub hint: &'static str,
}

/// Beschreibung eines CLIs.
pub struct Spec {
    pub kind: AgentKind,
    /// Programmname(n) – der erste ist der übliche
    pub programs: &'static [&'static str],
    pub package: &'static str,
    pub models: &'static [(&'static str, &'static str)],
    pub efforts: &'static [&'static str],
    pub auth: &'static [AuthSpec],
    pub login_command: &'static str,
    pub login_help: &'static str,
    pub supports_resume: bool,
}

const PROVIDER_KEYS: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "OPENAI_API_KEY",
    "OPENROUTER_API_KEY",
    "GOOGLE_GENERATIVE_AI_API_KEY",
    "XAI_API_KEY",
    "DEEPSEEK_API_KEY",
    "GROQ_API_KEY",
    "MISTRAL_API_KEY",
];

pub static SPECS: [Spec; 6] = [
    Spec {
        kind: AgentKind::Claude,
        programs: &["claude"],
        package: "@anthropic-ai/claude-code",
        models: &[
            ("claude-opus-5-5", "Opus 5.5"),
            ("claude-fable-5-2", "Fable 5.2"),
            ("claude-sonnet-5-5", "Sonnet 5.5"),
            ("claude-haiku-5-5", "Haiku 5.5"),
            // Kurznamen des CLIs: immer das jeweils neueste Modell der Reihe
            ("opus", "Opus"),
            ("fable", "Fable"),
            ("sonnet", "Sonnet"),
            ("haiku", "Haiku"),
        ],
        // Claude Code kennt zwischen „high“ und „max“ noch „xhigh“ (Extra hoch)
        efforts: &["low", "medium", "high", "xhigh", "max"],
        auth: &[
            AuthSpec {
                key: "login",
                label: "Claude-Abo (Anmeldung im CLI)",
                env_vars: &[],
                hint: "Pro/Max/Team-Abo: Anmeldung im Terminal mit „claude auth login“.",
            },
            AuthSpec {
                key: "oauth_token",
                label: "OAuth-Token (claude setup-token)",
                env_vars: &["CLAUDE_CODE_OAUTH_TOKEN"],
                hint: "Langlebiges Token eines Claude-Abos, erzeugt mit „claude setup-token“.",
            },
            AuthSpec {
                key: "api_key",
                label: "API-Schlüssel (Anthropic Console)",
                env_vars: &["ANTHROPIC_API_KEY"],
                hint: "Abrechnung nach Verbrauch über die Anthropic Console.",
            },
        ],
        login_command: "claude auth login",
        login_help: "Öffne den angezeigten Link, melde dich mit deinem Claude-Konto an und füge den Code danach unten ins Feld „Code aus dem Browser“ ein. Claude Code zeigt den Code im Terminal nicht an.",
        supports_resume: true,
    },
    Spec {
        kind: AgentKind::Codex,
        programs: &["codex"],
        package: "@openai/codex",
        models: &[
            ("gpt-6.1-sol", "GPT-6.1 Sol"),
            ("gpt-6-astra", "GPT-6 Astra"),
            ("gpt-6-sol", "GPT-6 Sol"),
            ("gpt-6-luna", "GPT-6 Luna"),
            ("gpt-6.1-terra", "GPT-6.1 Terra"),
            ("gpt-5.6-sol", "GPT-5.6 Sol"),
            ("gpt-5.5", "GPT-5.5"),
        ],
        efforts: &["low", "medium", "high", "max"],
        auth: &[
            AuthSpec {
                key: "login",
                label: "ChatGPT-Abo (Anmeldung im CLI)",
                env_vars: &[],
                hint: "Plus/Pro/Team: Anmeldung mit Geräte-Code („codex login --device-auth“).",
            },
            AuthSpec {
                key: "api_key",
                label: "API-Schlüssel (OpenAI)",
                env_vars: &["OPENAI_API_KEY"],
                hint: "Abrechnung nach Verbrauch über die OpenAI-Plattform.",
            },
        ],
        login_command: "codex login --device-auth",
        login_help: "Öffne den angezeigten Link, melde dich bei ChatGPT an und gib den angezeigten Geräte-Code ein. Das Terminal meldet danach die erfolgreiche Anmeldung.",
        supports_resume: true,
    },
    Spec {
        kind: AgentKind::Gemini,
        programs: &["gemini"],
        package: "@google/gemini-cli",
        models: &[
            ("auto", "Auto"),
            ("pro", "Pro"),
            ("flash", "Flash"),
            ("flash-lite", "Flash Lite"),
            ("gemini-3-pro-preview", "Gemini 3 Pro (Vorschau)"),
            ("gemini-3-flash-preview", "Gemini 3 Flash (Vorschau)"),
            ("gemini-2.5-pro", "Gemini 2.5 Pro"),
            ("gemini-2.5-flash", "Gemini 2.5 Flash"),
        ],
        efforts: &[],
        auth: &[
            AuthSpec {
                key: "login",
                label: "Google-Konto (Anmeldung im CLI)",
                env_vars: &[],
                hint: "Gemini Code Assist / Google AI Pro: Anmeldung mit Google im Terminal.",
            },
            AuthSpec {
                key: "api_key",
                label: "API-Schlüssel (Google AI Studio)",
                env_vars: &["GEMINI_API_KEY"],
                hint: "Schlüssel aus Google AI Studio.",
            },
        ],
        login_command: "gemini",
        login_help: "Wähle „Login with Google“, öffne den angezeigten Link, melde dich an und füge den Code hier ein. Danach mit „/quit“ beenden.",
        supports_resume: true,
    },
    Spec {
        kind: AgentKind::Opencode,
        programs: &["opencode"],
        package: "opencode-ai",
        models: &[
            ("opencode/big-pickle", "Big Pickle (OpenCode Zen)"),
            ("anthropic/claude-opus-5-5", "Claude Opus 5.5"),
            ("openai/gpt-6.1-sol", "GPT-6.1 Sol"),
            ("google/gemini-3-pro-preview", "Gemini 3 Pro"),
        ],
        efforts: &["low", "medium", "high", "max"],
        auth: &[
            AuthSpec {
                key: "login",
                label: "Anbieter im CLI anmelden",
                env_vars: &[],
                hint: "„opencode auth login“: Anbieter wählen (z. B. Claude-Abo, ChatGPT, OpenCode Zen) und anmelden.",
            },
            AuthSpec {
                key: "api_key",
                label: "API-Schlüssel eines Anbieters",
                env_vars: PROVIDER_KEYS,
                hint: "Schlüssel eines Anbieters als Umgebungsvariable; das Modell dann als „anbieter/modell“ wählen.",
            },
        ],
        login_command: "opencode auth login",
        login_help: "Wähle mit den Pfeiltasten einen Anbieter und folge den Anweisungen (Link öffnen bzw. Schlüssel einfügen).",
        supports_resume: true,
    },
    Spec {
        kind: AgentKind::Kilo,
        programs: &["kilo", "kilocode"],
        package: "@kilocode/cli",
        models: &[
            ("kilo/~anthropic/claude-opus-latest", "Claude Opus (Kilo)"),
            ("kilo/~anthropic/claude-sonnet-latest", "Claude Sonnet (Kilo)"),
            ("kilo/~openai/gpt-sol-latest", "GPT Sol (Kilo)"),
            ("kilo/~google/gemini-pro-latest", "Gemini Pro (Kilo)"),
            ("kilo/~x-ai/grok-latest", "Grok (Kilo)"),
        ],
        efforts: &["low", "medium", "high", "max"],
        auth: &[
            AuthSpec {
                key: "login",
                label: "Kilo-Konto oder Anbieter im CLI anmelden",
                env_vars: &[],
                hint: "„kilo auth login“: Kilo Gateway oder einen anderen Anbieter anmelden.",
            },
            AuthSpec {
                key: "api_key",
                label: "API-Schlüssel eines Anbieters",
                env_vars: PROVIDER_KEYS,
                hint: "Schlüssel eines Anbieters als Umgebungsvariable; das Modell dann als „anbieter/modell“ wählen.",
            },
        ],
        login_command: "kilo auth login",
        login_help: "Wähle mit den Pfeiltasten einen Anbieter (z. B. Kilo Gateway) und folge den Anweisungen.",
        supports_resume: true,
    },
    Spec {
        kind: AgentKind::Grok,
        programs: &["grok"],
        // Offizielles Grok Build CLI. @vibe-kit/grok-cli nutzt die abgeschaltete Live Search
        // (HTTP 410) und ist nicht mehr brauchbar.
        package: "@xai-official/grok",
        models: &[
            ("grok-4.7", "Grok 4.7"),
            ("grok-4.7-build-fast", "Grok 4.7 Build Fast"),
            ("grok-4.6", "Grok 4.6"),
            ("grok-4.5", "Grok 4.5"),
        ],
        efforts: &["low", "medium", "high", "xhigh", "max"],
        auth: &[
            AuthSpec {
                key: "api_key",
                label: "API-Schlüssel (xAI)",
                env_vars: &["XAI_API_KEY", "GROK_API_KEY"],
                hint: "Schlüssel aus der xAI-Konsole (console.x.ai). Grok nutzt damit die Agent Tools API.",
            },
            AuthSpec {
                key: "login",
                label: "Grok-Konto (Anmeldung im CLI)",
                env_vars: &[],
                hint: "SuperGrok oder X Premium: „grok login --device-auth“. Den Code aus dem Terminal im Browser bestätigen.",
            },
        ],
        login_command: "grok login --device-auth",
        login_help: "Im Terminal erscheinen ein Link und ein Geräte-Code. Öffne den Link im Browser und bestätige dort genau diesen Code. Das Terminal meldet danach die Anmeldung. Den Code nicht ins Terminal eintippen.",
        supports_resume: true,
    },
];

pub fn spec(kind: AgentKind) -> &'static Spec {
    SPECS.iter().find(|s| s.kind == kind).unwrap_or(&SPECS[0])
}

impl Spec {
    pub fn auth(&self, key: &str) -> Option<&AuthSpec> {
        self.auth.iter().find(|a| a.key == key)
    }

    pub fn model_label(&self, id: &str) -> String {
        self.models.iter().find(|(m, _)| *m == id).map(|(_, l)| l.to_string()).unwrap_or_else(|| id.to_string())
    }

    /// Stufe passend für das CLI (leer = Vorgabe des CLIs).
    pub fn effort(&self, effort: &str) -> Option<&'static str> {
        self.efforts.iter().find(|e| **e == effort).copied()
    }

    pub fn auth_infos(&self) -> Vec<AuthKindInfo> {
        self.auth
            .iter()
            .map(|a| AuthKindInfo {
                key: a.key.into(),
                label: a.label.into(),
                env_vars: a.env_vars.iter().map(|s| s.to_string()).collect(),
                hint: a.hint.into(),
            })
            .collect()
    }

    pub fn builtin_models(&self) -> Vec<ModelOption> {
        self.models.iter().map(|(id, label)| ModelOption { id: id.to_string(), label: label.to_string() }).collect()
    }
}

/// `GET /agents` – alle Agent-Arten mit Programm, Version und den Konten des Benutzers.
#[get("/agents")]
async fn list(state: &State<AppState>, user: CurrentUser) -> AppResult<Json<Vec<AgentInfo>>> {
    let all = accounts::list_for(state, user.id()).await?;
    let mut out = Vec::new();
    for s in SPECS.iter() {
        let bin = tools::find_agent(state, s.kind);
        let version = match &bin {
            Some((p, _)) => tools::version_of(state, p).await,
            None => String::new(),
        };
        out.push(AgentInfo {
            kind: s.kind,
            name: s.kind.name().into(),
            program: s.programs[0].into(),
            npm_package: s.package.into(),
            installed: bin.is_some(),
            version,
            bin_path: bin.map(|(p, _)| p.to_string_lossy().into_owned()).unwrap_or_default(),
            models: s.builtin_models(),
            efforts: s.efforts.iter().map(|e| e.to_string()).collect(),
            auth_kinds: s.auth_infos(),
            login_command: s.login_command.into(),
            login_help: s.login_help.into(),
            supports_resume: s.supports_resume,
            accounts: all.iter().filter(|a| a.kind == s.kind).cloned().collect(),
        });
    }
    Ok(Json(out))
}

/// `GET /agents/<kind>/models?account=` – Modelle; Codex, OpenCode, Kilo und Grok listen sie selbst
/// (lokal, ohne kostenpflichtige Aufrufe), sonst die Vorschläge von nrzX.
#[get("/agents/<kind>/models?<account>")]
async fn models(state: &State<AppState>, user: CurrentUser, kind: &str, account: Option<i64>) -> AppResult<Json<ModelList>> {
    let kind = AgentKind::parse(kind).ok_or(AppError::NotFound)?;
    let s = spec(kind);
    let builtin = ModelList { models: s.builtin_models(), source: "builtin".into() };
    let Some((bin, _)) = tools::find_agent(state, kind) else {
        return Ok(Json(builtin));
    };
    let acc = match account {
        Some(id) => Some(accounts::get_row(state, user.id(), id).await?),
        None => accounts::default_for(state, user.id(), kind).await?,
    };
    let args: &[&str] = match kind {
        AgentKind::Codex => &["debug", "models"],
        AgentKind::Opencode | AgentKind::Kilo | AgentKind::Grok => &["models"],
        _ => return Ok(Json(builtin)),
    };
    let mut cmd = crate::platform::command(&bin);
    cmd.args(args).stdin(std::process::Stdio::null()).kill_on_drop(true);
    super::env::apply_base(&mut cmd, state);
    if let Some(acc) = &acc {
        for (k, v) in accounts::env_for(state, acc) {
            cmd.env(k, v);
        }
    }
    let out = match tokio::time::timeout(Duration::from_secs(40), cmd.output()).await {
        Ok(Ok(o)) if o.status.success() => o,
        _ => return Ok(Json(builtin)),
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let models = match kind {
        AgentKind::Codex => parse_codex_models(&text),
        AgentKind::Grok => parse_grok_models(&text),
        _ => text
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty() && l.contains('/') && !l.contains(' '))
            .take(400)
            .map(|l| ModelOption { id: l.to_string(), label: l.to_string() })
            .collect(),
    };
    if models.is_empty() {
        return Ok(Json(builtin));
    }
    Ok(Json(ModelList { models, source: "cli".into() }))
}

/// `codex debug models` → sichtbare Modelle nach Priorität.
fn parse_codex_models(text: &str) -> Vec<ModelOption> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else { return Vec::new() };
    let mut list: Vec<(i64, ModelOption)> = v["models"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|m| m["visibility"].as_str() != Some("hide"))
                .filter_map(|m| {
                    let id = m["slug"].as_str()?.to_string();
                    let label = m["display_name"].as_str().unwrap_or(&id).to_string();
                    Some((m["priority"].as_i64().unwrap_or(99), ModelOption { id, label }))
                })
                .collect()
        })
        .unwrap_or_default();
    list.sort_by_key(|(p, _)| *p);
    list.into_iter().map(|(_, m)| m).collect()
}

/// `grok models` – Zeilen `* id` bzw. `- id`, der Hinweis „(default)“ fällt weg.
fn parse_grok_models(text: &str) -> Vec<ModelOption> {
    text.lines()
        .filter_map(|l| {
            let rest = l.trim().strip_prefix('*').or_else(|| l.trim().strip_prefix('-'))?.trim();
            let id = rest.split_whitespace().next()?.trim();
            if id.is_empty() || id.contains('/') || id.contains('(') {
                return None;
            }
            Some(ModelOption { id: id.to_string(), label: id.to_string() })
        })
        .take(400)
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn codex_models() {
        let text = r#"{"models":[{"slug":"b","display_name":"B","visibility":"list","priority":2},{"slug":"a","display_name":"A","visibility":"list","priority":1},{"slug":"x","visibility":"hide","priority":0}]}"#;
        let m = super::parse_codex_models(text);
        assert_eq!(m.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), vec!["a", "b"]);
    }

    #[test]
    fn grok_models() {
        let text = "You are logged in with grok.com.\n\nDefault model: grok-4.7\n\nAvailable models:\n  * grok-4.7 (default)\n  - grok-4.7-build-fast\n  - grok-4.6\n";
        let m = super::parse_grok_models(text);
        assert_eq!(m.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), vec!["grok-4.7", "grok-4.7-build-fast", "grok-4.6"]);
    }
}
