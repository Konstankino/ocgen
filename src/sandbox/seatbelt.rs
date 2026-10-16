//! macOS: a Seatbelt profile that denies everything by default, applied to the
//! helper with `sandbox_init` (the API `sandbox-exec` and other sandboxes use)
//! before it starts the command, which inherits it.
//!
//! The profile is ocgen's own rather than a library's: the common ones allow
//! every Mach service, and through launchd or LaunchServices a sandboxed
//! command could start programs outside the sandbox. Here only the services
//! ordinary tools need are reachable.

use std::path::{Path, PathBuf};

use super::policy::{is_within, spine, Policy};

/// Mach services a command may look up: user and group names, notifications,
/// logging, preferences, certificate checks. Not launchd's job control and not
/// LaunchServices, which would start programs outside the sandbox.
const SERVICES: [&str; 11] = [
    "com.apple.system.opendirectoryd.libinfo",
    "com.apple.system.DirectoryService.libinfo_v1",
    "com.apple.system.notification_center",
    "com.apple.system.logger",
    "com.apple.logd",
    "com.apple.diagnosticd",
    "com.apple.cfprefsd.daemon",
    "com.apple.cfprefsd.agent",
    "com.apple.trustd",
    "com.apple.trustd.agent",
    "com.apple.bsd.dirhelper",
];

/// The services name resolution needs, with the network on.
const NETWORK_SERVICES: [&str; 5] = [
    "com.apple.dnssd.service",
    "com.apple.mDNSResponder",
    "com.apple.SystemConfiguration.configd",
    "com.apple.SystemConfiguration.DNSConfiguration",
    "com.apple.networkd",
];

/// The Seatbelt profile (SBPL) for `p`. Later rules win, so the denials for
/// protected and hidden paths come after the grants that contain them.
pub fn profile(p: &Policy) -> String {
    let mut s = String::from("(version 1)(deny default)");
    s.push_str("(allow process-fork)(allow signal (target same-sandbox))(allow sysctl-read)");
    s.push_str("(allow process-info* (target self))(allow user-preference-read)");
    // Names and sizes along any path (`realpath`, `getcwd`), not contents.
    s.push_str("(allow file-read-metadata)(allow ipc-posix*)(allow system-socket)");
    let services: Vec<&str> = SERVICES
        .iter()
        .chain(if p.network {
            &NETWORK_SERVICES[..]
        } else {
            &[]
        })
        .copied()
        .collect();
    s.push_str(&rule("allow mach-lookup", "global-name", &services));
    let runnable: Vec<&PathBuf> = p.exec.iter().chain(&p.write).collect();
    s.push_str(&rule("allow process-exec", "subpath", &runnable));
    let readable: Vec<&PathBuf> = p.grants().collect();
    s.push_str(&rule("allow file-read*", "subpath", &readable));
    // A granted path that is a symlink: the link itself, read on the way.
    let links: Vec<&PathBuf> = p.links.iter().map(|(l, _)| l).collect();
    s.push_str(&rule("allow file-read*", "literal", &links));
    s.push_str(&rule(
        "allow file-write*",
        "subpath",
        &p.write.iter().collect::<Vec<_>>(),
    ));
    s.push_str(&rule(
        "allow file-ioctl",
        "literal",
        &["/dev/tty", "/dev/dtracehelper"],
    ));
    s.push_str(&rule(
        "allow file-read* file-write*",
        "literal",
        &["/dev/dtracehelper"],
    ));
    if p.network {
        s.push_str("(allow network*)");
    }
    // Protected paths can't be written, created, renamed or removed — nor the
    // folders above them in a writable root, which would move them away.
    let protect: Vec<&PathBuf> = p.protect.iter().collect();
    s.push_str(&rule("deny file-write*", "subpath", &protect));
    let mut above: Vec<PathBuf> = Vec::new();
    for t in &p.protect {
        for w in p.write.iter().filter(|w| is_within(t, w)) {
            for a in spine(t, w) {
                super::policy::push_new(&mut above, a);
            }
        }
    }
    s.push_str(&rule(
        "deny file-write-unlink",
        "literal",
        &above.iter().collect::<Vec<_>>(),
    ));
    s.push_str(&rule(
        "deny file-read*",
        "subpath",
        &p.hide.iter().collect::<Vec<_>>(),
    ));
    s
}

