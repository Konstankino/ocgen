//! Native Windows: what the AppContainer may open, as access control entries
//! on the files themselves. An AppContainer can open only what an entry
//! names it (or a capability it holds) in: one per project (the "package"),
//! plus one capability every project's container shares for the toolchains
//! and caches, so those big trees are granted once per machine.
//!
//! Only allow entries. Windows doesn't hold an AppContainer to a deny entry
//! for it, nor to a higher integrity label, on a path whose folder grants it
//! (on Windows 11 and Server 2025, a file carrying both was still written).
//! So a protected path inside a writable root stops inheriting what its
//! folder passes down, and gets only reading and running; the folders on the
//! way to it, the same, and they may not be removed or renamed — nor what is
//! in them removed through them — so it can't be moved away either.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::policy::{is_within, push_new, same, spine, Policy};

// Access rights (winnt.h).
const FILE_WRITE_DATA: u32 = 0x2; // FILE_ADD_FILE on a folder
const FILE_APPEND_DATA: u32 = 0x4; // FILE_ADD_SUBDIRECTORY on a folder
const FILE_DELETE_CHILD: u32 = 0x40;
const DELETE: u32 = 0x1_0000;

/// Read and run (FILE_GENERIC_READ | FILE_GENERIC_EXECUTE).
pub const READ_RUN: u32 = 0x1200A9;
/// Read, run, write, delete (no changing permissions or owner).
pub const MODIFY: u32 = READ_RUN | 0x120116 | DELETE | FILE_DELETE_CHILD;
/// A folder on the way to a protected path: everything but being removed or
/// renamed, or removing what is in it through it (each entry in it still
/// can be removed by its own right to — a protected one has none).
pub const KEEP: u32 = MODIFY & !(DELETE | FILE_DELETE_CHILD);
/// Creating an entry in a folder.
pub const ADD: u32 = FILE_WRITE_DATA | FILE_APPEND_DATA;
/// The folder a missing protected path would be created in: [`KEEP`], and
/// nothing new in it.
pub const KEEP_CLOSED: u32 = KEEP & !ADD;

/// Whose entry: the project's container, or the capability every project's
/// container holds (toolchains and caches).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Who {
    Package,
    Toolchain,
}

/// Where an entry applies: on its path, on what is below it (a folder's),
/// or both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Reach {
    Itself,
    Below,
    Both,
}

/// One allow entry to have on `path`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ace {
    pub path: PathBuf,
    pub who: Who,
    pub mask: u32,
    pub reach: Reach,
}

/// The mandatory label a path gets: an AppContainer runs at Low integrity,
/// so its writable roots are labelled Low, and protected paths inside them
/// Medium again (not relied on: see the module's note).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Low,
    Medium,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Plan {
    pub aces: Vec<Ace>,
    /// Paths that stop inheriting from their folder: the container's
    /// entries on them are the ones in `aces` (none for a hidden path).
    pub cuts: Vec<PathBuf>,
    pub labels: Vec<(PathBuf, Level)>,
}

