//! Builds the llama-server argument list from a Profile + Settings. All flags live here.

use super::CommandPreview;
use crate::settings::{Profile, Settings};
use std::path::Path;

pub fn build(profile: &Profile, settings: &Settings, server_exe: &Path) -> CommandPreview {
    let mut a: Vec<String> = Vec::new();
    let mut push = |flag: &str, value: String| {
        a.push(flag.to_string());
        a.push(value);
    };

    push("-m", profile.model_path.clone());
    if let Some(mm) = profile.mmproj_path.as_deref().filter(|s| !s.trim().is_empty()) {
        push("--mmproj", mm.to_string());
    }
    push("-ngl", profile.ngl.to_string());
    if profile.cpu_moe_all {
        a.push("--cpu-moe".into());
    } else if let Some(n) = profile.n_cpu_moe {
        a.push("--n-cpu-moe".into());
        a.push(n.to_string());
    }
    a.push("--ctx-size".into());
    a.push(profile.ctx_size.to_string());
    if !profile.cache_type_k.trim().is_empty() {
        a.push("-ctk".into());
        a.push(profile.cache_type_k.trim().to_string());
    }
    if !profile.cache_type_v.trim().is_empty() {
        a.push("-ctv".into());
        a.push(profile.cache_type_v.trim().to_string());
    }
    if profile.flash_attn {
        a.push("-fa".into());
        a.push("on".into());
    }
    for (flag, v) in [
        ("-t", profile.threads),
        ("-tb", profile.threads_batch),
        ("-b", profile.batch),
        ("-ub", profile.ubatch),
    ] {
        if let Some(v) = v {
            a.push(flag.into());
            a.push(v.to_string());
        }
    }
    a.push("--parallel".into());
    a.push(profile.parallel.max(1).to_string());
    if profile.jinja {
        a.push("--jinja".into());
    }
    for (flag, v) in [("--temp", profile.temp), ("--top-p", profile.top_p)] {
        if let Some(v) = v {
            a.push(flag.into());
            a.push(fmt_f32(v));
        }
    }
    if let Some(k) = profile.top_k {
        a.push("--top-k".into());
        a.push(k.to_string());
    }
    if let Some(v) = profile.min_p {
        a.push("--min-p".into());
        a.push(fmt_f32(v));
    }
    if !settings.alias.trim().is_empty() {
        a.push("--alias".into());
        a.push(settings.alias.trim().to_string());
    }
    a.push("--host".into());
    a.push(settings.bind_host().to_string());
    a.push("--port".into());
    a.push(settings.port.to_string());
    if !settings.api_key.trim().is_empty() {
        a.push("--api-key".into());
        a.push(settings.api_key.trim().to_string());
    }
    a.push("--metrics".into());
    a.extend(split_shell(&profile.extra_args));

    let program = server_exe.display().to_string();
    let display = display_line(&program, &a);
    CommandPreview { program, args: a, display }
}

fn fmt_f32(v: f32) -> String {
    // f32 Display is the shortest round-trip form ("0.7", not "0.699999988").
    format!("{v}")
}

/// Copyable Windows command line (CommandLineToArgvW quoting rules).
pub fn display_line(program: &str, args: &[String]) -> String {
    std::iter::once(program)
        .chain(args.iter().map(String::as_str))
        .map(quote_windows)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Same as [`display_line`] but with the `--api-key` value masked, for logs and status.
pub fn redacted_display(program: &str, args: &[String]) -> String {
    let mut masked = args.to_vec();
    for i in 0..masked.len() {
        if masked[i] == "--api-key" && i + 1 < masked.len() {
            masked[i + 1] = "***".into();
        } else if masked[i].starts_with("--api-key=") {
            masked[i] = "--api-key=***".into();
        }
    }
    display_line(program, &masked)
}

/// Quote one argument the way the MSVC CRT / CommandLineToArgvW parses it back.
pub fn quote_windows(arg: &str) -> String {
    if !arg.is_empty() && !arg.chars().any(|c| matches!(c, ' ' | '\t' | '\n' | '\x0b' | '"')) {
        return arg.to_string();
    }
    let mut out = String::with_capacity(arg.len() + 2);
    out.push('"');
    let mut backslashes = 0usize;
    for c in arg.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                out.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                out.push('"');
                backslashes = 0;
            }
            _ => {
                out.extend(std::iter::repeat_n('\\', backslashes));
                out.push(c);
                backslashes = 0;
            }
        }
    }
    out.extend(std::iter::repeat_n('\\', backslashes * 2));
    out.push('"');
    out
}

