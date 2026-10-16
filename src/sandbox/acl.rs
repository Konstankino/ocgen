//! Native Windows: what the AppContainer is granted and denied, as access
//! control entries on the files themselves. An AppContainer can open only what
//! an entry names it (or a capability it holds) in: one per project (the
//! "package"), plus one capability every project's container shares for the
//! toolchains and caches, so those big trees are granted once per machine.
//! A deny entry wins over the allow a folder's children inherit, so the
//! protected paths inside the project can be carved out natively.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::policy::{is_within, push_new, same, spine, Policy};

// Access rights (winnt.h).
const FILE_READ_DATA: u32 = 0x1;
const FILE_WRITE_DATA: u32 = 0x2; // FILE_ADD_FILE on a folder
const FILE_APPEND_DATA: u32 = 0x4; // FILE_ADD_SUBDIRECTORY on a folder
const FILE_READ_EA: u32 = 0x8;
const FILE_WRITE_EA: u32 = 0x10;
const FILE_EXECUTE: u32 = 0x20;
const FILE_DELETE_CHILD: u32 = 0x40;
const FILE_WRITE_ATTRIBUTES: u32 = 0x100;
const DELETE: u32 = 0x1_0000;
const WRITE_DAC: u32 = 0x4_0000;
const WRITE_OWNER: u32 = 0x8_0000;

/// Read and run (FILE_GENERIC_READ | FILE_GENERIC_EXECUTE).
pub const READ_RUN: u32 = 0x1200A9;
/// Read, run, write, delete (no changing permissions or owner).
pub const MODIFY: u32 = READ_RUN | 0x120116 | DELETE | FILE_DELETE_CHILD;
/// Every way to change a protected path, or what's in it.
pub const PROTECT: u32 = FILE_WRITE_DATA
    | FILE_APPEND_DATA
    | FILE_WRITE_EA
    | FILE_WRITE_ATTRIBUTES
    | FILE_DELETE_CHILD
    | DELETE
    | WRITE_DAC
    | WRITE_OWNER;
/// Reading a hidden path's contents.
pub const HIDE: u32 = FILE_READ_DATA | FILE_READ_EA | FILE_EXECUTE;
/// Creating an entry in a folder.
pub const ADD: u32 = FILE_WRITE_DATA | FILE_APPEND_DATA;

/// Whose entry: the project's container, or the capability every project's
/// container holds (toolchains and caches).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Who {
    Package,
    Toolchain,
}

/// One access control entry to have on `path`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ace {
    pub path: PathBuf,
    pub who: Who,
    pub deny: bool,
    pub mask: u32,
    /// Inherited by everything below (else on this object only).
    pub inherit: bool,
}

/// The mandatory label a path gets: an AppContainer runs at Low integrity and
/// can't write to anything labelled higher (Medium, the default), so its
/// writable roots are labelled Low, and protected paths inside them Medium
/// again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Low,
    Medium,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Plan {
    pub aces: Vec<Ace>,
    pub labels: Vec<(PathBuf, Level)>,
}

/// The entries and labels that give `p`'s command what it may have.
pub fn plan(p: &Policy) -> Plan {
    let mut out = Plan::default();
    let mut add = |path: &Path, who, deny, mask, inherit| {
        push_new(
            &mut out.aces,
            Ace {
                path: path.to_path_buf(),
                who,
                deny,
                mask,
                inherit,
            },
        )
    };
    let ours = |w: &Path| same(w, &p.project) || same(w, &p.dir);
    for g in p.exec.iter().chain(&p.read) {
        add(g, Who::Toolchain, false, READ_RUN, true);
    }
    let mut labels = Vec::new();
    for w in &p.write {
        let who = if ours(w) {
            Who::Package
        } else {
            Who::Toolchain
        };
        add(w, who, false, MODIFY, true);
        push_new(&mut labels, (w.clone(), Level::Low));
    }
    // Protected paths: no change to them; their folder can't delete them, nor
    // the folders above them be renamed or removed (which would move them).
    let guard = |t: &Path, add: &mut dyn FnMut(&Path, Who, bool, u32, bool)| {
        for w in p.write.iter().filter(|w| is_within(t, w)) {
            for d in spine(t, w) {
                add(&d, Who::Package, true, FILE_DELETE_CHILD | DELETE, false);
            }
        }
    };
    for t in p.protect.iter().filter(|t| t.exists()) {
        add(t, Who::Package, true, PROTECT, true);
        push_new(&mut labels, (t.clone(), Level::Medium));
        guard(t, &mut add);
    }
    // A missing one can't be created: nothing new in its nearest existing
    // folder — unless that is a writable root, where the tripwire catches it.
    for (_, parent) in p.missing_protected() {
        if p.write.iter().any(|w| same(w, &parent)) {
            continue;
        }
        add(&parent, Who::Package, true, ADD, false);
        guard(&parent, &mut add);
    }
    for h in p.hide.iter().filter(|h| h.exists()) {
        add(h, Who::Package, true, HIDE, true);
    }
    out.labels = labels;
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_plan_grants_roots_and_carves_out_protected_paths() {
        let t = tempfile::tempdir().unwrap();
        let root = super::super::policy::real_path(t.path());
        let p = root.join("p");
        for d in [".claude/hooks", ".git/hooks", "src"] {
            std::fs::create_dir_all(p.join(d)).unwrap();
        }
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
                p.join(".claude/settings.local.json"),
                p.join(".mcp.json"),
            ],
            hide: vec![root.join("h/.ssh"), root.join("h/.aws")],
            ..Policy::default()
        };
        let plan = plan(&pol);
        let has = |path: &Path, who, deny, mask, inherit| {
            plan.aces.contains(&Ace {
                path: path.to_path_buf(),
                who,
                deny,
                mask,
                inherit,
            })
        };
        assert!(has(&p, Who::Package, false, MODIFY, true));
        assert!(has(
            &root.join("h/.cargo/registry"),
            Who::Toolchain,
            false,
            MODIFY,
            true
        ));
        assert!(has(&root.join("h"), Who::Toolchain, false, READ_RUN, true));
        assert!(has(
            &p.join(".claude/hooks"),
            Who::Package,
            true,
            PROTECT,
            true
        ));
        // Its folders: no deleting or renaming it, or them.
        assert!(has(
            &p,
            Who::Package,
            true,
            FILE_DELETE_CHILD | DELETE,
            false
        ));
        assert!(has(
            &p.join(".claude"),
            Who::Package,
            true,
            FILE_DELETE_CHILD | DELETE,
            false
        ));
        // settings.local.json is missing in .claude: nothing new there.
        assert!(has(&p.join(".claude"), Who::Package, true, ADD, false));
        // .mcp.json would be created right in the project: the tripwire's.
        assert!(!plan.aces.iter().any(|a| a.path.ends_with(".mcp.json")));
        assert!(!plan.aces.iter().any(|a| a.path == p && a.mask == ADD));
        assert!(has(&root.join("h/.ssh"), Who::Package, true, HIDE, true));
        assert!(!plan.aces.iter().any(|a| a.path.ends_with(".aws")));
        assert_eq!(
            plan.labels,
            [
                (p.clone(), Level::Low),
                (root.join("h/.cargo/registry"), Level::Low),
                (p.join(".claude/hooks"), Level::Medium),
            ]
        );
        // Allows before denies is the applier's business; a deny never grants.
        assert!(plan
            .aces
            .iter()
            .all(|a| !a.deny || a.mask & MODIFY != MODIFY));
    }
}
