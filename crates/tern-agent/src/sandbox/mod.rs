//! Sandbox v1 for the commands the research agent runs on its workbench
//! (#75): file conversions, a script over a data file, a citation tool.
//!
//! | Guard | What it does |
//! | --- | --- |
//! | Programs | Only programs on the allow-list run (pandoc, typst, python, git, ffmpeg...). No shells: one program per call, no pipes or redirection |
//! | Folder | The command runs in the chat's folder; a path argument outside it asks first |
//! | Deny paths | `~/.ssh`, `~/.aws`, browser profiles, Tern's own settings and keys, `.git/hooks`, shell start-up files: refused outright |
//! | Environment | Scrubbed: only the variables a program needs to start (PATH, TEMP...), never keys or tokens |
//! | Network | Off by default: every proxy variable points at a closed local port, and package tools are told to stay offline |
//! | Limits | Wall-clock time, memory, CPU time and the number of processes; the whole process tree is killed at the limit, on Stop, or from the Terminal tab |
//!
//! Windows: a Job Object with kill-on-close holds the whole tree (the
//! process starts suspended, joins the job, then runs), with job memory,
//! process count and CPU time limits. Linux and macOS: the command leads
//! its own process group (killed as a group) under rlimits.
//!
//! What v1 does not do: it cannot stop a program from opening a path it
//! was not given on its command line, and the network block relies on
//! programs honouring proxy variables. An OS sandbox (Seatbelt, bwrap,
//! Windows process containers) is the planned v2. A restricted Windows
//! token is not used: the standard library starts processes with the
//! user's own token, and replacing that needs a launcher of its own (v2).

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::io::AsyncReadExt;

use crate::live::{self, RunState, Stream};

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

/// Programs the workbench may run unless the user changes the list:
/// converters, typesetters, data tools, interpreters for the user's own
/// scripts. Shells are left out on purpose (they would run anything).
pub const DEFAULT_PROGRAMS: &[&str] = &[
    "pandoc",
    "typst",
    "python",
    "python3",
    "py",
    "uv",
    "Rscript",
    "node",
    "git",
    "ffmpeg",
    "ffprobe",
    "magick",
    "tesseract",
    "qpdf",
    "pdftotext",
    "pdfinfo",
    "pdftoppm",
    "rg",
    "jq",
    "sqlite3",
    "latexmk",
    "pdflatex",
    "xelatex",
    "lualatex",
    "bibtex",
    "biber",
    "dot",
    "mmdc",
    "soffice",
    "libreoffice",
];

/// Where a closed proxy points: nothing listens on the discard port.
const BLACKHOLE: &str = "http://127.0.0.1:9";

/// The limits on one command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Wall-clock time before the tree is killed.
    pub wall: Duration,
    /// Memory for the whole tree (Windows) or each process (Linux), MB.
    pub memory_mb: u64,
    /// Processes in the tree at once (Windows).
    pub processes: u32,
    /// CPU time, seconds.
    pub cpu_secs: u64,
    /// Output kept for the model per pipe, bytes.
    pub output_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            wall: Duration::from_secs(120),
            memory_mb: 2048,
            processes: 16,
            cpu_secs: 240,
            output_bytes: 256 * 1024,
        }
    }
}

/// The policy one command runs under.
#[derive(Clone, Debug)]
pub struct SandboxPolicy {
    /// The chat's folder: the command runs in it.
    pub folder: PathBuf,
    /// Network for spawned commands (off unless the user allowed it).
    pub network: bool,
    /// Program names that may run (matched without `.exe`, any case).
    pub programs: Vec<String>,
    /// Paths no argument may point into.
    pub deny: Vec<PathBuf>,
    pub limits: Limits,
}

impl SandboxPolicy {
    /// The default policy for a folder: network off, the default
    /// programs, the default deny paths.
    pub fn for_folder(folder: &Path) -> Self {
        Self {
            folder: crate::projects::plain_path(folder),
            network: false,
            programs: DEFAULT_PROGRAMS.iter().map(|s| (*s).to_owned()).collect(),
            deny: default_deny(),
            limits: Limits::default(),
        }
    }

