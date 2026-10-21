//! The hooks' own sandbox. Claude Code runs hooks outside its sandbox, so
//! with the project's sandbox on (`OCGEN_SANDBOX=1`) the project code a hook
//! starts itself — the check, the formatter — runs in an OS sandbox of ocgen's
//! that denies everything a [`Policy`] doesn't grant. When no sandbox here can
//! enforce it, the command doesn't run at all — never unsandboxed.
//!
//! Each OS has a chain of sandboxes, best first ([`chain`]); the hook picks
//! the first that works here and can run the command ([`select`]) and starts
//! `ocgen __sandbox` with the policy, which applies exactly that one
//! ([`helper_main`]) — a sandbox that restricts its own process (Seatbelt,
//! Landlock) must not restrict the hook.
//!
//! Native Windows has none ([`NATIVE_WINDOWS`]): there only a WASI module
//! runs, and with the project's sandbox on, everything else is refused.

pub mod bwrap;
pub mod cmdline;
#[cfg(target_os = "linux")]
pub mod landlock;
pub mod policy;
pub mod seatbelt;
pub mod wasm;

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};

use serde::{Deserialize, Serialize};

pub use policy::Policy;

/// A sandbox ocgen can run a command in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// macOS: a Seatbelt profile of ocgen's.
    Seatbelt,
    /// Linux: bubblewrap's namespaces.
    Bwrap,
    /// Linux without namespaces: Landlock, and seccomp for the network.
    Landlock,
    /// Every OS, for WASI modules only: wasmtime.
    Wasm,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Seatbelt => "Seatbelt",
            Kind::Bwrap => "bubblewrap",
            Kind::Landlock => "Landlock + seccomp",
            Kind::Wasm => "wasmtime (WASI modules only)",
        }
    }
}

/// What the hook scripts say when they would have to run a command in the
/// sandbox: only the binary can.
pub const NEEDS_BINARY: &str = "the project's sandbox is on, and only the ocgen binary can run this command in it — install the ocgen that generated this project so the hooks use it; ocgen never runs it unsandboxed";

/// Why nothing runs sandboxed on native Windows, where Claude Code has no
/// sandbox either: with the project's sandbox on, the hooks' check and the
/// commands Claude runs are refused with this.
pub const NATIVE_WINDOWS: &str = "ocgen's sandbox doesn't run on native Windows: the sandbox Windows offers a program (an AppContainer) can't open the NUL device, which cargo, git and most tools open, so they fail in it. Run Claude Code and ocgen in WSL2, where the sandbox works, or turn the sandbox off for this project (`ocgen edit team`); ocgen never runs a command unsandboxed";

/// The env variable the policy travels to the helper in.
pub const POLICY_VAR: &str = "OCGEN_SANDBOX_POLICY";
/// The ocgen binary to run the helper with when this process isn't one (a
/// test calling the library). Never set by ocgen.
pub const HELPER_VAR: &str = "OCGEN_SANDBOX_HELPER";

/// The sandboxes this OS can use, best first.
pub fn chain() -> &'static [Kind] {
    if cfg!(target_os = "macos") {
        &[Kind::Seatbelt, Kind::Wasm]
    } else if cfg!(target_os = "linux") {
        &[Kind::Bwrap, Kind::Landlock, Kind::Wasm]
    } else {
        &[Kind::Wasm]
    }
}

/// Whether `kind` works on this machine at all, and if not, why.
pub fn probe(kind: Kind, env: &HashMap<String, String>) -> Result<(), String> {
    let _ = env;
    match kind {
        #[cfg(target_os = "macos")]
        Kind::Seatbelt => Ok(()),
        #[cfg(target_os = "linux")]
        Kind::Bwrap => {
            static BWRAP: std::sync::OnceLock<Result<PathBuf, String>> = std::sync::OnceLock::new();
            let path = env.get("PATH").map(String::as_str).unwrap_or("");
            BWRAP.get_or_init(|| bwrap::probe(path)).clone().map(|_| ())
        }
        #[cfg(target_os = "linux")]
        Kind::Landlock => landlock::abi().map(|_| ()),
        Kind::Wasm => Ok(()),
        #[allow(unreachable_patterns)]
        other => Err(format!("{} isn't available on this OS", other.name())),
    }
}

