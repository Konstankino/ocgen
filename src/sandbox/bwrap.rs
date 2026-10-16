//! Linux: bubblewrap (`bwrap`), a separate program, building a new, empty root
//! for the command with only the granted paths mounted in it.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::policy::{is_within, push_new, spine, Policy};

/// Paths `--proc` and `--dev` already provide. (A grant under /tmp — a project
/// checked out there — is bound on top of the fresh `--tmpfs /tmp`.)
const PROVIDED: [&str; 2] = ["/proc", "/dev"];

/// The namespaces the command gets: its own users, processes, IPC and host
/// name, and its own network (none) unless the network is on.
fn namespaces(network: bool) -> Vec<&'static str> {
    let mut a = vec![
        "--die-with-parent",
        "--unshare-user",
        "--unshare-ipc",
        "--unshare-pid",
        "--unshare-uts",
        "--unshare-cgroup-try",
    ];
    if !network {
        a.push("--unshare-net");
    }
    a
}

/// The `bwrap` options that run `cmd` under `p`, in order: namespaces; a
/// fresh /proc, /dev and /tmp; the granted paths, shallowest first so a deeper
/// grant lands on top (read-only, or writable for the writable roots), and the
/// symlinks among them; then, shallowest first, every folder above a protected
/// path bound onto itself (a mount point can't be renamed away), the protected
/// paths read-only — a missing one through its nearest existing folder, unless
/// that is a writable root (see [`Policy::tripwires`]) — and a writable root
/// inside one (a worker's worktree in `.claude/`) writable again; last, an
/// empty folder over each hidden folder and /dev/null over each hidden file.
pub fn args(p: &Policy, cmd: &str) -> Vec<String> {
    let mut a: Vec<String> = namespaces(p.network)
        .into_iter()
        .map(String::from)
        .collect();
    for x in ["--proc", "/proc", "--dev", "/dev", "--tmpfs", "/tmp"] {
        a.push(x.into());
    }
    let provided = |g: &Path| PROVIDED.iter().any(|x| is_within(g, Path::new(x)));
    let mut grants: Vec<(&PathBuf, bool)> = p
        .exec
        .iter()
        .chain(&p.read)
        .map(|g| (g, false))
        .chain(p.write.iter().map(|g| (g, true)))
        .filter(|(g, _)| !provided(g))
        .collect();
    grants.sort_by_key(|(g, _)| g.components().count());
    for (g, write) in &grants {
        let how = if *write { "--bind" } else { "--ro-bind" };
        a.extend([how.into(), s(g), s(g)]);
    }
    for (link, to) in &p.links {
        let inside = link
            .parent()
            .is_some_and(|d| grants.iter().any(|(g, _)| is_within(d, g)));
        if !provided(link) && !inside {
            a.extend(["--symlink".into(), s(to), s(link)]);
        }
    }

    // What to keep read-only: each protected path that exists, else its
    // nearest existing folder unless that is a writable root.
    let mut ro: Vec<PathBuf> = Vec::new();
    for t in &p.protect {
        if t.exists() {
            push_new(&mut ro, t.clone());
        }
    }
    for (_, parent) in p.missing_protected() {
        if !p.write.iter().any(|w| super::policy::same(w, &parent)) {
            push_new(&mut ro, parent);
        }
    }
    let mut above: Vec<PathBuf> = Vec::new();
    for t in &ro {
        for w in p.write.iter().filter(|w| is_within(t, w)) {
            for d in spine(t, w).into_iter().skip(1) {
                push_new(&mut above, d);
            }
        }
    }
    // The writable roots inside a read-only path, open again.
    let reopen: Vec<&PathBuf> = p
        .write
        .iter()
        .filter(|w| {
            ro.iter()
                .any(|t| is_within(w, t) && !super::policy::same(w, t))
        })
        .collect();
    let writable =
        |d: &Path| !ro.iter().any(|t| is_within(d, t)) || reopen.iter().any(|w| is_within(d, w));
    let mut layers: Vec<(&PathBuf, bool)> = Vec::new();
    for d in above.iter().filter(|d| d.is_dir() && !ro.contains(d)) {
        layers.push((d, writable(d)));
    }
    layers.extend(ro.iter().map(|t| (t, false)));
    layers.extend(reopen.iter().map(|w| (*w, true)));
    // Shallowest first, so a deeper layer lands on top.
    layers.sort_by_key(|(t, _)| t.components().count());
    for (t, write) in layers {
        let how = if write { "--bind" } else { "--ro-bind" };
        a.extend([how.into(), s(t), s(t)]);
    }
    for h in &p.hide {
        if h.is_dir() {
            a.extend(["--tmpfs".into(), s(h)]);
        } else if h.exists() {
            a.extend(["--ro-bind".into(), "/dev/null".into(), s(h)]);
        }
    }
    a.extend(["--chdir".into(), s(&p.dir)]);
    for v in ["TMPDIR", "TMP", "TEMP"] {
        a.extend(["--setenv".into(), v.into(), "/tmp".into()]);
    }
    a.extend(["--".into(), "/bin/sh".into(), "-c".into(), cmd.into()]);
    a
}

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// `bwrap` on `path` (a `PATH` value), when it can make the namespaces the
/// sandbox needs here (unprivileged user namespaces may be off: Ubuntu 24.04
/// and later restrict them with AppArmor).
pub fn probe(path: &str) -> Result<PathBuf, String> {
    let bwrap = super::on_path("bwrap", path)
        .ok_or("bubblewrap (bwrap) isn't installed (e.g. sudo apt install bubblewrap)")?;
    let out = Command::new(&bwrap)
        .args(namespaces(false))
        .args(["--ro-bind", "/", "/", "true"])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("{} can't run: {e}", bwrap.display()))?;
    if out.status.success() {
        return Ok(bwrap);
    }
    let why = String::from_utf8_lossy(&out.stderr).trim().to_string();
    let hint =
        match std::fs::read_to_string("/proc/sys/kernel/apparmor_restrict_unprivileged_userns") {
            Ok(v) if v.trim() == "1" => {
                " (kernel.apparmor_restrict_unprivileged_userns=1 restricts user namespaces)"
            }
            _ => "",
        };
    Err(format!("bwrap can't make its namespaces here: {why}{hint}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn pos(a: &[String], x: &str, y: &str) -> usize {
        a.windows(2)
            .rposition(|w| w[0] == x && w[1] == y)
            .unwrap_or_else(|| panic!("no {x} {y} in {a:?}"))
    }

    #[cfg(unix)]
    #[test]
    fn args_build_a_root_from_the_grants_and_protect_inside_them() {
        let t = tempfile::tempdir().unwrap();
        let root = super::super::policy::real_path(t.path());
        for d in [
            "p/.claude/worktrees/w1/.claude/hooks",
            "p/.git/hooks",
            "h/.ssh",
            "h/.cargo/bin",
        ] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        std::fs::write(root.join("h/.npmrc"), "").unwrap();
        let (p, d) = (root.join("p"), root.join("p/.claude/worktrees/w1"));
        let pol = Policy {
            project: p.clone(),
            dir: d.clone(),
            exec: vec!["/usr".into(), root.join("h/.cargo/bin")],
            read: vec![root.join("h")],
            write: vec![p.clone(), d.clone(), "/dev/null".into()],
            protect: vec![
                p.join(".claude/settings.local.json"),
                p.join(".git/hooks"),
                p.join(".mcp.json"),
                d.join(".claude/hooks"),
            ],
            hide: vec![
                root.join("h/.ssh"),
                root.join("h/.npmrc"),
                root.join("h/.aws"),
            ],
            links: vec![("/bin".into(), "usr/bin".into())],
            ..Policy::default()
        };
        let a = args(&pol, "cargo test");
        let st = |p: &Path| p.to_string_lossy().into_owned();
        assert!(a.contains(&"--unshare-net".to_string()), "{a:?}");
        // Grants, shallowest first; /dev comes from --dev.
        let usr = pos(&a, "--ro-bind", "/usr");
        let h = pos(&a, "--ro-bind", &st(&root.join("h")));
        let bin = pos(&a, "--ro-bind", &st(&root.join("h/.cargo/bin")));
        let proj = pos(&a, "--bind", &st(&p));
        assert!(usr < h && h < bin, "{a:?}");
        assert!(
            !a.windows(2)
                .any(|w| w[0] == "--bind" && w[1] == "/dev/null"),
            "{a:?}"
        );
        pos(&a, "--symlink", "usr/bin");
        // .claude read-only (settings.local.json is missing in it), the
        // folders above protected paths bound onto themselves (read-only in
        // .claude, writable in the worktree), the worktree writable again,
        // then its own hooks read-only — shallowest first.
        let dot = pos(&a, "--ro-bind", &st(&p.join(".claude")));
        let above = pos(&a, "--ro-bind", &st(&p.join(".claude/worktrees")));
        let reopen = pos(&a, "--bind", &st(&d));
        let own_dot = pos(&a, "--bind", &st(&d.join(".claude")));
        let own = pos(&a, "--ro-bind", &st(&d.join(".claude/hooks")));
        pos(&a, "--bind", &st(&p.join(".git")));
        pos(&a, "--ro-bind", &st(&p.join(".git/hooks")));
        assert!(
            proj < dot && dot < above && above < reopen && reopen < own_dot && own_dot < own,
            "{a:?}"
        );
        // .mcp.json is missing right in the project: left to the tripwire.
        assert!(!a.iter().any(|x| x.ends_with(".mcp.json")), "{a:?}");
        assert!(
            !a.windows(2).any(|w| w[0] == "--ro-bind" && w[1] == st(&p)),
            "{a:?}"
        );
        // Hidden: an empty folder, /dev/null over a file, nothing when missing.
        pos(&a, "--tmpfs", &st(&root.join("h/.ssh")));
        assert!(a
            .windows(3)
            .any(|w| w == ["--ro-bind", "/dev/null", &st(&root.join("h/.npmrc"))]));
        assert!(!a.iter().any(|x| x.ends_with(".aws")));
        assert_eq!(a[a.len() - 4..], ["--", "/bin/sh", "-c", "cargo test"]);
        pos(&a, "--chdir", &st(&d));
    }

    #[cfg(unix)]
    #[test]
    fn a_project_under_tmp_is_bound_over_the_fresh_tmp() {
        let pol = Policy {
            project: "/tmp/ocgen-sb-nowhere/p".into(),
            dir: "/tmp/ocgen-sb-nowhere/p".into(),
            write: vec!["/tmp/ocgen-sb-nowhere/p".into(), "/dev/null".into()],
            ..Policy::default()
        };
        let a = args(&pol, "x");
        let tmpfs = pos(&a, "--tmpfs", "/tmp");
        let bind = pos(&a, "--bind", "/tmp/ocgen-sb-nowhere/p");
        assert!(tmpfs < bind, "{a:?}");
        assert!(
            !a.iter().any(|x| x == "/dev/null"),
            "--dev provides it: {a:?}"
        );
    }

    #[test]
    fn the_network_keeps_the_hosts_namespace() {
        let pol = Policy {
            network: true,
            ..Policy::default()
        };
        assert!(!args(&pol, "x").contains(&"--unshare-net".to_string()));
    }
}