    /// One line for the Terminal tab and the log.
    pub fn summary(&self) -> String {
        format!(
            "{} · {} MB · {} s · {}",
            if self.network {
                "network on"
            } else {
                "network off"
            },
            self.limits.memory_mb,
            self.limits.wall.as_secs(),
            if cfg!(windows) {
                "job object"
            } else {
                "process group + rlimits"
            }
        )
    }

    /// The scrubbed environment a command starts with.
    pub fn env(&self) -> Vec<(String, String)> {
        const KEEP: &[&str] = &[
            "PATH",
            "PATHEXT",
            "SYSTEMROOT",
            "SYSTEMDRIVE",
            "WINDIR",
            "COMSPEC",
            "TEMP",
            "TMP",
            "TMPDIR",
            "HOME",
            "USERPROFILE",
            "HOMEDRIVE",
            "HOMEPATH",
            "APPDATA",
            "LOCALAPPDATA",
            "PROGRAMDATA",
            "PROGRAMFILES",
            "PROGRAMFILES(X86)",
            "COMMONPROGRAMFILES",
            "NUMBER_OF_PROCESSORS",
            "PROCESSOR_ARCHITECTURE",
            "OS",
            "LANG",
            "LC_ALL",
            "LC_CTYPE",
            "TZ",
            "USER",
            "USERNAME",
            "LOGNAME",
            "SHELL",
            "XDG_RUNTIME_DIR",
            "FONTCONFIG_PATH",
        ];
        let mut out: Vec<(String, String)> = std::env::vars()
            .filter(|(k, _)| KEEP.iter().any(|w| w.eq_ignore_ascii_case(k)))
            .filter(|(k, _)| !secret_name(k))
            .collect();
        // Names are case-insensitive on Windows only: there `https_proxy`
        // and `HTTPS_PROXY` are one variable.
        let mut set = |k: &str, v: &str| {
            out.retain(|(o, _)| {
                if cfg!(windows) {
                    !o.eq_ignore_ascii_case(k)
                } else {
                    o != k
                }
            });
            out.push((k.to_owned(), v.to_owned()));
        };
        set("TERN_SANDBOX", "1");
        set("NO_COLOR", "1");
        set("PYTHONIOENCODING", "utf-8");
        set("PYTHONUTF8", "1");
        set("PYTHONDONTWRITEBYTECODE", "1");
        // git never runs hooks from a repository inside the sandbox.
        set("GIT_CONFIG_COUNT", "1");
        set("GIT_CONFIG_KEY_0", "core.hooksPath");
        set(
            "GIT_CONFIG_VALUE_0",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        );
        set("GIT_TERMINAL_PROMPT", "0");
        if !self.network {
            // Lower case first, so on Windows the upper-case name is the
            // one kept.
            for k in [
                "http_proxy",
                "https_proxy",
                "all_proxy",
                "HTTP_PROXY",
                "HTTPS_PROXY",
                "ALL_PROXY",
                "FTP_PROXY",
            ] {
                set(k, BLACKHOLE);
            }
            set("no_proxy", "");
            set("NO_PROXY", "");
            set("PIP_NO_INDEX", "1");
            set("UV_OFFLINE", "1");
            set("npm_config_offline", "true");
            set("HF_HUB_OFFLINE", "1");
            set("GIT_ALLOW_PROTOCOL", "file");
        }
        out
    }
}

/// A variable name that looks like it holds a secret.
pub fn secret_name(k: &str) -> bool {
    let k = k.to_ascii_uppercase();
    // Tern's own git setting (`GIT_CONFIG_KEY_0=core.hooksPath`).
    if k.starts_with("GIT_CONFIG_KEY_") {
        return false;
    }
    [
        "KEY",
        "TOKEN",
        "SECRET",
        "PASSWORD",
        "PASSWD",
        "CREDENTIAL",
        "AUTH",
        "COOKIE",
        "SESSION",
    ]
    .iter()
    .any(|w| k.contains(w))
}