/// Whether `kind` can run `cmd` under `p`, and if not, why.
pub fn admits(kind: Kind, p: &Policy, cmd: &str) -> Result<(), String> {
    match kind {
        #[cfg(target_os = "linux")]
        Kind::Landlock => landlock::admits(p),
        Kind::Wasm => wasm::admits(p, cmd),
        _ => Ok(()),
    }
}

/// The first sandbox in `chain` that works here and can run `cmd` under
/// `p` — or why none can, each one's reason in turn (on native Windows,
/// [`NATIVE_WINDOWS`] first).
pub fn select(
    chain: &[Kind],
    probe: &dyn Fn(Kind) -> Result<(), String>,
    p: &Policy,
    cmd: &str,
) -> Result<Kind, String> {
    let mut why: Vec<String> = cfg!(windows)
        .then(|| NATIVE_WINDOWS.to_string())
        .into_iter()
        .collect();
    for &k in chain {
        match probe(k).and_then(|()| admits(k, p, cmd)) {
            Ok(()) => return Ok(k),
            Err(e) => why.push(format!("{}: {e}", k.name())),
        }
    }
    Err(format!(
        "the project's sandbox is on, but no sandbox here can run this command, and ocgen never runs it unsandboxed — {}",
        why.join("; ")
    ))
}

/// The command a hook runs `cmd` (sh syntax) with, in `dir` of `project`,
/// under the hooks' sandbox `env` describes — or why it can't run at all.
pub fn command(
    env: &HashMap<String, String>,
    project: &Path,
    dir: &Path,
    cmd: &str,
) -> Result<Command, String> {
    let git = git_dirs(dir);
    let mut p = policy::compile(&policy::Request {
        env,
        project,
        dir,
        git: &git,
    })
    .map_err(|e| format!("the project's sandbox is on, but {e}"))?;
    let kind = select(chain(), &|k| probe(k, env), &p, cmd)?;
    p.backend = Some(kind);
    let exe = helper_exe(env).ok_or(
        "the project's sandbox is on, but the ocgen binary that runs it can't be found, and ocgen never runs this command unsandboxed",
    )?;
    let mut c = Command::new(exe);
    c.args(["__sandbox", "-c", cmd]);
    c.env(POLICY_VAR, serde_json::to_string(&p).unwrap_or_default());
    for v in &p.env_remove {
        c.env_remove(v);
    }
    Ok(c)
}

/// The repository's git folders for `dir` (its own and the shared one, which
/// for a worktree or a monorepo lie outside the project).
fn git_dirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(out) = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "--absolute-git-dir", "--git-common-dir"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| {
            let p = PathBuf::from(l.trim());
            if p.is_relative() {
                dir.join(p)
            } else {
                p
            }
        })
        .collect()
}