/// The entries, cuts and labels that give `p`'s command what it may have.
pub fn plan(p: &Policy) -> Plan {
    let mut out = Plan::default();
    let add = |out: &mut Plan, path: &Path, who, mask, reach| {
        push_new(
            &mut out.aces,
            Ace {
                path: path.to_path_buf(),
                who,
                mask,
                reach,
            },
        )
    };
    let reach = |path: &Path| {
        if path.is_dir() {
            Reach::Both
        } else {
            Reach::Itself
        }
    };
    let who = |w: &Path| {
        if same(w, &p.project) || same(w, &p.dir) {
            Who::Package
        } else {
            Who::Toolchain
        }
    };
    // The innermost writable root holding a path (a worktree's, not the
    // project's around it).
    let root = |t: &Path| {
        p.write
            .iter()
            .filter(|w| is_within(t, w))
            .max_by_key(|w| w.components().count())
    };
    for g in p.exec.iter().chain(&p.read) {
        add(&mut out, g, Who::Toolchain, READ_RUN, reach(g));
    }
    // Protected paths (one inside another is covered by it).
    let protect: Vec<&PathBuf> = p
        .protect
        .iter()
        .filter(|t| t.exists())
        .filter(|t| {
            !p.protect
                .iter()
                .any(|o| !same(o, t) && is_within(t, o) && o.exists())
        })
        .collect();
    // The folders on the way to them, each with what it may do itself.
    let mut kept: Vec<(PathBuf, Who, u32)> = Vec::new();
    let mut keep = |d: &Path, w: Who, mask: u32| match kept.iter_mut().find(|(k, ..)| same(k, d)) {
        Some(k) => k.2 &= mask,
        None => kept.push((d.to_path_buf(), w, mask)),
    };
    for t in &protect {
        let Some(w) = root(t) else { continue };
        push_new(&mut out.cuts, t.to_path_buf());
        add(&mut out, t, who(w), READ_RUN, reach(t));
        for d in spine(t, w) {
            keep(&d, who(w), KEEP);
        }
    }
    // A missing one can't be created: nothing new in its nearest existing
    // folder — unless that is a writable root, where the tripwire catches it.
    for (_, parent) in p.missing_protected() {
        if p.write.iter().any(|w| same(w, &parent)) || protect.iter().any(|t| is_within(&parent, t))
        {
            continue;
        }
        let Some(w) = root(&parent) else { continue };
        keep(&parent, who(w), KEEP_CLOSED);
        for d in spine(&parent, w) {
            keep(&d, who(w), KEEP);
        }
    }
    let mut labels = Vec::new();
    for w in &p.write {
        match kept.iter().find(|(k, ..)| same(k, w)) {
            Some((_, _, mask)) => {
                // Inside another root (a worktree in the project's
                // `.claude/`), it would inherit being removable from it.
                if p.write.iter().any(|o| !same(o, w) && is_within(w, o)) {
                    push_new(&mut out.cuts, w.clone());
                }
                add(&mut out, w, who(w), *mask, Reach::Itself);
                add(&mut out, w, who(w), MODIFY, Reach::Below);
            }
            None => add(&mut out, w, who(w), MODIFY, Reach::Both),
        }
        push_new(&mut labels, (w.clone(), Level::Low));
    }
    // The kept folders below a root: cut off from its grant, then the same.
    for (d, w, mask) in &kept {
        if p.write.iter().any(|r| same(r, d)) {
            continue;
        }
        push_new(&mut out.cuts, d.clone());
        add(&mut out, d, *w, *mask, Reach::Itself);
        add(&mut out, d, *w, MODIFY, Reach::Below);
    }
    for t in &protect {
        push_new(&mut labels, (t.to_path_buf(), Level::Medium));
    }
    // A hidden path: cut off, with nothing for the container.
    for h in p.hide.iter().filter(|h| h.exists()) {
        push_new(&mut out.cuts, h.clone());
    }
    out.labels = labels;
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_plan_grants_roots_and_never_grants_protected_paths_more_than_reading() {
        let t = tempfile::tempdir().unwrap();
        let root = super::super::policy::real_path(t.path());
        let p = root.join("p");
        for d in [".claude/hooks", ".git/hooks", "src"] {
            std::fs::create_dir_all(p.join(d)).unwrap();
        }
        std::fs::write(p.join(".claude/settings.json"), "{}").unwrap();
        std::fs::create_dir_all(root.join("h/.cargo/registry")).unwrap();
        std::fs::create_dir_all(root.join("h/.ssh")).unwrap();
        let pol = Policy {
            project: p.clone(),
            dir: p.clone(),
            exec: vec![root.join("h/.cargo/bin")],
            read: vec![root.join("h")],
            write: vec![p.clone(), root.join("h/.cargo/registry")],
            protect: vec![
                p.join(".claude/hooks"),
                p.join(".claude/settings.json"),
                p.join(".claude/settings.local.json"),
                p.join(".mcp.json"),
            ],
            hide: vec![root.join("h/.ssh"), root.join("h/.aws")],
            ..Policy::default()
        };
        let plan = plan(&pol);
        let has = |path: &Path, who, mask, reach| {
            plan.aces.contains(&Ace {
                path: path.to_path_buf(),
                who,
                mask,
                reach,
            })
        };
        let cut = |path: &Path| plan.cuts.iter().any(|c| c == path);
        let on = |path: &Path| plan.aces.iter().filter(|a| a.path == path).count();
        assert!(has(
            &root.join("h/.cargo/registry"),
            Who::Toolchain,
            MODIFY,
            Reach::Both
        ));
        assert!(has(&root.join("h"), Who::Toolchain, READ_RUN, Reach::Both));
        // The project: everything below it, but it can't be removed or
        // renamed itself, nor what is in it removed through it.
        assert!(has(&p, Who::Package, KEEP, Reach::Itself));
        assert!(has(&p, Who::Package, MODIFY, Reach::Below));
        assert_eq!(on(&p), 2);
        assert!(!cut(&p));
        // Protected paths: cut off, read and run only.
        for t in [p.join(".claude/hooks"), p.join(".claude/settings.json")] {
            assert!(cut(&t), "{}", t.display());
            assert_eq!(on(&t), 1, "{}", t.display());
        }
        assert!(has(
            &p.join(".claude/hooks"),
            Who::Package,
            READ_RUN,
            Reach::Both
        ));
        assert!(has(
            &p.join(".claude/settings.json"),
            Who::Package,
            READ_RUN,
            Reach::Itself
        ));
        // Their folder: cut off, kept, and closed (settings.local.json is
        // missing there: nothing new in it).
        assert!(cut(&p.join(".claude")));
        assert!(has(
            &p.join(".claude"),
            Who::Package,
            KEEP_CLOSED,
            Reach::Itself
        ));
        assert!(has(&p.join(".claude"), Who::Package, MODIFY, Reach::Below));
        // .mcp.json would be created right in the project: the tripwire's.
        assert!(!plan.aces.iter().any(|a| a.path.ends_with(".mcp.json")));
        assert!(plan.aces.iter().all(|a| a.path != p || a.mask & ADD == ADD));
        // Nothing for the container on a hidden path; a missing one, nothing.
        assert!(cut(&root.join("h/.ssh")) && on(&root.join("h/.ssh")) == 0);
        assert!(!plan.cuts.iter().any(|c| c.ends_with(".aws")));
        // Nothing removes or renames a protected path or a folder above it.
        for a in &plan.aces {
            let above = [p.clone(), p.join(".claude")].contains(&a.path);
            let protected = a.path.starts_with(p.join(".claude/hooks"))
                || a.path == p.join(".claude/settings.json");
            if (above && a.reach == Reach::Itself) || protected {
                assert_eq!(a.mask & (DELETE | FILE_DELETE_CHILD), 0, "{a:?}");
            }
            if protected {
                assert_eq!(a.mask & ADD, 0, "{a:?}");
            }
        }
        assert_eq!(
            plan.labels,
            [
                (p.clone(), Level::Low),
                (root.join("h/.cargo/registry"), Level::Low),
                (p.join(".claude/hooks"), Level::Medium),
                (p.join(".claude/settings.json"), Level::Medium),
            ]
        );
    }

    /// A worktree inside `.claude/` is a writable root of its own: its
    /// protected paths are kept from it, not from the project around it.
    #[test]
    fn a_worktrees_protected_paths_hang_off_the_worktree() {
        let t = tempfile::tempdir().unwrap();
        let p = super::super::policy::real_path(t.path()).join("p");
        let wt = p.join(".claude/worktrees/w1");
        std::fs::create_dir_all(wt.join(".claude/hooks")).unwrap();
        std::fs::create_dir_all(p.join(".claude/hooks")).unwrap();
        let pol = Policy {
            project: p.clone(),
            dir: wt.clone(),
            write: vec![p.clone(), wt.clone()],
            protect: vec![p.join(".claude/hooks"), wt.join(".claude/hooks")],
            ..Policy::default()
        };
        let plan = plan(&pol);
        assert!(plan.cuts.contains(&wt.join(".claude")));
        // Inside the project's grant, the worktree itself is cut off too, or
        // it could be removed or renamed through what it inherits.
        assert!(plan.cuts.contains(&wt));
        assert!(plan
            .aces
            .iter()
            .any(|a| a.path == wt && a.reach == Reach::Itself && a.mask == KEEP));
        // The worktree's own folders, not the project's way down to it.
        assert!(!plan.cuts.contains(&p.join(".claude/worktrees")));
    }
}