/// Paths no command argument may point into.
pub fn default_deny() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let home = home_dir();
    if let Some(h) = &home {
        for rel in [
            ".ssh",
            ".aws",
            ".azure",
            ".gnupg",
            ".kube",
            ".docker",
            ".config/gcloud",
            ".config/gh",
            ".netrc",
            ".git-credentials",
            ".bashrc",
            ".bash_profile",
            ".zshrc",
            ".zprofile",
            ".profile",
            ".config/fish",
            ".mozilla",
            ".config/google-chrome",
            ".config/chromium",
            ".config/BraveSoftware",
            ".config/microsoft-edge",
            "Library/Application Support/Google/Chrome",
            "Library/Application Support/Firefox",
            "Library/Application Support/BraveSoftware",
            "Library/Application Support/Microsoft Edge",
            "Library/Safari",
            "Library/Keychains",
            "Library/Cookies",
            "Documents/WindowsPowerShell",
            "Documents/PowerShell",
        ] {
            out.push(h.join(rel));
        }
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) {
        for rel in [
            "Google/Chrome/User Data",
            "Microsoft/Edge/User Data",
            "BraveSoftware/Brave-Browser/User Data",
            "Microsoft/Credentials",
        ] {
            out.push(local.join(rel));
        }
    }
    if let Some(roaming) = std::env::var_os("APPDATA").map(PathBuf::from) {
        for rel in [
            "Mozilla/Firefox",
            "Microsoft/Windows/Start Menu/Programs/Startup",
            "Microsoft/Credentials",
            "Microsoft/Protect",
        ] {
            out.push(roaming.join(rel));
        }
    }
    if let Ok(p) = tern_platform::AppPaths::discover() {
        out.push(p.config_dir);
        out.push(p.data_dir);
    }
    out
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// Why the sandbox refused a command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Denial {
    pub reason: String,
}

impl Denial {
    fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

/// A command the sandbox accepted, ready to run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prepared {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    /// The command as one line, for the Terminal tab.
    pub display: String,
    /// Path arguments that point outside the folder (they ask first).
    pub outside: Vec<PathBuf>,
}

/// Splits a command line into words: spaces separate, `"..."` and
/// `'...'` group. Pipes, redirection, `;`, `&&` and `$(...)` outside
/// quotes are refused: one program per call.
pub fn split_command(line: &str) -> Result<Vec<String>, Denial> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut started = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => cur.push(c),
            None => match c {
                '"' | '\'' => {
                    quote = Some(c);
                    started = true;
                }
                c if c.is_whitespace() => {
                    if started {
                        words.push(std::mem::take(&mut cur));
                        started = false;
                    }
                }
                '|' | ';' | '<' | '>' | '`' | '&' => {
                    return Err(Denial::new(format!(
                        "`{c}` is shell syntax: the workbench runs one program per call, without a shell (no pipes, redirection or chains). Run each step as its own command."
                    )));
                }
                '$' if chars.peek() == Some(&'(') => {
                    return Err(Denial::new(
                        "`$(...)` is shell syntax: the workbench runs one program per call, without a shell.",
                    ));
                }
                c => {
                    cur.push(c);
                    started = true;
                }
            },
        }
    }
    if quote.is_some() {
        return Err(Denial::new("A quote in the command is not closed."));
    }
    if started {
        words.push(cur);
    }
    Ok(words)
}

/// The program's bare name: no folder, no `.exe`, lower case.
pub fn program_stem(p: &str) -> String {
    let name = p.rsplit(['/', '\\']).next().unwrap_or(p);
    let lower = name.to_ascii_lowercase();
    for ext in [".exe", ".cmd", ".bat", ".com"] {
        if let Some(s) = lower.strip_suffix(ext) {
            return s.to_owned();
        }
    }
    lower
}

