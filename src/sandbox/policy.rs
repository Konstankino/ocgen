//! What a hook's sandbox allows, compiled from the hook env (the lists ocgen
//! renders into `settings.json`) for one command in one folder.
//!
//! Everything is denied unless it is listed: the system's own folders and the
//! toolchains' (read, and run), the dependency caches, the project and the
//! command's folder (write) — minus the write-protected paths in them, and
//! minus the hidden ones in whatever is readable. Paths are written as in the
//! sandbox settings: `~/` the home folder, `/` (or `C:/`) absolute, anything
//! else relative to the project (a relative write-protected or hidden entry
//! names a path in the project and the same one in the command's folder).

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::Kind;

/// The env keys ocgen renders for the hooks' sandbox (see `render.rs`).
pub const ON: &str = "OCGEN_SANDBOX";
pub const DENY_WRITE: &str = "OCGEN_SANDBOX_DENY_WRITE";
pub const DENY_READ: &str = "OCGEN_SANDBOX_DENY_READ";
pub const DENY_ENV: &str = "OCGEN_SANDBOX_DENY_ENV";
pub const ALLOW_READ: &str = "OCGEN_SANDBOX_ALLOW_READ";
pub const ALLOW_EXEC: &str = "OCGEN_SANDBOX_ALLOW_EXEC";
pub const ALLOW_WRITE: &str = "OCGEN_SANDBOX_ALLOW_WRITE";
pub const NETWORK: &str = "OCGEN_SANDBOX_NETWORK";

/// The system's folders a command may read and run from, on this OS. Windows
/// needs none: its own folders and Program Files are readable by every
/// AppContainer already.
pub const SYSTEM_EXEC: &[&str] = if cfg!(target_os = "macos") {
    &[
        "/usr",
        "/bin",
        "/sbin",
        "/System",
        "/Library",
        "/opt/homebrew",
        "/opt/local",
        "/nix/store",
        "/Applications/Xcode.app",
        // The developer folder `xcode-select` chose (the link is followed).
        "/private/var/db/xcode_select_link",
    ]
} else if cfg!(target_os = "linux") {
    &[
        "/usr",
        "/bin",
        "/sbin",
        "/lib",
        "/lib32",
        "/lib64",
        "/libx32",
        "/opt",
        "/nix/store",
        "/run/current-system/sw",
        "/snap",
        "/home/linuxbrew/.linuxbrew",
    ]
} else {
    &[]
};

/// The system's files a command may read (not run), on this OS.
pub const SYSTEM_READ: &[&str] = if cfg!(target_os = "macos") {
    &[
        "/private/etc",
        "/private/var/select",
        "/private/var/db/timezone",
        "/dev/random",
        "/dev/urandom",
    ]
} else if cfg!(target_os = "linux") {
    &[
        "/etc",
        // Where /etc/resolv.conf points under systemd-resolved.
        "/run/systemd/resolve",
        "/proc",
        "/sys",
        "/dev/random",
        "/dev/urandom",
    ]
} else {
    &[]
};

/// The devices a command may write (where a sandbox lists devices one by one).
pub const SYSTEM_DEVICES: &[&str] = if cfg!(target_os = "macos") {
    // /dev/fd: /dev/stdout and the like, and bash's process substitution.
    &["/dev/null", "/dev/zero", "/dev/tty", "/dev/fd"]
} else if cfg!(target_os = "linux") {
    // /dev/shm: POSIX shared memory (Python's multiprocessing).
    &[
        "/dev/null",
        "/dev/zero",
        "/dev/full",
        "/dev/tty",
        "/dev/shm",
    ]
} else {
    &[]
};

/// The system's per-user cache folders compilers write to, on this OS: on
/// macOS the user's cache folder (clang's module cache, xcrun's database),
/// which tools find through `confstr`, not the environment.
pub fn system_caches() -> Vec<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        use std::ffi::{c_char, c_int, CStr};
        extern "C" {
            fn confstr(name: c_int, buf: *mut c_char, len: usize) -> usize;
        }
        const CS_DARWIN_USER_CACHE_DIR: c_int = 65538;
        let mut buf = [0 as c_char; 1024];
        // SAFETY: the buffer's length is passed; confstr NUL-terminates it.
        let n = unsafe { confstr(CS_DARWIN_USER_CACHE_DIR, buf.as_mut_ptr(), buf.len()) };
        if n > 0 && n <= buf.len() {
            // SAFETY: NUL-terminated within the buffer.
            let s = unsafe { CStr::from_ptr(buf.as_ptr()) };
            return vec![PathBuf::from(s.to_string_lossy().into_owned())];
        }
    }
    Vec::new()
}

