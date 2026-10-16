//! Linux without usable namespaces: Landlock (kernel 6.2 and later) for the
//! files, and a seccomp filter for the network, applied to the helper before
//! it starts the command, which inherits both.
//!
//! Landlock grants whole folders and can't take a path inside a grant back,
//! so the writable roots are [`split`] around the protected paths: nothing new
//! can be created right in a folder that holds one (the project's top level,
//! `.git/`, `.claude/`), and a hidden path inside a grant can't be hidden.

use std::path::PathBuf;

use super::policy::{list_dir, split, Policy};

/// The Landlock ABI the kernel speaks: 3 or later (Linux 6.2) is needed, so
/// truncating a file is a right the sandbox controls.
pub fn abi() -> Result<i32, String> {
    // SAFETY: the version query takes no ruleset (NULL, 0) and only returns a
    // number or -1.
    let v = unsafe {
        libc::syscall(
            libc::SYS_landlock_create_ruleset,
            std::ptr::null::<libc::c_void>(),
            0usize,
            1u32, // LANDLOCK_CREATE_RULESET_VERSION
        )
    };
    match v {
        v if v < 0 => Err("Landlock isn't enabled in this kernel".into()),
        v if v < 3 => Err(format!(
            "the kernel's Landlock ABI is {v}, and 3 (Linux 6.2) is needed"
        )),
        v => Ok(v as i32),
    }
}

/// Whether Landlock can enforce `p`: nothing hidden inside a grant.
pub fn admits(p: &Policy) -> Result<(), String> {
    match p.hide.first() {
        Some(h) => Err(format!(
            "Landlock can't hide {} inside a folder it grants",
            h.display()
        )),
        None => Ok(()),
    }
}

/// Sandbox this process (and what it starts) with `p`.
pub fn apply(p: &Policy) -> Result<(), String> {
    use landlock::{
        make_bitflags, path_beneath_rules, Access, AccessFs, CompatLevel, Compatible, Ruleset,
        RulesetAttr, RulesetCreatedAttr, RulesetStatus, ABI,
    };
    abi()?;
    let abi = ABI::V3;
    let s = split(&p.write, &p.protect, &list_dir);
    let read = make_bitflags!(AccessFs::{ReadFile | ReadDir});
    let err = |e: landlock::RulesetError| format!("Landlock: {e}");
    let status = Ruleset::default()
        .set_compatibility(CompatLevel::HardRequirement)
        .handle_access(AccessFs::from_all(abi))
        .map_err(err)?
        .create()
        .map_err(err)?
        .add_rules(path_beneath_rules(&p.exec, AccessFs::from_read(abi)))
        .map_err(err)?
        .add_rules(path_beneath_rules(&p.read, read))
        .map_err(err)?
        .add_rules(path_beneath_rules(&s.spine, AccessFs::from_read(abi)))
        .map_err(err)?
        .add_rules(path_beneath_rules(&s.write, AccessFs::from_all(abi)))
        .map_err(err)?
        .restrict_self()
        .map_err(err)?;
    if status.ruleset != RulesetStatus::FullyEnforced {
        return Err("Landlock enforced the rules only in part".into());
    }
    if !p.network {
        deny_network()?;
    }
    Ok(())
}

/// No new internet sockets (IPv4, IPv6): `socket()` fails with EACCES. Local
/// sockets stay, for the IPC tools use.
fn deny_network() -> Result<(), String> {
    use seccompiler::{
        BpfProgram, SeccompAction, SeccompCmpArgLen, SeccompCmpOp, SeccompCondition, SeccompFilter,
        SeccompRule,
    };
    fn err<E: std::fmt::Display>(e: E) -> String {
        format!("seccomp: {e}")
    }
    let family = |f: i32| -> Result<SeccompRule, String> {
        let c = SeccompCondition::new(0, SeccompCmpArgLen::Dword, SeccompCmpOp::Eq, f as u64)
            .map_err(err)?;
        SeccompRule::new(vec![c]).map_err(err)
    };
    let rules = [(
        libc::SYS_socket,
        vec![family(libc::AF_INET)?, family(libc::AF_INET6)?],
    )]
    .into_iter()
    .collect();
    let filter = SeccompFilter::new(
        rules,
        SeccompAction::Allow,
        SeccompAction::Errno(libc::EACCES as u32),
        std::env::consts::ARCH.try_into().map_err(err)?,
    )
    .map_err(err)?;
    let prog: BpfProgram = filter.try_into().map_err(err)?;
    seccompiler::apply_filter(&prog).map_err(err)
}

/// What [`apply`] would grant, for `ocgen sandbox status`.
pub fn describe(p: &Policy) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let s = split(&p.write, &p.protect, &list_dir);
    (s.write, s.spine)
}
