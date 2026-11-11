//! Hermes Agent (Nous Research) wiring: find Hermes profiles on this PC — native Windows
//! (`%LOCALAPPDATA%\hermes`) and inside WSL distros (`~/.hermes`) — and point a profile at
//! this llama-server by writing a named provider into its `config.yaml`.
//!
//! What a push does (mirrors the macOS oMLX widget):
//! - backs up `config.yaml` to `config.yaml.bak.YYYYMMDD-HHMMSS` first
//! - writes `providers.neurallama = { name, base_url, api_key, transport, default_model }`
//! - sets `model.provider: neurallama` and `model.default: <alias>`
//! - removes stale per-provider keys from `model:` (they cause HTTP 409s in Hermes)
//! - sets `model.context_length` (or removes it when unknown); keeps every other key,
//!   including `_config_version`. Comments are not preserved (the backup keeps them).
//! - writes LF line endings, no BOM. Never touches `.env`.
//! - one profile per push; the UI never preselects `default` and asks twice for it.

use super::{HermesPushResult, HermesTarget};
use serde_yaml::{Mapping, Value};
use std::path::{Path, PathBuf};

pub const PROVIDER_KEY: &str = "neurallama";
const PROVIDER_NAME: &str = "Neurallama (llama.cpp)";
const STALE_MODEL_KEYS: [&str; 6] = ["base_url", "api_key", "api", "api_mode", "transport", "lmstudio_load_mode"];
const IDENTITY_FILES: [&str; 6] = ["config.yaml", ".env", "SOUL.md", "profile.yaml", "auth.json", "state.db"];

#[derive(Debug, Clone)]
pub struct PushSpec {
    pub base_url_v1: String,
    pub api_key: String,
    pub model: String,
    pub context_length: Option<u32>,
}

// ---------------------------------------------------------------------------------------
// Pure YAML logic
// ---------------------------------------------------------------------------------------

fn parse(text: &str) -> Result<Mapping, String> {
    let text = text.trim_start_matches('\u{feff}');
    if text.trim().is_empty() {
        return Ok(Mapping::new());
    }
    match serde_yaml::from_str::<Value>(text) {
        Ok(Value::Mapping(m)) => Ok(m),
        Ok(Value::Null) => Ok(Mapping::new()),
        Ok(_) => Err("config.yaml is not a YAML mapping; not touching it.".into()),
        Err(e) => Err(format!("config.yaml has a YAML error, so it was left unchanged: {e}")),
    }
}

fn key(s: &str) -> Value {
    Value::String(s.to_string())
}

/// Apply the provider block to an existing config.yaml text. Returns the new text.
pub fn apply(existing: &str, spec: &PushSpec) -> Result<String, String> {
    let mut root = parse(existing)?;

    // providers.neurallama
    let mut provider = Mapping::new();
    provider.insert(key("name"), key(PROVIDER_NAME));
    provider.insert(key("base_url"), key(&spec.base_url_v1));
    provider.insert(key("api_key"), key(&spec.api_key));
    provider.insert(key("transport"), key("chat_completions"));
    provider.insert(key("default_model"), key(&spec.model));
    let providers = root.entry(key("providers")).or_insert_with(|| Value::Mapping(Mapping::new()));
    if !providers.is_mapping() {
        *providers = Value::Mapping(Mapping::new());
    }
    providers.as_mapping_mut().unwrap().insert(key(PROVIDER_KEY), Value::Mapping(provider));

    // model:
    let model = root.entry(key("model")).or_insert_with(|| Value::Mapping(Mapping::new()));
    if !model.is_mapping() {
        // Old style `model: some-id` → becomes a mapping.
        *model = Value::Mapping(Mapping::new());
    }
    let m = model.as_mapping_mut().unwrap();
    for k in STALE_MODEL_KEYS {
        m.remove(key(k));
    }
    m.insert(key("provider"), key(PROVIDER_KEY));
    m.insert(key("default"), key(&spec.model));
    match spec.context_length {
        Some(n) => {
            m.insert(key("context_length"), Value::Number(n.into()));
        }
        None => {
            m.remove(key("context_length"));
        }
    }

    let out = serde_yaml::to_string(&Value::Mapping(root)).map_err(|e| format!("Could not write YAML: {e}"))?;
    Ok(out.replace("\r\n", "\n"))
}