static IS_BINARY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Note that this process is the ocgen binary (whatever its file is called),
/// so it can run the helper itself. `main` calls it first.
pub fn this_is_the_binary() {
    IS_BINARY.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// The ocgen binary to run the helper with: this one when it is ocgen, else
/// (a test calling the library) the one [`HELPER_VAR`] names.
pub fn helper_exe(env: &HashMap<String, String>) -> Option<PathBuf> {
    if IS_BINARY.load(std::sync::atomic::Ordering::Relaxed) {
        return std::env::current_exe().ok();
    }
    env.get(HELPER_VAR)
        .map(PathBuf::from)
        .filter(|p| p.is_file())
}

/// A program on `path` (a `PATH` value), as `command -v` finds it.
pub(crate) fn on_path(name: &str, path: &str) -> Option<PathBuf> {
    path.split(':').filter(|d| !d.is_empty()).find_map(|d| {
        let p = Path::new(d).join(name);
        #[cfg(unix)]
        let runnable = {
            use std::os::unix::fs::PermissionsExt;
            std::fs::metadata(&p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        };
        #[cfg(not(unix))]
        let runnable = p.is_file();
        runnable.then_some(p)
    })
}

/// How a sandboxed command ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    Code(i32),
    Signal(i32),
}

impl From<ExitStatus> for Exit {
    fn from(s: ExitStatus) -> Self {
        #[cfg(unix)]
        if let Some(sig) = std::os::unix::process::ExitStatusExt::signal(&s) {
            return Exit::Signal(sig);
        }
        Exit::Code(s.code().unwrap_or(1))
    }
}

/// The exit code for a sandbox that couldn't be applied (as `env` and
/// `timeout` use 125 for their own failures): the command never ran.
pub const NOT_APPLIED: i32 = 125;

/// `ocgen __sandbox -c <command>`: run the command under the policy in
/// [`POLICY_VAR`], in the sandbox the hook chose, and exit as it did.
pub fn helper_main(args: &[OsString]) -> ! {
    let code = match helper(args) {
        Ok(Exit::Code(c)) => c,
        Ok(Exit::Signal(sig)) => reraise(sig),
        Err(why) => {
            eprintln!("ocgen sandbox: {why}");
            NOT_APPLIED
        }
    };
    std::process::exit(code)
}

fn helper(args: &[OsString]) -> Result<Exit, String> {
    let cmd = match args {
        [flag, cmd] if flag == "-c" => cmd.to_string_lossy().into_owned(),
        _ => return Err("usage: ocgen __sandbox -c <command> (run by ocgen's hooks)".into()),
    };
    let raw = std::env::var(POLICY_VAR)
        .map_err(|_| format!("no {POLICY_VAR}: this is run by ocgen's hooks"))?;
    std::env::remove_var(POLICY_VAR);
    let mut p: Policy =
        serde_json::from_str(&raw).map_err(|e| format!("unreadable policy: {e}"))?;
    let kind = p.backend.ok_or("no sandbox chosen")?;
    // What only a check afterwards can catch (see Policy::tripwires).
    let tripwires = match kind {
        Kind::Bwrap => p.tripwires(),
        _ => Vec::new(),
    };
    // A temporary folder of its own (bwrap makes its).
    let tmp = match kind {
        Kind::Seatbelt | Kind::Landlock | Kind::Wasm => {
            sweep_temp();
            let t = std::env::temp_dir().join(format!(
                "ocgen-sb-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_nanos())
            ));
            std::fs::create_dir_all(&t).map_err(|e| format!("{}: {e}", t.display()))?;
            for v in ["TMPDIR", "TMP", "TEMP"] {
                std::env::set_var(v, &t);
            }
            let t = policy::real_path(&t);
            p.write.push(t.clone());
            Some(t)
        }
        _ => None,
    };
    let exit = run(kind, &p, &cmd);
    if let Some(t) = tmp {
        let _ = std::fs::remove_dir_all(t);
    }
    if kind == Kind::Landlock && !matches!(exit, Ok(Exit::Code(0))) {
        eprintln!(
            "ocgen sandbox: under Landlock nothing new can be created right in {} or in a folder on the way to a protected path (.git/, .claude/) — `ocgen sandbox status` says why Landlock is used",
            p.dir.display()
        );
    }
    let exit = exit?;
    let caught = tripwire(&tripwires);
    Ok(match exit {
        Exit::Code(0) if caught => Exit::Code(1),
        e => e,
    })
}

/// Remove the temporary folders of helpers that never got to (a check killed
/// at its timeout takes its helper with it), once they are a day old.
fn sweep_temp() {
    let day = std::time::Duration::from_secs(24 * 60 * 60);
    let Ok(rd) = std::fs::read_dir(std::env::temp_dir()) else {
        return;
    };
    for e in rd.filter_map(Result::ok) {
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > day);
        if old && e.file_name().to_string_lossy().starts_with("ocgen-sb-") {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

/// Run `cmd` in `kind` under `p`, in this process (the helper).
fn run(kind: Kind, p: &Policy, cmd: &str) -> Result<Exit, String> {
    let sh = || {
        let mut c = Command::new("/bin/sh");
        c.args(["-c", cmd]).current_dir(&p.dir);
        c
    };
    let wait = |mut c: Command| -> Result<Exit, String> {
        c.status()
            .map(Exit::from)
            .map_err(|e| format!("can't start the command: {e}"))
    };
    let _ = (&sh, &wait);
    match kind {
        #[cfg(target_os = "macos")]
        Kind::Seatbelt => {
            seatbelt::apply(&seatbelt::profile(p))?;
            wait(sh())
        }
        #[cfg(target_os = "linux")]
        Kind::Bwrap => {
            let path = std::env::var("PATH").unwrap_or_default();
            let bwrap = on_path("bwrap", &path).ok_or("bwrap is gone")?;
            let mut c = Command::new(bwrap);
            c.args(bwrap::args(p, cmd));
            wait(c)
        }
        #[cfg(target_os = "linux")]
        Kind::Landlock => {
            landlock::apply(p)?;
            wait(sh())
        }
        Kind::Wasm => wasm::run(p, cmd).map(Exit::Code),
        #[allow(unreachable_patterns)]
        other => Err(format!("{} isn't available on this OS", other.name())),
    }
}

/// Move aside each of `paths` the command created (see
/// [`Policy::tripwires`]), saying so; whether there was one.
fn tripwire(paths: &[PathBuf]) -> bool {
    let mut caught = false;
    for t in paths {
        if std::fs::symlink_metadata(t).is_err() {
            continue;
        }
        caught = true;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let name = t
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let aside = t.with_file_name(format!("{name}.ocgen-quarantine-{stamp}"));
        match std::fs::rename(t, &aside) {
            Ok(()) => eprintln!(
                "ocgen sandbox: the command created {}, which the hooks' sandbox protects — moved it to {}",
                t.display(),
                aside.display()
            ),
            Err(e) => eprintln!(
                "ocgen sandbox: the command created {}, which the hooks' sandbox protects, and it can't be moved aside: {e}",
                t.display()
            ),
        }
    }
    caught
}

/// End this process the way the command ended: by the same signal.
fn reraise(sig: i32) -> i32 {
    #[cfg(unix)]
    {
        extern "C" {
            fn signal(sig: i32, handler: usize) -> usize;
            fn raise(sig: i32) -> i32;
        }
        // SAFETY: restore the default action, then deliver the signal to self.
        unsafe {
            signal(sig, 0);
            raise(sig);
        }
    }
    128 + sig
}

/// The hooks' sandbox on this machine, for `ocgen sandbox status` and
/// `ocgen verify`.
pub struct Report {
    /// Whether the project's sandbox is on (`OCGEN_SANDBOX=1`).
    pub on: bool,
    /// Each sandbox in this OS's chain and whether it works here.
    pub backends: Vec<(Kind, Result<(), String>)>,
    /// The policy for a command in the project's folder.
    pub policy: Result<Policy, String>,
}

impl Report {
    /// The sandbox an ordinary command would run in: the first in the chain
    /// that works here and isn't for WASI modules only.
    pub fn native(&self) -> Option<Kind> {
        self.backends
            .iter()
            .find(|(k, r)| *k != Kind::Wasm && r.is_ok())
            .map(|(k, _)| *k)
    }

    /// One line on what the chosen sandbox does here.
    pub fn detail(&self) -> String {
        let Ok(p) = &self.policy else {
            return String::new();
        };
        let mut s = format!(
            "{} granted, {} write-protected inside, {} hidden; network {}",
            p.grants().count(),
            p.protect.len(),
            p.hide.len(),
            if p.network { "on" } else { "off" }
        );
        if self.native() == Some(Kind::Landlock) {
            s.push_str(
                "; no new entries in the project's top folder, .git/ or .claude/ (Landlock)",
            );
        }
        s
    }
}

/// The hooks' sandbox for `project` under `env` (the project's hook env).
pub fn report(env: &HashMap<String, String>, project: &Path) -> Report {
    let on = env.get(policy::ON).map(String::as_str) == Some("1");
    let git = git_dirs(project);
    let policy = policy::compile(&policy::Request {
        env,
        project,
        dir: project,
        git: &git,
    });
    // Works here, and can enforce this project's policy (wasmtime: for a WASI
    // module, which no ordinary check is).
    let backends = chain()
        .iter()
        .map(|&k| {
            let ok = probe(k, env).and_then(|()| match &policy {
                Ok(p) if k != Kind::Wasm => admits(k, p, ""),
                _ => Ok(()),
            });
            (k, ok)
        })
        .collect();
    Report {
        on,
        backends,
        policy,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_takes_the_first_sandbox_that_works_and_says_why_none_does() {
        let p = Policy::default();
        let probe = |k: Kind| match k {
            Kind::Bwrap => Err("no namespaces".to_string()),
            _ => Ok(()),
        };
        assert_eq!(
            select(
                &[Kind::Bwrap, Kind::Landlock, Kind::Wasm],
                &probe,
                &p,
                "cargo test"
            ),
            Ok(Kind::Landlock)
        );
        let none = |_| Err("nope".to_string());
        let err = select(&[Kind::Bwrap, Kind::Wasm], &none, &p, "cargo test").unwrap_err();
        assert!(err.contains("never runs it unsandboxed"), "{err}");
        assert!(
            err.contains("bubblewrap: nope; wasmtime (WASI modules only): nope"),
            "{err}"
        );
        // wasmtime only for a WASI module.
        let only_wasm = |k: Kind| {
            if k == Kind::Wasm {
                Ok(())
            } else {
                Err("x".into())
            }
        };
        let err = select(&[Kind::Seatbelt, Kind::Wasm], &only_wasm, &p, "cargo test").unwrap_err();
        assert!(err.contains("isn't a WASI module"), "{err}");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn landlock_refuses_what_it_cant_hide() {
        let p = Policy {
            hide: vec!["/h/.ssh".into()],
            ..Policy::default()
        };
        let ok = |_| Ok(());
        assert_eq!(
            select(&[Kind::Landlock, Kind::Bwrap], &ok, &p, "x"),
            Ok(Kind::Bwrap)
        );
    }

    #[test]
    fn the_chain_ends_with_wasmtime() {
        assert_eq!(chain().last(), Some(&Kind::Wasm));
    }

    #[test]
    fn helper_exe_is_this_binary_or_the_one_a_test_names() {
        let env = HashMap::new();
        // The test harness isn't ocgen (nothing marked it).
        assert_eq!(helper_exe(&env), None);
        let exe = std::env::current_exe().unwrap();
        let env = HashMap::from([(HELPER_VAR.to_string(), exe.to_string_lossy().into_owned())]);
        assert_eq!(helper_exe(&env), Some(exe));
    }

    #[cfg(unix)]
    #[test]
    fn a_tripwire_moves_what_was_created_aside() {
        let t = tempfile::tempdir().unwrap();
        let made = t.path().join(".mcp.json");
        let absent = t.path().join(".other");
        std::fs::write(&made, "{}").unwrap();
        assert!(tripwire(&[made.clone(), absent]));
        assert!(!made.exists());
        let moved: Vec<_> = std::fs::read_dir(t.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            moved
                .iter()
                .any(|n| n.starts_with(".mcp.json.ocgen-quarantine-")),
            "{moved:?}"
        );
        assert!(!tripwire(&[t.path().join("none")]));
    }
}