/// What one command may do, and where. Paths are resolved (symlinks followed,
/// plain on Windows); all but `protect` and `hide` exist.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Policy {
    /// The sandbox that enforces it, once chosen.
    pub backend: Option<Kind>,
    pub project: PathBuf,
    /// The folder the command runs in.
    pub dir: PathBuf,
    /// Readable and runnable: the system's folders and the toolchains'.
    pub exec: Vec<PathBuf>,
    /// Readable only.
    pub read: Vec<PathBuf>,
    /// Writable, readable and runnable: the project, the command's folder, the
    /// dependency caches.
    pub write: Vec<PathBuf>,
    /// Never writable, inside the writable roots (missing ones too: they must
    /// not be created).
    pub protect: Vec<PathBuf>,
    /// Never readable, inside what is readable.
    pub hide: Vec<PathBuf>,
    /// Granted paths that are symlinks, with the link's text: a sandbox that
    /// builds a new root recreates them.
    pub links: Vec<(PathBuf, PathBuf)>,
    pub network: bool,
    /// Env variables the command doesn't get.
    pub env_remove: Vec<String>,
}

/// What a policy is compiled from.
pub struct Request<'a> {
    pub env: &'a HashMap<String, String>,
    pub project: &'a Path,
    pub dir: &'a Path,
    /// The repository's git folders (they may lie above the project): readable.
    pub git: &'a [PathBuf],
}

impl Policy {
    /// Every path a command may read: what it may run, read or write.
    pub fn grants(&self) -> impl Iterator<Item = &PathBuf> {
        self.exec.iter().chain(&self.read).chain(&self.write)
    }

    /// The protected paths that don't exist, each with the folder in which the
    /// first missing part would be created (its nearest existing ancestor).
    pub fn missing_protected(&self) -> Vec<(PathBuf, PathBuf)> {
        self.protect
            .iter()
            .filter(|p| fs::symlink_metadata(p).is_err())
            .filter_map(|p| {
                let mut cur = p.as_path();
                while let Some(parent) = cur.parent() {
                    if fs::symlink_metadata(parent).is_ok() {
                        return Some((cur.to_path_buf(), parent.to_path_buf()));
                    }
                    cur = parent;
                }
                None
            })
            .collect()
    }

    /// The paths a command must not create that only a check after it ran can
    /// catch (a "tripwire"), for a sandbox that can protect only what exists:
    /// a missing protected path's first missing part, when it would be created
    /// right in a writable root (where denying new entries would deny them
    /// all). A missing path deeper down is protected through its nearest
    /// existing ancestor instead.
    pub fn tripwires(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for (first, parent) in self.missing_protected() {
            if self.write.iter().any(|w| same(w, &parent)) {
                push_new(&mut out, first);
            }
        }
        out
    }
}