/// (current provider, current model, points at this server?)
pub fn inspect(text: &str, base_url_v1: &str) -> (Option<String>, Option<String>, bool) {
    let Ok(root) = parse(text) else { return (None, None, false) };
    let model = root.get(key("model"));
    let provider = model.and_then(|m| m.get("provider")).and_then(Value::as_str).map(str::to_string);
    let default = match model {
        Some(Value::String(s)) => Some(s.clone()),
        Some(m) => m.get("default").and_then(Value::as_str).map(str::to_string),
        None => None,
    };
    let ours = root
        .get(key("providers"))
        .and_then(|p| p.get(PROVIDER_KEY))
        .and_then(|p| p.get("base_url").or_else(|| p.get("api")))
        .and_then(Value::as_str)
        .map(|u| u.trim_end_matches('/') == base_url_v1.trim_end_matches('/'))
        .unwrap_or(false);
    let points_here = ours && provider.as_deref() == Some(PROVIDER_KEY);
    (provider, default, points_here)
}

pub fn valid_profile_name(name: &str) -> bool {
    let b = name.as_bytes();
    !b.is_empty()
        && b.len() <= 64
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_' || *c == b'-')
}

// ---------------------------------------------------------------------------------------
// Discovery
// ---------------------------------------------------------------------------------------

/// Where a target's files live.
#[derive(Debug, Clone)]
#[cfg_attr(not(windows), allow(dead_code))]
pub enum Location {
    Native { home: PathBuf },
    Wsl { distro: String, home: String },
}

#[derive(Debug, Clone)]
pub struct Found {
    pub target: HermesTarget,
    pub location: Location,
}

#[cfg_attr(not(windows), allow(dead_code))]
pub struct DiscoverCtx {
    pub base_url_v1: String,
    pub port: u16,
    pub lan_mode: bool,
}

/// Native Hermes home: HERMES_HOME, else %LOCALAPPDATA%\hermes on Windows, ~/.hermes elsewhere.
pub fn native_root() -> Option<PathBuf> {
    if let Some(h) = std::env::var_os("HERMES_HOME") {
        return Some(PathBuf::from(h));
    }
    if cfg!(windows) {
        dirs::data_local_dir().map(|d| d.join("hermes"))
    } else {
        dirs::home_dir().map(|d| d.join(".hermes"))
    }
}

fn has_identity(dir: &Path) -> bool {
    IDENTITY_FILES.iter().any(|f| dir.join(f).exists())
}

/// (profile name, home dir) for the default profile and each named profile under `root`.
pub fn native_profiles(root: &Path) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    if !root.is_dir() {
        return out;
    }
    if has_identity(root) {
        out.push(("default".to_string(), root.to_path_buf()));
    }
    let profiles = root.join("profiles");
    if let Ok(rd) = std::fs::read_dir(&profiles) {
        let mut named: Vec<(String, PathBuf)> = rd
            .flatten()
            .filter(|e| e.path().is_dir())
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                let ok = valid_profile_name(&name)
                    && name != "default"
                    && !profiles.join(".deleted").join(&name).exists()
                    && has_identity(&e.path());
                ok.then(|| (name, e.path()))
            })
            .collect();
        named.sort();
        out.extend(named);
    }
    out
}

pub fn discover(ctx: &DiscoverCtx) -> Vec<Found> {
    let mut found = Vec::new();
    if let Some(root) = native_root() {
        for (profile, home) in native_profiles(&root) {
            let config = home.join("config.yaml");
            let text = std::fs::read_to_string(&config).unwrap_or_default();
            let (current_provider, current_model, points_here) = inspect(&text, &ctx.base_url_v1);
            found.push(Found {
                target: HermesTarget {
                    id: format!("native:{profile}"),
                    kind: "native".into(),
                    distro: None,
                    is_default: profile == "default",
                    profile,
                    home: home.display().to_string(),
                    config_path: config.display().to_string(),
                    config_exists: config.exists(),
                    current_provider,
                    current_model,
                    points_here,
                    base_url: ctx.base_url_v1.clone(),
                    note: None,
                },
                location: Location::Native { home },
            });
        }
    }
    found.extend(wsl::discover(ctx));
    found
}

// ---------------------------------------------------------------------------------------
// Push
// ---------------------------------------------------------------------------------------

pub fn push(found: &Found, spec: &PushSpec) -> HermesPushResult {
    let target_id = found.target.id.clone();
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let result = match &found.location {
        Location::Native { home } => push_native(home, spec, &stamp),
        Location::Wsl { distro, home } => wsl::push(distro, home, spec, &stamp),
    };
    match result {
        Ok(backup_path) => {
            let mut message = format!(
                "Hermes profile \"{}\" now uses Neurallama ({}). Start a new Hermes session to pick it up.",
                found.target.profile, spec.model
            );
            if let Some(n) = spec.context_length {
                if n < 65536 {
                    message.push_str(&format!(
                        " Note: context is {n} tokens; Hermes works best with 64K or more."
                    ));
                }
            }
            HermesPushResult { target_id, ok: true, backup_path, message }
        }
        Err(e) => HermesPushResult { target_id, ok: false, backup_path: None, message: e },
    }
}