/// `(<action> (<filter> "a") (<filter> "b"))`, or nothing for no paths.
fn rule<T: AsRef<Path>>(action: &str, filter: &str, items: &[T]) -> String {
    if items.is_empty() {
        return String::new();
    }
    let list: String = items
        .iter()
        .map(|i| format!(" ({filter} {})", string(&i.as_ref().to_string_lossy())))
        .collect();
    format!("({action}{list})")
}

/// An SBPL string literal.
fn string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Sandbox this process with `profile`; every process it starts after inherits
/// it. Never undone.
#[cfg(target_os = "macos")]
pub fn apply(profile: &str) -> Result<(), String> {
    use std::ffi::{c_char, c_int, CStr, CString};
    extern "C" {
        fn sandbox_init(profile: *const c_char, flags: u64, errorbuf: *mut *mut c_char) -> c_int;
        fn sandbox_free_error(errorbuf: *mut c_char);
    }
    let text = CString::new(profile).map_err(|_| "a NUL byte in the profile".to_string())?;
    let mut err: *mut c_char = std::ptr::null_mut();
    // SAFETY: a NUL-terminated profile; flags 0 means "profile text"; the error
    // buffer is freed with the call made for it.
    let rc = unsafe { sandbox_init(text.as_ptr(), 0, &mut err) };
    if rc == 0 {
        return Ok(());
    }
    let why = if err.is_null() {
        "unknown error".to_string()
    } else {
        // SAFETY: sandbox_init set a NUL-terminated message.
        let s = unsafe { CStr::from_ptr(err) }
            .to_string_lossy()
            .into_owned();
        unsafe { sandbox_free_error(err) };
        s
    };
    Err(format!("Seatbelt refused the profile: {why}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> Policy {
        Policy {
            project: "/p".into(),
            dir: "/p".into(),
            exec: vec!["/usr".into(), "/h/.cargo/bin".into()],
            read: vec!["/private/etc".into()],
            write: vec!["/p".into(), "/h/.cargo/registry".into()],
            protect: vec![
                "/p/.claude/settings.json".into(),
                "/p/.git/hooks".into(),
                "/p/.mcp.json".into(),
            ],
            hide: vec!["/p/.env\"q".into()],
            ..Policy::default()
        }
    }

    #[test]
    fn the_profile_denies_by_default_and_protects_inside_grants() {
        let s = profile(&policy());
        assert!(s.starts_with("(version 1)(deny default)"), "{s}");
        for want in [
            "(allow process-exec (subpath \"/usr\") (subpath \"/h/.cargo/bin\") (subpath \"/p\") (subpath \"/h/.cargo/registry\"))",
            "(allow file-read* (subpath \"/usr\") (subpath \"/h/.cargo/bin\") (subpath \"/private/etc\") (subpath \"/p\") (subpath \"/h/.cargo/registry\"))",
            "(allow file-write* (subpath \"/p\") (subpath \"/h/.cargo/registry\"))",
            "(deny file-write* (subpath \"/p/.claude/settings.json\") (subpath \"/p/.git/hooks\") (subpath \"/p/.mcp.json\"))",
            "(deny file-write-unlink (literal \"/p\") (literal \"/p/.claude\") (literal \"/p/.git\"))",
            "(deny file-read* (subpath \"/p/.env\\\"q\"))",
            "(global-name \"com.apple.system.opendirectoryd.libinfo\")",
        ] {
            assert!(s.contains(want), "no {want} in {s}");
        }
        // Denials after the grants they cut into.
        assert!(s.find("(deny file-write*").unwrap() > s.find("(allow file-write*").unwrap());
        // No network, no launchd or LaunchServices.
        assert!(!s.contains("network*") && !s.contains("dnssd"), "{s}");
        assert!(
            !s.contains("launchservicesd") && !s.contains("xpc.launchd"),
            "{s}"
        );
        assert!(
            !s.contains("(allow mach*") && !s.contains("(allow default"),
            "{s}"
        );
    }

    #[test]
    fn the_network_opens_sockets_and_name_resolution() {
        let mut p = policy();
        p.network = true;
        let s = profile(&p);
        assert!(
            s.contains("(allow network*)") && s.contains("com.apple.dnssd.service"),
            "{s}"
        );
    }
}