/// The policy for running a command in `req.dir`, or why none can be made.
pub fn compile(req: &Request) -> Result<Policy, String> {
    let get = |k: &str| req.env.get(k).map(String::as_str).unwrap_or("");
    let home = home(req.env);
    let project = real_path(req.project);
    let dir = real_path(req.dir);
    let mut p = Policy {
        project: project.clone(),
        dir: dir.clone(),
        network: get(NETWORK) == "1",
        ..Policy::default()
    };

    // Writable roots: the project, the command's folder, the caches.
    push_new(&mut p.write, project.clone());
    push_new(&mut p.write, dir.clone());
    for e in get(ALLOW_WRITE).split_ascii_whitespace() {
        p.add(Grant::Write, e, home.as_deref());
    }
    for e in SYSTEM_DEVICES {
        p.add(Grant::Write, e, None);
    }
    for e in system_caches() {
        p.add(Grant::Write, &e.to_string_lossy(), None);
    }
    for e in SYSTEM_EXEC
        .iter()
        .copied()
        .chain(get(ALLOW_EXEC).split_ascii_whitespace())
    {
        p.add(Grant::Exec, e, home.as_deref());
    }
    for e in SYSTEM_READ
        .iter()
        .copied()
        .chain(get(ALLOW_READ).split_ascii_whitespace())
    {
        p.add(Grant::Read, e, home.as_deref());
    }
    #[cfg(windows)]
    if let Some(git) = git_install(req.env) {
        p.add(Grant::Exec, &crate::paths::for_shell(&git), None);
    }
    for g in req.git {
        if g.exists() && !p.write.iter().any(|w| is_within(g, w)) {
            push_new(&mut p.read, real_path(g));
        }
    }

    // Write-protected and hidden paths, where something grants them.
    let bases: Vec<&Path> = if dir == project {
        vec![&project]
    } else {
        vec![&project, &dir]
    };
    for t in entries(get(DENY_WRITE), home.as_deref(), &bases) {
        if p.write.iter().any(|w| is_within(&t, w)) {
            push_new(&mut p.protect, t);
        }
    }
    for t in entries(get(DENY_READ), home.as_deref(), &bases) {
        if p.grants().any(|g| is_within(&t, g)) {
            push_new(&mut p.hide, t);
        }
    }
    p.env_remove = get(DENY_ENV)
        .split_ascii_whitespace()
        .filter(|v| env_name(v))
        .map(String::from)
        .collect();

    // A writable root inside a protected path would undo the protection.
    for w in &p.write {
        if let Some(t) = p.protect.iter().find(|t| is_within(w, t)) {
            return Err(format!(
                "{} lies in {}, which the hooks' sandbox write-protects",
                w.display(),
                t.display()
            ));
        }
    }
    Ok(p)
}

#[derive(Clone, Copy)]
enum Grant {
    Read,
    Exec,
    Write,
}

impl Policy {
    /// Grant the path `e` names, if it exists (and once: the widest grant wins).
    fn add(&mut self, how: Grant, e: &str, home: Option<&Path>) {
        let Some(raw) = entry(e, home, &self.project) else {
            return;
        };
        let Ok(meta) = fs::symlink_metadata(&raw) else {
            return;
        };
        let path = real_path(&raw);
        if !path.exists() {
            return;
        }
        if meta.file_type().is_symlink() {
            if let Ok(to) = fs::read_link(&raw) {
                let link = (raw.clone(), to);
                if !self.links.contains(&link) {
                    self.links.push(link);
                }
            }
        }
        let has = |v: &Vec<PathBuf>| v.contains(&path);
        match how {
            Grant::Write => {
                self.read.retain(|r| r != &path);
                self.exec.retain(|r| r != &path);
                push_new(&mut self.write, path);
            }
            Grant::Exec if !has(&self.write) => {
                self.read.retain(|r| r != &path);
                push_new(&mut self.exec, path);
            }
            Grant::Read if !has(&self.write) && !has(&self.exec) => {
                push_new(&mut self.read, path);
            }
            _ => {}
        }
    }
}

/// The home folder: `HOME`, else (Windows) `USERPROFILE`.
pub fn home(env: &HashMap<String, String>) -> Option<PathBuf> {
    ["HOME", "USERPROFILE"]
        .iter()
        .filter_map(|k| env.get(*k))
        .find(|h| !h.is_empty())
        .map(PathBuf::from)
}

/// The path entry `e` names: `~/x` in `home`, an absolute path as is, a
/// relative one in `base`. `None` for a `~` entry without a home.
pub fn entry(e: &str, home: Option<&Path>, base: &Path) -> Option<PathBuf> {
    if e == "~" || e.starts_with("~/") {
        let rest = e[1..].trim_start_matches('/');
        let h = home?;
        return Some(if rest.is_empty() {
            h.to_path_buf()
        } else {
            h.join(rest)
        });
    }
    if Path::new(e).is_absolute() || e.starts_with('/') {
        return Some(PathBuf::from(e));
    }
    Some(base.join(e.strip_prefix("./").unwrap_or(e)))
}

/// The paths `list` names, resolved, each once: a relative entry in each of
/// `bases`, the others once.
pub fn entries(list: &str, home: Option<&Path>, bases: &[&Path]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in list.split_ascii_whitespace() {
        let relative =
            !(e == "~" || e.starts_with("~/") || e.starts_with('/') || Path::new(e).is_absolute());
        let n = if relative { bases.len() } else { 1 };
        for base in &bases[..n.min(bases.len())] {
            if let Some(p) = entry(e, home, base) {
                push_new(&mut out, real_path(&p));
            }
        }
    }
    out
}