/// Checks a command against the policy and resolves its program.
/// `cwd` is a folder inside the chat's folder (relative), or `None`.
pub fn prepare(
    policy: &SandboxPolicy,
    command: &str,
    extra_args: &[String],
    cwd: Option<&str>,
) -> Result<Prepared, Denial> {
    let mut words = split_command(command)?;
    words.extend(extra_args.iter().cloned());
    let Some(first) = words.first().cloned() else {
        return Err(Denial::new("The command is empty."));
    };
    let stem = program_stem(&first);
    if !policy.programs.iter().any(|p| program_stem(p) == stem) {
        return Err(Denial::new(format!(
            "`{first}` is not on the workbench's program list ({}). Add it in Settings › Permissions if you trust it.",
            policy.programs.join(", ")
        )));
    }
    let folder = policy.folder.clone();
    let cwd = match cwd.map(str::trim).filter(|c| !c.is_empty() && *c != ".") {
        Some(rel) => {
            let p = lexical(&folder.join(rel));
            if !p.starts_with(&folder) {
                return Err(Denial::new(
                    "The command must run inside the chat's folder.",
                ));
            }
            p
        }
        None => folder.clone(),
    };
    let program = find_program(&first, &folder)
        .ok_or_else(|| Denial::new(format!("`{first}` is not installed (not found on PATH).")))?;
    if let Some(d) = denied(policy, &program) {
        return Err(Denial::new(format!(
            "`{}` is inside {}, a protected place.",
            program.display(),
            d.display()
        )));
    }
    let mut outside = Vec::new();
    for w in &words[1..] {
        for cand in path_candidates(w) {
            let p = resolve_arg(&cand, &cwd);
            if let Some(d) = denied(policy, &p) {
                return Err(Denial::new(format!(
                    "The argument `{w}` points into {}, a protected place (keys, browser profiles, Tern's settings, git hooks or shell start-up files).",
                    d.display()
                )));
            }
            if !p.starts_with(&folder) && !outside.contains(&p) {
                outside.push(p);
            }
        }
    }
    let display = words
        .iter()
        .map(|w| {
            if w.is_empty() || w.contains(char::is_whitespace) {
                format!("\"{w}\"")
            } else {
                w.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    Ok(Prepared {
        program,
        args: words[1..].to_vec(),
        cwd,
        display,
        outside,
    })
}

/// The deny entry `p` falls under, if any. `.git/hooks` anywhere counts.
pub fn denied(policy: &SandboxPolicy, p: &Path) -> Option<PathBuf> {
    let n = crate::permissions::norm(p);
    let fold = |s: &str| {
        if cfg!(windows) || cfg!(target_os = "macos") {
            s.to_ascii_lowercase()
        } else {
            s.to_owned()
        }
    };
    let nf = fold(&n);
    if nf.contains("/.git/hooks") || nf.ends_with(".git/hooks") {
        return Some(PathBuf::from(".git/hooks"));
    }
    policy.deny.iter().find_map(|d| {
        let df = fold(&crate::permissions::norm(d));
        let df = df.trim_end_matches('/');
        (nf == df || nf.starts_with(&format!("{df}/"))).then(|| d.clone())
    })
}

/// The parts of one argument that may be paths: the argument itself, and
/// what follows `=` in `--out=x`.
fn path_candidates(w: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut push = |s: &str| {
        if looks_like_path(s) {
            out.push(s.to_owned());
        }
    };
    match w.split_once('=') {
        // `--output=../x.md`: the value is the path, not the flag.
        Some((flag, v)) if flag.starts_with('-') => push(v),
        Some((_, v)) => {
            push(w);
            push(v);
        }
        None => push(w),
    }
    out
}

fn looks_like_path(s: &str) -> bool {
    if s.is_empty() || s.starts_with('-') && !s.contains(['/', '\\']) {
        return false;
    }
    if s.contains("://") {
        return false;
    }
    let b = s.as_bytes();
    s.contains(['/', '\\'])
        || s.starts_with('~')
        || s.starts_with('.')
        || (b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':')
}

fn resolve_arg(arg: &str, cwd: &Path) -> PathBuf {
    let p = if let Some(rest) = arg.strip_prefix("~/").or_else(|| arg.strip_prefix("~\\")) {
        home_dir()
            .map(|h| h.join(rest))
            .unwrap_or_else(|| PathBuf::from(arg))
    } else if arg == "~" {
        home_dir().unwrap_or_else(|| PathBuf::from(arg))
    } else {
        let p = PathBuf::from(arg);
        if p.is_absolute() { p } else { cwd.join(p) }
    };
    // Follow links that exist, so a link inside the folder that points at
    // a protected place is caught.
    p.canonicalize()
        .map(|c| crate::projects::plain_path(&c))
        .unwrap_or_else(|_| lexical(&p))
}

/// `a/b/../c` as `a/c`, without touching the disk.
pub fn lexical(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Finds a program on PATH (never in the chat's folder, so a script the
/// agent wrote cannot pose as `python`). A full path is taken as given.
pub fn find_program(name: &str, folder: &Path) -> Option<PathBuf> {
    let p = Path::new(name);
    if p.is_absolute() {
        return p.is_file().then(|| p.to_path_buf());
    }
    if name.contains(['/', '\\']) {
        return None;
    }
    let exts: Vec<String> = if cfg!(windows) {
        let pe = std::env::var("PATHEXT").unwrap_or_else(|_| ".EXE;.CMD;.BAT;.COM".into());
        std::iter::once(String::new())
            .chain(pe.split(';').map(|e| e.to_ascii_lowercase()))
            .collect()
    } else {
        vec![String::new()]
    };
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        if dir.as_os_str().is_empty() || dir == Path::new(".") || dir.starts_with(folder) {
            continue;
        }
        for e in &exts {
            let cand = dir.join(format!("{name}{e}"));
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    None
}

/// How a command ended.
#[derive(Clone, Debug)]
pub struct Outcome {
    /// The Terminal tab's run id.
    pub run: u64,
    pub state: RunState,
    pub exit: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub took: Duration,
}

/// Records a refusal in the Terminal tab and tern.log.
pub fn note_denied(command: &str, folder: &Path, reason: &str) -> u64 {
    let id = live::start_run(command, &folder.display().to_string(), "sandbox");
    live::push_output(id, Stream::Tern, &format!("Refused: {reason}\n"));
    live::finish_run(id, RunState::Denied, None);
    crate::diag::note(&format!("sandbox: denied `{command}`: {reason}"));
    id
}

/// Runs a prepared command under the policy, streaming its output to the
/// Terminal tab.
pub async fn run(policy: &SandboxPolicy, p: &Prepared) -> Outcome {
    let started = Instant::now();
    let id = live::start_run(&p.display, &p.cwd.display().to_string(), &policy.summary());
    crate::diag::note(&format!(
        "sandbox: run {id} `{}` in {} ({})",
        p.display,
        p.cwd.display(),
        policy.summary()
    ));
    let mut cmd = tokio::process::Command::new(&p.program);
    cmd.args(&p.args)
        .current_dir(&p.cwd)
        .env_clear()
        .envs(policy.env())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    unix::configure(&mut cmd, &policy.limits);
    #[cfg(windows)]
    windows::configure(&mut cmd);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            live::push_output(id, Stream::Tern, &format!("Could not start: {e}\n"));
            live::finish_run(id, RunState::Failed, None);
            crate::diag::note(&format!("sandbox: run {id} could not start: {e}"));
            return Outcome {
                run: id,
                state: RunState::Failed,
                exit: None,
                stdout: String::new(),
                stderr: format!("could not start: {e}"),
                took: started.elapsed(),
            };
        }
    };
    #[cfg(windows)]
    let tree = windows::contain(&child, &policy.limits);
    #[cfg(unix)]
    let tree = unix::Tree::of(&child);
    let stop = Arc::new(tokio::sync::Notify::new());
    {
        let stop = stop.clone();
        live::set_killer(id, Arc::new(move || stop.notify_one()));
    }
    let cap = policy.limits.output_bytes;
    let out = child
        .stdout
        .take()
        .map(|s| tokio::spawn(pump(id, s, Stream::Stdout, cap)));
    let err = child
        .stderr
        .take()
        .map(|s| tokio::spawn(pump(id, s, Stream::Stderr, cap)));
    let (state, exit) = tokio::select! {
        r = child.wait() => match r {
            Ok(s) => (RunState::Exited, s.code()),
            Err(_) => (RunState::Failed, None),
        },
        _ = tokio::time::sleep(policy.limits.wall) => (RunState::TimedOut, None),
        _ = stop.notified() => (RunState::Killed, None),
    };
    if state != RunState::Exited {
        tree.kill();
        let _ = child.start_kill();
        let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
        let line = match state {
            RunState::TimedOut => format!(
                "Stopped after {} s (the time limit); the whole process tree was killed.\n",
                policy.limits.wall.as_secs()
            ),
            _ => "Killed; the whole process tree was stopped.\n".to_owned(),
        };
        live::push_output(id, Stream::Tern, &line);
    }
    // Anything the tree left running goes with it.
    drop(tree);
    let read = |h: Option<tokio::task::JoinHandle<String>>| async move {
        match h {
            Some(h) => tokio::time::timeout(Duration::from_secs(5), h)
                .await
                .ok()
                .and_then(Result::ok)
                .unwrap_or_default(),
            None => String::new(),
        }
    };
    let stdout = read(out).await;
    let stderr = read(err).await;
    live::finish_run(id, state, exit);
    let took = started.elapsed();
    crate::diag::note(&format!(
        "sandbox: run {id} {} exit={} in {:.1}s",
        state.label(),
        exit.map(|c| c.to_string()).unwrap_or_else(|| "-".into()),
        took.as_secs_f32()
    ));
    Outcome {
        run: id,
        state,
        exit,
        stdout,
        stderr,
        took,
    }
}

async fn pump<R: tokio::io::AsyncRead + Unpin>(id: u64, mut r: R, s: Stream, cap: usize) -> String {
    let mut kept = String::new();
    let mut buf = vec![0u8; 8192];
    let mut carry: Vec<u8> = Vec::new();
    loop {
        let n = match r.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        carry.extend_from_slice(&buf[..n]);
        // Hold back a cut UTF-8 sequence until the rest arrives.
        let cut = match std::str::from_utf8(&carry) {
            Ok(_) => carry.len(),
            Err(e) if e.error_len().is_none() => e.valid_up_to(),
            Err(_) => carry.len(),
        };
        let text = String::from_utf8_lossy(&carry[..cut]).into_owned();
        carry.drain(..cut);
        live::push_output(id, s, &text);
        if kept.len() < cap {
            kept.push_str(&text);
        }
    }
    if !carry.is_empty() {
        let text = String::from_utf8_lossy(&carry).into_owned();
        live::push_output(id, s, &text);
        kept.push_str(&text);
    }
    if kept.len() > cap {
        let mut end = cap;
        while !kept.is_char_boundary(end) {
            end -= 1;
        }
        kept.truncate(end);
        kept.push_str("\n[output cut]");
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(folder: &Path) -> SandboxPolicy {
        let mut p = SandboxPolicy::for_folder(folder);
        p.deny = vec![folder.join("secret")];
        p
    }

    #[test]
    fn commands_split_without_a_shell() {
        assert_eq!(
            split_command(r#"pandoc "my notes.md" -o 'out file.docx'"#).unwrap(),
            vec!["pandoc", "my notes.md", "-o", "out file.docx"]
        );
        assert!(split_command("cat a | grep b").is_err());
        assert!(split_command("echo a > b").is_err());
        assert!(split_command("a && b").is_err());
        assert!(split_command("echo $(whoami)").is_err());
        assert!(split_command("echo \"open").is_err());
        assert_eq!(
            split_command("python -c \"print('a|b')\"").unwrap()[2],
            "print('a|b')"
        );
        assert_eq!(split_command("\"\"").unwrap(), vec![String::new()]);
    }

    #[test]
    fn programs_off_the_list_are_refused() {
        let d = tempfile::tempdir().unwrap();
        let p = policy(d.path());
        let e = prepare(&p, "bash -c ls", &[], None).unwrap_err();
        assert!(e.reason.contains("program list"), "{}", e.reason);
        assert_eq!(program_stem("C:\\Py\\Python.EXE"), "python");
        assert!(prepare(&p, "", &[], None).is_err());
    }

    #[test]
    fn deny_paths_and_git_hooks() {
        let d = tempfile::tempdir().unwrap();
        let p = policy(d.path());
        assert!(denied(&p, &d.path().join("secret/key.pem")).is_some());
        assert!(denied(&p, &d.path().join("repo/.git/hooks/pre-commit")).is_some());
        assert!(denied(&p, &d.path().join("notes.md")).is_none());
        assert!(denied(&p, &d.path().join("secretive.md")).is_none());
    }

    #[test]
    fn the_environment_is_scrubbed_and_offline() {
        let d = tempfile::tempdir().unwrap();
        let p = policy(d.path());
        let env = p.env();
        assert!(env.iter().all(|(k, _)| !secret_name(k)));
        assert!(
            env.iter()
                .any(|(k, v)| k == "HTTPS_PROXY" && v == BLACKHOLE)
        );
        assert!(
            env.iter()
                .any(|(k, v)| k == "GIT_CONFIG_KEY_0" && v == "core.hooksPath")
        );
        let mut on = p.clone();
        on.network = true;
        assert!(on.env().iter().all(|(k, _)| k != "HTTPS_PROXY"));
        assert!(secret_name("OPENAI_API_KEY") && secret_name("GITHUB_TOKEN"));
        assert!(!secret_name("PATH"));
    }

    #[test]
    fn path_arguments() {
        assert!(looks_like_path("out/a.docx"));
        assert!(looks_like_path("C:\\x"));
        assert!(looks_like_path("~/x"));
        assert!(!looks_like_path("--standalone"));
        assert!(!looks_like_path("https://a.b/c"));
        assert_eq!(
            path_candidates("--output=../x.md"),
            vec!["../x.md".to_owned()]
        );
        assert_eq!(lexical(Path::new("/a/b/../c/./d")), PathBuf::from("/a/c/d"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_command_runs_streams_and_is_killed_at_its_limit() {
        let d = tempfile::tempdir().unwrap();
        let mut p = policy(d.path());
        p.programs = vec!["sh".into(), "echo".into(), "sleep".into()];
        let prep = prepare(&p, "echo hello sandbox", &[], None).unwrap();
        let o = run(&p, &prep).await;
        assert_eq!(o.state, RunState::Exited);
        assert_eq!(o.exit, Some(0));
        assert_eq!(o.stdout.trim(), "hello sandbox");
        p.limits.wall = Duration::from_millis(300);
        let prep = prepare(&p, "sleep 30", &[], None).unwrap();
        let o = run(&p, &prep).await;
        assert_eq!(o.state, RunState::TimedOut);
        assert!(o.took < Duration::from_secs(10));
        let prep = prepare(&p, "sh -c \"echo $HTTPS_PROXY\"", &[], None).unwrap();
        let o = run(&p, &prep).await;
        assert_eq!(o.stdout.trim(), BLACKHOLE);
    }
}