fn push_native(home: &Path, spec: &PushSpec, stamp: &str) -> Result<Option<String>, String> {
    let config = home.join("config.yaml");
    let existing = match std::fs::read_to_string(&config) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("Could not read {}: {e}", config.display())),
    };
    let new_text = apply(&existing, spec)?;
    let backup = if config.exists() {
        let dest = home.join(format!("config.yaml.bak.{stamp}"));
        std::fs::copy(&config, &dest).map_err(|e| format!("Could not back up config.yaml: {e}"))?;
        Some(dest.display().to_string())
    } else {
        None
    };
    crate::settings::write_atomic(&config, new_text.as_bytes())
        .map_err(|e| format!("Could not write {}: {e}", config.display()))?;
    Ok(backup)
}

// ---------------------------------------------------------------------------------------
// WSL
// ---------------------------------------------------------------------------------------

#[cfg(windows)]
mod wsl {
    use super::*;
    use std::io::Write;
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::time::Duration;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    /// Lists Hermes homes inside a distro. Output lines: `PROFILE\t<name>\t<dir>` and `GW\t<ip>`.
    const LIST_SCRIPT: &str = r#"H="${HERMES_HOME:-$HOME/.hermes}"
has() { for f in config.yaml .env SOUL.md profile.yaml auth.json state.db; do [ -e "$1/$f" ] && return 0; done; return 1; }
if [ -d "$H" ]; then
  has "$H" && printf 'PROFILE\tdefault\t%s\n' "$H"
  for p in "$H"/profiles/*/; do
    [ -d "$p" ] || continue
    n=$(basename "$p")
    [ -e "$H/profiles/.deleted/$n" ] && continue
    has "$p" && printf 'PROFILE\t%s\t%s\n' "$n" "${p%/}"
  done
fi
printf 'GW\t%s\n' "$(ip route show default 2>/dev/null | awk '{print $3; exit}')"
"#;

    const WRITE_SCRIPT: &str = r#"f="$1/config.yaml"
mkdir -p "$1" || exit 1
if [ -f "$f" ]; then cp -p "$f" "$f.bak.$2" || exit 1; echo "BACKUP $f.bak.$2"; fi
cat > "$f.tmp" && mv "$f.tmp" "$f"
"#;

    fn wsl() -> Command {
        let mut c = Command::new("wsl.exe");
        c.env("WSL_UTF8", "1").creation_flags(CREATE_NO_WINDOW);
        c
    }

    /// wsl.exe prints UTF-16LE unless WSL_UTF8=1 is honoured; handle both.
    fn decode(bytes: &[u8]) -> String {
        if bytes.len() >= 2 && bytes.iter().skip(1).step_by(2).take(8).all(|b| *b == 0) {
            let units: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            String::from_utf16_lossy(&units)
        } else {
            String::from_utf8_lossy(bytes).into_owned()
        }
    }

    fn run(mut cmd: Command, stdin: Option<&[u8]>, timeout: Duration) -> Result<String, String> {
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        cmd.stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() });
        let mut child = cmd.spawn().map_err(|e| format!("Could not run wsl.exe: {e}"))?;
        if let (Some(data), Some(mut si)) = (stdin, child.stdin.take()) {
            si.write_all(data).map_err(|e| format!("wsl.exe stdin: {e}"))?;
        }
        let start = std::time::Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if start.elapsed() > timeout => {
                    let _ = child.kill();
                    return Err("wsl.exe timed out".into());
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                Err(e) => return Err(e.to_string()),
            }
        }
        let out = child.wait_with_output().map_err(|e| e.to_string())?;
        if out.status.success() {
            Ok(decode(&out.stdout))
        } else {
            Err(format!("wsl.exe failed: {}", decode(&out.stderr).trim()))
        }
    }

    fn distros() -> Vec<String> {
        let mut cmd = wsl();
        cmd.args(["-l", "-q"]);
        run(cmd, None, Duration::from_secs(5))
            .map(|s| {
                s.lines()
                    .map(|l| l.trim().trim_matches('\0').to_string())
                    .filter(|l| !l.is_empty() && !l.to_lowercase().starts_with("docker-desktop"))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn mirrored_networking() -> bool {
        let Some(home) = dirs::home_dir() else { return false };
        std::fs::read_to_string(home.join(".wslconfig"))
            .map(|t| {
                t.lines().any(|l| {
                    let l = l.trim().to_lowercase().replace(' ', "");
                    l.starts_with("networkingmode=mirrored")
                })
            })
            .unwrap_or(false)
    }

    pub fn discover(ctx: &DiscoverCtx) -> Vec<Found> {
        let mut found = Vec::new();
        let mirrored = mirrored_networking();
        for distro in distros() {
            let mut cmd = wsl();
            cmd.args(["-d", &distro, "--exec", "sh", "-c", LIST_SCRIPT]);
            let Ok(out) = run(cmd, None, Duration::from_secs(8)) else { continue };
            let gw = out
                .lines()
                .find_map(|l| l.strip_prefix("GW\t"))
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            let (base_url, note) = if mirrored {
                (ctx.base_url_v1.clone(), None)
            } else if let Some(gw) = &gw {
                let url = format!("http://{gw}:{}/v1", ctx.port);
                let mut note = format!(
                    "WSL uses NAT networking, so Hermes reaches Windows at {gw} (this address changes when WSL restarts — push again then)."
                );
                if !ctx.lan_mode {
                    note.push_str(&format!(
                        " Turn on LAN mode so llama-server listens beyond 127.0.0.1, and allow the port in Windows Firewall from an admin PowerShell: New-NetFirewallRule -DisplayName \"Neurallama llama-server\" -Direction Inbound -Protocol TCP -LocalPort {} -Action Allow",
                        ctx.port
                    ));
                }
                (url, Some(note))
            } else {
                (ctx.base_url_v1.clone(), Some("Could not find the Windows host address from inside WSL.".into()))
            };
            for line in out.lines() {
                let mut parts = line.splitn(3, '\t');
                if parts.next() != Some("PROFILE") {
                    continue;
                }
                let (Some(profile), Some(home)) = (parts.next(), parts.next()) else { continue };
                if profile != "default" && !valid_profile_name(profile) {
                    continue;
                }
                let config_path = format!("{home}/config.yaml");
                let mut cat = wsl();
                cat.args(["-d", &distro, "--exec", "cat", &config_path]);
                let text = run(cat, None, Duration::from_secs(5));
                let config_exists = text.is_ok();
                let (current_provider, current_model, points_here) =
                    inspect(&text.unwrap_or_default(), &base_url);
                found.push(Found {
                    target: HermesTarget {
                        id: format!("wsl:{distro}:{profile}"),
                        kind: "wsl".into(),
                        distro: Some(distro.clone()),
                        profile: profile.to_string(),
                        is_default: profile == "default",
                        home: format!(r"\\wsl.localhost\{distro}{}", home.replace('/', r"\")),
                        config_path,
                        config_exists,
                        current_provider,
                        current_model,
                        points_here,
                        base_url: base_url.clone(),
                        note: note.clone(),
                    },
                    location: Location::Wsl { distro: distro.clone(), home: home.to_string() },
                });
            }
        }
        found
    }

    pub fn push(distro: &str, home: &str, spec: &PushSpec, stamp: &str) -> Result<Option<String>, String> {
        let config_path = format!("{home}/config.yaml");
        let mut cat = wsl();
        cat.args(["-d", distro, "--exec", "cat", &config_path]);
        let existing = run(cat, None, Duration::from_secs(5)).unwrap_or_default();
        let new_text = apply(&existing, spec)?;
        let mut cmd = wsl();
        cmd.args(["-d", distro, "--exec", "sh", "-c", WRITE_SCRIPT, "sh", home, stamp]);
        let out = run(cmd, Some(new_text.as_bytes()), Duration::from_secs(10))?;
        Ok(out.lines().find_map(|l| l.strip_prefix("BACKUP ")).map(|p| format!(r"\\wsl.localhost\{distro}{}", p.replace('/', r"\"))))
    }
}

#[cfg(not(windows))]
mod wsl {
    use super::*;
    pub fn discover(_ctx: &DiscoverCtx) -> Vec<Found> {
        Vec::new()
    }
    pub fn push(_d: &str, _h: &str, _s: &PushSpec, _t: &str) -> Result<Option<String>, String> {
        Err("WSL is only available on Windows.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> PushSpec {
        PushSpec {
            base_url_v1: "http://127.0.0.1:8080/v1".into(),
            api_key: "nl-key".into(),
            model: "local".into(),
            context_length: Some(65536),
        }
    }

    #[test]
    fn writes_provider_and_clears_stale_keys() {
        let existing = "\u{feff}_config_version: 7\nmodel:\n  provider: openrouter\n  default: anthropic/claude\n  base_url: https://x\n  api_mode: responses\n  context_length: 1000000\nproviders:\n  omlx:\n    name: oMLX\n    base_url: http://127.0.0.1:8000/v1\nterminal:\n  backend: local\n";
        let out = apply(existing, &spec()).unwrap();
        assert!(!out.contains('\u{feff}'));
        assert!(!out.contains("\r\n"));
        let v: Value = serde_yaml::from_str(&out).unwrap();
        assert_eq!(v["_config_version"], Value::Number(7.into()));
        assert_eq!(v["model"]["provider"], key("neurallama"));
        assert_eq!(v["model"]["default"], key("local"));
        assert_eq!(v["model"]["context_length"], Value::Number(65536.into()));
        assert!(v["model"].get("base_url").is_none());
        assert!(v["model"].get("api_mode").is_none());
        assert_eq!(v["providers"]["neurallama"]["base_url"], key("http://127.0.0.1:8080/v1"));
        assert_eq!(v["providers"]["neurallama"]["transport"], key("chat_completions"));
        // Other providers and sections are kept.
        assert_eq!(v["providers"]["omlx"]["name"], key("oMLX"));
        assert_eq!(v["terminal"]["backend"], key("local"));
        let (p, m, here) = inspect(&out, "http://127.0.0.1:8080/v1/");
        assert_eq!(p.as_deref(), Some("neurallama"));
        assert_eq!(m.as_deref(), Some("local"));
        assert!(here);
    }

    #[test]
    fn empty_and_string_model_configs() {
        let out = apply("", &PushSpec { context_length: None, ..spec() }).unwrap();
        let v: Value = serde_yaml::from_str(&out).unwrap();
        assert_eq!(v["model"]["provider"], key("neurallama"));
        assert!(v["model"].get("context_length").is_none());
        let out2 = apply("model: gpt-4o\n", &spec()).unwrap();
        let v2: Value = serde_yaml::from_str(&out2).unwrap();
        assert_eq!(v2["model"]["default"], key("local"));
    }

    #[test]
    fn refuses_broken_yaml() {
        assert!(apply("model: [unclosed\n", &spec()).is_err());
        assert!(apply("- just\n- a list\n", &spec()).is_err());
    }

    #[test]
    fn inspect_detects_other_provider() {
        let (p, _, here) = inspect("model:\n  provider: openrouter\n  default: x\n", "http://127.0.0.1:8080/v1");
        assert_eq!(p.as_deref(), Some("openrouter"));
        assert!(!here);
    }

    #[test]
    fn profile_discovery() {
        let root = std::env::temp_dir().join(format!("nl-hermes-{}", crate::settings::new_id()));
        std::fs::create_dir_all(root.join("profiles/work")).unwrap();
        std::fs::create_dir_all(root.join("profiles/empty")).unwrap();
        std::fs::create_dir_all(root.join("profiles/Bad Name")).unwrap();
        std::fs::create_dir_all(root.join("profiles/gone")).unwrap();
        std::fs::create_dir_all(root.join("profiles/.deleted/gone")).unwrap();
        std::fs::write(root.join("config.yaml"), "model: {}\n").unwrap();
        std::fs::write(root.join("profiles/work/SOUL.md"), "hi").unwrap();
        std::fs::write(root.join("profiles/Bad Name/SOUL.md"), "hi").unwrap();
        std::fs::write(root.join("profiles/gone/SOUL.md"), "hi").unwrap();
        let names: Vec<String> = native_profiles(&root).into_iter().map(|(n, _)| n).collect();
        assert_eq!(names, vec!["default", "work"]);
        // Push to a profile with no config.yaml yet: creates it, no backup.
        let found = Found {
            target: HermesTarget {
                id: "native:work".into(),
                kind: "native".into(),
                distro: None,
                profile: "work".into(),
                is_default: false,
                home: String::new(),
                config_path: String::new(),
                config_exists: false,
                current_provider: None,
                current_model: None,
                points_here: false,
                base_url: String::new(),
                note: None,
            },
            location: Location::Native { home: root.join("profiles/work") },
        };
        let r = push(&found, &spec());
        assert!(r.ok, "{}", r.message);
        assert!(r.backup_path.is_none());
        // Second push backs up.
        let r2 = push(&found, &spec());
        assert!(r2.backup_path.is_some());
        std::fs::remove_dir_all(root).ok();
    }
}