/// A path as the kernel sees it: its longest existing part with every symlink
/// resolved, then the rest as written. On Windows, plain (no `\\?\`).
pub fn real_path(path: &Path) -> PathBuf {
    let mut full = path.to_path_buf();
    if full.is_relative() {
        if let Ok(cwd) = std::env::current_dir() {
            full = cwd.join(full);
        }
    }
    let mut rest = Vec::new();
    let mut cur = full.as_path();
    loop {
        if let Ok(real) = fs::canonicalize(cur) {
            let mut real = crate::paths::plain(&real);
            for r in rest.iter().rev() {
                real.push(r);
            }
            return real;
        }
        match (cur.parent(), cur.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name.to_os_string());
                cur = parent;
            }
            _ => return normal(&full),
        }
    }
}

/// `p` with `.` parts and repeated separators dropped.
fn normal(p: &Path) -> PathBuf {
    p.components()
        .filter(|c| !matches!(c, std::path::Component::CurDir))
        .collect()
}

/// Whether `p` is `base` or lies inside it (case-insensitively on Windows).
pub fn is_within(p: &Path, base: &Path) -> bool {
    if cfg!(windows) {
        let low = |x: &Path| PathBuf::from(x.to_string_lossy().to_lowercase());
        low(p).starts_with(low(base))
    } else {
        p.starts_with(base)
    }
}

/// Whether `a` and `b` are the same path (case-insensitively on Windows).
pub fn same(a: &Path, b: &Path) -> bool {
    is_within(a, b) && is_within(b, a)
}

/// Every folder above `p` that lies inside `root`, from `root` down (`root`
/// included, `p` not).
pub fn spine(p: &Path, root: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = p
        .ancestors()
        .skip(1)
        .take_while(|a| is_within(a, root))
        .map(Path::to_path_buf)
        .collect();
    out.reverse();
    out
}

/// An environment variable's name (what `unset` takes).
pub fn env_name(s: &str) -> bool {
    s.bytes().next().is_some_and(|b| !b.is_ascii_digit())
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

pub fn push_new<T: PartialEq>(v: &mut Vec<T>, x: T) {
    if !v.contains(&x) {
        v.push(x);
    }
}

/// Where Git for Windows is installed, when that isn't a system folder every
/// AppContainer may read already (a per-user or scoop install): found from the
/// `sh` on PATH (`<git>\usr\bin\sh.exe` or `<git>\bin\sh.exe`).
#[cfg(windows)]
fn git_install(env: &HashMap<String, String>) -> Option<PathBuf> {
    let path = env.get("PATH").or_else(|| env.get("Path"))?;
    let sh = crate::paths::find_in(std::ffi::OsStr::new(path), "sh", ".exe")?;
    let bin = sh.parent()?;
    let git = if bin.file_name()? == "bin" && bin.parent()?.file_name()? == "usr" {
        bin.parent()?.parent()?
    } else {
        bin.parent()?
    };
    let git = real_path(git);
    let system = ["ProgramFiles", "ProgramFiles(x86)", "SystemRoot"]
        .iter()
        .filter_map(|k| env.get(*k))
        .any(|s| is_within(&git, Path::new(s)));
    (!system).then_some(git)
}

/// How the writable roots split around the protected paths, for a sandbox
/// that can only grant whole folders and files and can't take a path inside
/// a grant back (Landlock, WASI): `write` the folders and files granted in
/// full; `spine` the folders on the way to a protected path, readable (and
/// runnable) but with no entry created, renamed or removed in them. A
/// protected path is read-only through its folder's spine grant; a missing
/// one can't be created, since its nearest existing folder is on the spine.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Split {
    pub write: Vec<PathBuf>,
    pub spine: Vec<PathBuf>,
}

/// One folder entry: its name, and whether it is a symlink.
pub type Listing = Vec<(OsString, bool)>;