/// Split free text shell-style: whitespace separates, '…' and "…" group. Backslashes are
/// literal (Windows paths) except `\"` inside double quotes.
pub fn split_shell(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_token = false;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                in_token = true;
                while let Some(d) = chars.next() {
                    match d {
                        '"' => break,
                        '\\' if chars.peek() == Some(&'"') => {
                            cur.push('"');
                            chars.next();
                        }
                        _ => cur.push(d),
                    }
                }
            }
            '\'' => {
                in_token = true;
                for d in chars.by_ref() {
                    if d == '\'' {
                        break;
                    }
                    cur.push(d);
                }
            }
            c if c.is_whitespace() => {
                if in_token {
                    out.push(std::mem::take(&mut cur));
                    in_token = false;
                }
            }
            _ => {
                in_token = true;
                cur.push(c);
            }
        }
    }
    if in_token {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn settings() -> Settings {
        Settings { api_key: "nl-secret".into(), ..Settings::default() }
    }

    fn profile() -> Profile {
        Profile { model_path: r"C:\AI\models\unsloth\Q\model.gguf".into(), ..Profile::default() }
    }

    fn value_of<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
        args.iter().position(|a| a == flag).map(|i| args[i + 1].as_str())
    }

    #[test]
    fn redacts_equals_form_extra_key() {
        let args = vec!["--api-key=secret-override".into(), "--api-key".into(), "another-secret".into()];
        let shown = redacted_display("llama-server", &args);
        assert!(!shown.contains("secret"));
        assert!(shown.contains("--api-key=***"));
    }

    #[test]
    fn default_profile_args() {
        let p = build(&profile(), &settings(), &PathBuf::from(r"C:\AI\llama.cpp\b1\llama-server.exe"));
        let a = &p.args;
        assert_eq!(value_of(a, "-m"), Some(r"C:\AI\models\unsloth\Q\model.gguf"));
        assert_eq!(value_of(a, "-ngl"), Some("99"));
        assert_eq!(value_of(a, "--ctx-size"), Some("32768"));
        assert_eq!(value_of(a, "-ctk"), Some("f16"));
        assert_eq!(value_of(a, "-ctv"), Some("f16"));
        assert_eq!(value_of(a, "-fa"), Some("on"));
        assert_eq!(value_of(a, "--parallel"), Some("1"));
        assert_eq!(value_of(a, "--alias"), Some("local"));
        assert_eq!(value_of(a, "--host"), Some("127.0.0.1"));
        assert_eq!(value_of(a, "--port"), Some("8080"));
        assert_eq!(value_of(a, "--api-key"), Some("nl-secret"));
        assert!(a.contains(&"--jinja".to_string()));
        assert_eq!(a.last().unwrap(), "--metrics");
        for f in ["--mmproj", "--cpu-moe", "--n-cpu-moe", "-t", "-tb", "-b", "-ub", "--temp", "--top-k"] {
            assert!(!a.contains(&f.to_string()), "{f} should be omitted");
        }
    }

    #[test]
    fn optional_flags() {
        let p = Profile {
            mmproj_path: Some(r"C:\m\mmproj-F16.gguf".into()),
            n_cpu_moe: Some(8),
            flash_attn: false,
            threads: Some(16),
            threads_batch: Some(24),
            batch: Some(2048),
            ubatch: Some(512),
            parallel: 0,
            jinja: false,
            temp: Some(0.7),
            top_p: Some(0.8),
            top_k: Some(20),
            min_p: Some(0.0),
            ..profile()
        };
        let a = build(&p, &settings(), Path::new("llama-server")).args;
        assert_eq!(value_of(&a, "--mmproj"), Some(r"C:\m\mmproj-F16.gguf"));
        assert_eq!(value_of(&a, "--n-cpu-moe"), Some("8"));
        assert!(!a.contains(&"-fa".to_string()));
        assert!(!a.contains(&"--jinja".to_string()));
        assert_eq!(value_of(&a, "-t"), Some("16"));
        assert_eq!(value_of(&a, "-tb"), Some("24"));
        assert_eq!(value_of(&a, "-b"), Some("2048"));
        assert_eq!(value_of(&a, "-ub"), Some("512"));
        assert_eq!(value_of(&a, "--parallel"), Some("1"));
        assert_eq!(value_of(&a, "--temp"), Some("0.7"));
        assert_eq!(value_of(&a, "--top-p"), Some("0.8"));
        assert_eq!(value_of(&a, "--top-k"), Some("20"));
        assert_eq!(value_of(&a, "--min-p"), Some("0"));
    }

    #[test]
    fn cpu_moe_all_wins() {
        let p = Profile { cpu_moe_all: true, n_cpu_moe: Some(4), ..profile() };
        let a = build(&p, &settings(), Path::new("x")).args;
        assert!(a.contains(&"--cpu-moe".to_string()));
        assert!(!a.contains(&"--n-cpu-moe".to_string()));
    }

    #[test]
    fn lan_and_no_key() {
        let s = Settings { lan_mode: true, api_key: "  ".into(), port: 9000, ..Settings::default() };
        let a = build(&profile(), &s, Path::new("x")).args;
        assert_eq!(value_of(&a, "--host"), Some("0.0.0.0"));
        assert_eq!(value_of(&a, "--port"), Some("9000"));
        assert!(!a.contains(&"--api-key".to_string()));
    }

    #[test]
    fn extra_args_last_and_split() {
        let p = Profile {
            extra_args: r#"--lora "C:\My Loras\a b.gguf" --override-kv 'x=str:a b'  -v --rope-scale 2"#.into(),
            ..profile()
        };
        let a = build(&p, &settings(), Path::new("x")).args;
        let tail: Vec<&str> = a[a.len() - 7..].iter().map(String::as_str).collect();
        assert_eq!(
            tail,
            ["--lora", r"C:\My Loras\a b.gguf", "--override-kv", "x=str:a b", "-v", "--rope-scale", "2"]
        );
        assert_eq!(a[a.len() - 8], "--metrics");
    }

    #[test]
    fn split_shell_edge_cases() {
        assert!(split_shell("").is_empty());
        assert!(split_shell("   \t ").is_empty());
        assert_eq!(split_shell(r"C:\a\b.gguf"), [r"C:\a\b.gguf"]);
        assert_eq!(split_shell(r#""""#), [""]);
        assert_eq!(split_shell(r#"--x="a b" y"#), ["--x=a b", "y"]);
        assert_eq!(split_shell(r#""say \"hi\"""#), [r#"say "hi""#]);
        assert_eq!(split_shell(r#""unterminated rest"#), ["unterminated rest"]);
        assert_eq!(split_shell("a\nb"), ["a", "b"]);
    }

    #[test]
    fn windows_quoting() {
        assert_eq!(quote_windows("plain"), "plain");
        assert_eq!(quote_windows(""), r#""""#);
        assert_eq!(quote_windows(r"C:\My Models\m.gguf"), r#""C:\My Models\m.gguf""#);
        assert_eq!(quote_windows(r"C:\dir with space\"), r#""C:\dir with space\\""#);
        assert_eq!(quote_windows(r#"a"b"#), r#""a\"b""#);
        assert_eq!(quote_windows(r#"a\"b"#), r#""a\\\"b""#);
        assert_eq!(quote_windows(r"C:\no\space"), r"C:\no\space");
        assert_eq!(quote_windows("C:\\Модели\\qwen.gguf"), "C:\\Модели\\qwen.gguf");
        assert_eq!(quote_windows("C:\\モデル 1\\q.gguf"), "\"C:\\モデル 1\\q.gguf\"");
    }

    #[test]
    fn display_and_redaction() {
        let p = Profile { model_path: r"C:\Users\Zoë B\AI\models\m.gguf".into(), ..profile() };
        let prev = build(&p, &settings(), Path::new(r"C:\Program Files\llama\llama-server.exe"));
        assert!(prev.display.starts_with(r#""C:\Program Files\llama\llama-server.exe" -m "C:\Users\Zoë B\AI\models\m.gguf" -ngl 99"#));
        assert!(prev.display.contains("--api-key nl-secret"));
        let red = redacted_display(&prev.program, &prev.args);
        assert!(red.contains("--api-key ***"));
        assert!(!red.contains("nl-secret"));
    }
}