/// [`Split`] the writable `roots` around `protect`, listing folders with
/// `list` (`fs::read_dir` in production). Symlinks on the spine are never
/// granted for writing (a link could point anywhere).
pub fn split(roots: &[PathBuf], protect: &[PathBuf], list: &dyn Fn(&Path) -> Listing) -> Split {
    fn visit(d: &Path, protect: &[PathBuf], list: &dyn Fn(&Path) -> Listing, out: &mut Split) {
        let below: Vec<&PathBuf> = protect
            .iter()
            .filter(|p| is_within(p, d) && !same(p, d))
            .collect();
        if below.is_empty() {
            push_new(&mut out.write, d.to_path_buf());
            return;
        }
        push_new(&mut out.spine, d.to_path_buf());
        for (name, link) in list(d) {
            let c = d.join(&name);
            if protect.iter().any(|p| same(p, &c)) || link {
                continue;
            }
            if below.iter().any(|p| is_within(p, &c)) {
                visit(&c, protect, list, out);
            } else {
                push_new(&mut out.write, c);
            }
        }
    }
    let mut out = Split::default();
    for r in roots {
        if !protect.iter().any(|p| is_within(r, p)) {
            visit(r, protect, list, &mut out);
        }
    }
    // A root inside another root's spine may have been granted in full.
    let spine = out.spine.clone();
    out.write.retain(|w| !spine.contains(w));
    out
}

/// A folder's entries, for [`split`].
pub fn list_dir(d: &Path) -> Listing {
    let Ok(rd) = fs::read_dir(d) else {
        return Vec::new();
    };
    let mut out: Listing = rd
        .filter_map(Result::ok)
        .map(|e| {
            let link = e.file_type().map(|t| t.is_symlink()).unwrap_or(true);
            (e.file_name(), link)
        })
        .collect();
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder that exists nowhere, so results are the same on every OS.
    #[cfg(unix)]
    const NOWHERE: &str = "/ocgen-sandbox-test-nowhere";

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[cfg(unix)]
    #[test]
    fn entries_name_home_absolute_and_project_paths() {
        let home = PathBuf::from(format!("{NOWHERE}/home"));
        let p = PathBuf::from(format!("{NOWHERE}/proj"));
        let d = PathBuf::from(format!("{NOWHERE}/proj/.claude/worktrees/w1"));
        let at = |s: &str| PathBuf::from(format!("{NOWHERE}/{s}"));
        let list = format!("~/.claude ./.claude/hooks .git/hooks {NOWHERE}/abs ~/.claude ~");
        assert_eq!(
            entries(&list, Some(&home), &[&p, &d]),
            [
                at("home/.claude"),
                at("proj/.claude/hooks"),
                at("proj/.claude/worktrees/w1/.claude/hooks"),
                at("proj/.git/hooks"),
                at("proj/.claude/worktrees/w1/.git/hooks"),
                at("abs"),
                at("home"),
            ],
            "each once, a relative one in each base"
        );
        // No home: no `~` entries.
        assert_eq!(entries("~/.ssh ./x", None, &[&p]), [at("proj/x")]);
    }

    #[cfg(unix)]
    #[test]
    fn real_path_resolves_symlinks_and_keeps_a_missing_tail() {
        let t = tempfile::tempdir().unwrap();
        let base = real_path(t.path());
        fs::create_dir_all(t.path().join("dotfiles/claude")).unwrap();
        std::os::unix::fs::symlink(t.path().join("dotfiles/claude"), t.path().join(".claude"))
            .unwrap();
        let link = t.path().join(".claude/ocgen/approvals");
        assert_eq!(
            real_path(&link),
            base.join("dotfiles/claude/ocgen/approvals")
        );
        assert_eq!(
            real_path(Path::new(&format!("{}//x/", t.path().display()))),
            base.join("x")
        );
        assert_eq!(
            real_path(Path::new("/ocgen-no-such/./a//b")),
            PathBuf::from("/ocgen-no-such/a/b")
        );
    }

    /// A project, a worker's worktree inside its `.claude`, a home with a cache,
    /// a toolchain and credentials.
    #[cfg(unix)]
    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
        let t = tempfile::tempdir().unwrap();
        for d in [
            "home/.ssh",
            "home/.cargo/registry",
            "home/.cargo/bin",
            "home/.claude",
            "p/.claude/hooks",
            "p/.claude/worktrees/w1/.claude",
            "p/.git/hooks",
            "p/src",
        ] {
            fs::create_dir_all(t.path().join(d)).unwrap();
        }
        fs::write(t.path().join("p/.claude/settings.json"), "{}").unwrap();
        let root = real_path(t.path());
        let (home, p) = (root.join("home"), root.join("p"));
        (t, root, home, p)
    }

    #[cfg(unix)]
    fn hook_env(home: &Path, extra: &[(&str, &str)]) -> HashMap<String, String> {
        let mut e = env(&[
            ("HOME", home.to_str().unwrap()),
            (
                DENY_WRITE,
                "~/.claude ~/.cargo/bin ./.claude/settings.json ./.claude/settings.local.json ./.claude/hooks ./.git/hooks ./.mcp.json",
            ),
            (DENY_READ, "~/.ssh ~/.aws"),
            (DENY_ENV, "GH_TOKEN 1BAD NPM_TOKEN"),
            (ALLOW_EXEC, "~/.cargo/bin ~/.rustup"),
            (ALLOW_WRITE, "~/.cargo/registry"),
        ]);
        for (k, v) in extra {
            e.insert(k.to_string(), v.to_string());
        }
        e
    }

    #[cfg(unix)]
    #[test]
    fn compile_grants_the_lists_and_protects_inside_the_writable_roots() {
        let (_t, _root, home, p) = fixture();
        let e = hook_env(&home, &[]);
        let pol = compile(&Request {
            env: &e,
            project: &p,
            dir: &p,
            git: &[],
        })
        .unwrap();
        assert_eq!(pol.project, p);
        assert_eq!(pol.write[..2], [p.clone(), home.join(".cargo/registry")]);
        assert!(pol.exec.contains(&home.join(".cargo/bin")));
        assert!(
            !pol.exec.iter().any(|x| x.ends_with(".rustup")),
            "missing: left out"
        );
        assert!(pol.exec.contains(&PathBuf::from("/usr")));
        // Protected: only what lies in a writable root (the rest is denied
        // anyway), missing ones included.
        assert_eq!(
            pol.protect,
            [
                p.join(".claude/settings.json"),
                p.join(".claude/settings.local.json"),
                p.join(".claude/hooks"),
                p.join(".git/hooks"),
                p.join(".mcp.json"),
            ]
        );
        // Hidden: nothing readable holds the credentials.
        assert!(pol.hide.is_empty(), "{:?}", pol.hide);
        assert_eq!(pol.env_remove, ["GH_TOKEN", "NPM_TOKEN"]);
        assert!(!pol.network);
        assert_eq!(
            pol.tripwires(),
            [p.join(".mcp.json")],
            "settings.local.json is caught by its folder"
        );
        assert_eq!(
            pol.missing_protected(),
            [
                (p.join(".claude/settings.local.json"), p.join(".claude")),
                (p.join(".mcp.json"), p.clone()),
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn compile_protects_a_worktree_inside_dot_claude_and_hides_what_a_grant_holds() {
        let (_t, root, home, p) = fixture();
        let d = p.join(".claude/worktrees/w1");
        // The user granted their whole home folder for reading.
        let e = hook_env(&home, &[(ALLOW_READ, "~"), (NETWORK, "1")]);
        let pol = compile(&Request {
            env: &e,
            project: &p,
            dir: &d,
            git: &[root.join("p/.git"), root.join("elsewhere")],
        })
        .unwrap();
        assert_eq!(pol.write[..2], [p.clone(), d.clone()]);
        assert!(pol.protect.contains(&d.join(".claude/hooks")));
        assert!(pol.protect.contains(&d.join(".git/hooks")));
        assert_eq!(pol.hide, [home.join(".ssh"), home.join(".aws")]);
        assert!(pol.read.contains(&home));
        assert!(
            !pol.read.contains(&root.join("p/.git")),
            "inside a writable root"
        );
        assert!(pol.network);
    }

    #[cfg(unix)]
    #[test]
    fn compile_refuses_a_writable_root_inside_a_protected_path() {
        let (_t, _root, home, _p) = fixture();
        let inside = home.join(".claude");
        let e = hook_env(&home, &[]);
        let err = compile(&Request {
            env: &e,
            project: &inside,
            dir: &inside,
            git: &[],
        })
        .unwrap_err();
        assert!(err.contains("write-protects"), "{err}");
    }

    #[test]
    fn home_falls_back_to_the_user_profile() {
        assert_eq!(
            home(&env(&[("USERPROFILE", "C:/Users/x")])),
            Some(PathBuf::from("C:/Users/x"))
        );
        assert_eq!(
            home(&env(&[("HOME", ""), ("USERPROFILE", "C:/Users/x")])),
            Some(PathBuf::from("C:/Users/x"))
        );
        assert_eq!(
            home(&env(&[("HOME", "/h"), ("USERPROFILE", "u")])),
            Some("/h".into())
        );
        assert_eq!(home(&env(&[])), None);
    }

    #[test]
    fn helpers() {
        assert!(env_name("GH_TOKEN") && env_name("_x1"));
        assert!(!env_name("") && !env_name("1A") && !env_name("A-B") && !env_name("A B"));
        assert!(is_within(Path::new("/a/b/c"), Path::new("/a/b")));
        assert!(is_within(Path::new("/a/b"), Path::new("/a/b")));
        assert!(!is_within(Path::new("/a/bc"), Path::new("/a/b")));
        assert_eq!(
            spine(Path::new("/r/a/b/c"), Path::new("/r")),
            [PathBuf::from("/r"), "/r/a".into(), "/r/a/b".into()]
        );
        assert_eq!(
            spine(Path::new("/r/x"), Path::new("/r")),
            [PathBuf::from("/r")]
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_paths_compare_without_case() {
        assert!(is_within(
            Path::new(r"C:\Users\X\Proj\src"),
            Path::new("c:/users/x/proj")
        ));
        assert!(same(Path::new(r"C:\A"), Path::new("c:/a")));
    }

    /// A fake tree for [`split`]: each folder's entries; names ending in `@`
    /// are symlinks.
    fn tree(spec: &[(&str, &[&str])]) -> impl Fn(&Path) -> Listing {
        let map: HashMap<PathBuf, Listing> = spec
            .iter()
            .map(|(d, names)| {
                let names = names
                    .iter()
                    .map(|n| match n.strip_suffix('@') {
                        Some(n) => (OsString::from(n), true),
                        None => (OsString::from(*n), false),
                    })
                    .collect();
                (PathBuf::from(d), names)
            })
            .collect();
        move |d: &Path| map.get(d).cloned().unwrap_or_default()
    }

    fn paths(v: &[&str]) -> Vec<PathBuf> {
        v.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn split_grants_a_root_with_nothing_protected_in_full() {
        let list = tree(&[]);
        let s = split(&paths(&["/p", "/c"]), &paths(&["/elsewhere/x"]), &list);
        assert_eq!(s.write, paths(&["/p", "/c"]));
        assert!(s.spine.is_empty());
    }

    #[test]
    fn split_walks_around_protected_paths() {
        let list = tree(&[
            ("/p", &[".claude", ".git", "src", "Cargo.toml", "link@"]),
            (
                "/p/.claude",
                &["settings.json", "hooks", "worktrees", "notes"],
            ),
            ("/p/.claude/worktrees", &["w1"]),
            ("/p/.claude/worktrees/w1", &[".claude", "src"]),
            ("/p/.claude/worktrees/w1/.claude", &["hooks", "x"]),
            ("/p/.git", &["hooks", "config", "objects"]),
        ]);
        let protect = paths(&[
            "/p/.claude/settings.json",
            "/p/.claude/hooks",
            "/p/.claude/settings.local.json",
            "/p/.git/hooks",
            "/p/.git/config",
            "/p/.mcp.json",
            "/p/.claude/worktrees/w1/.claude/hooks",
        ]);
        let roots = paths(&["/p", "/p/.claude/worktrees/w1"]);
        let s = split(&roots, &protect, &list);
        assert_eq!(
            s.spine,
            paths(&[
                "/p",
                "/p/.claude",
                "/p/.claude/worktrees",
                "/p/.claude/worktrees/w1",
                "/p/.claude/worktrees/w1/.claude",
                "/p/.git",
            ])
        );
        assert_eq!(
            s.write,
            paths(&[
                "/p/.claude/worktrees/w1/.claude/x",
                "/p/.claude/worktrees/w1/src",
                "/p/.claude/notes",
                "/p/.git/objects",
                "/p/src",
                "/p/Cargo.toml",
            ])
        );
        // No write grant covers a protected path, and every protected path's
        // folder is on the spine (so nothing new appears next to it).
        for p in &protect {
            assert!(!s.write.iter().any(|w| is_within(p, w)), "{p:?}");
            assert!(
                s.spine.iter().any(|d| Some(d.as_path()) == p.parent()),
                "{p:?}"
            );
        }
    }

    #[test]
    fn split_skips_a_root_inside_a_protected_path() {
        let list = tree(&[]);
        let s = split(
            &paths(&["/p/.claude/hooks/x"]),
            &paths(&["/p/.claude/hooks"]),
            &list,
        );
        assert_eq!(s, Split::default());
    }
}
