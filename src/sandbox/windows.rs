//! Native Windows (no WSL): the command runs in an AppContainer — an identity
//! that can open only what an access control entry names it in ([`acl`]) —
//! inside a job object that ends every process it starts with it.
//!
//! The entries are put on the files once and kept (a big tree takes a while
//! to update), recorded in `%LOCALAPPDATA%\ocgen\sandbox\` so `ocgen sandbox
//! reset` can take them off again.
//!
//! The command runs as one program, without a shell ([`cmdline::split`]): Git
//! Bash can't be trusted in an AppContainer ([`NO_GIT_BASH`]). A command Claude
//! runs in PowerShell ([`run_agent`]) runs in PowerShell, refused when that
//! can't start here; one it runs in Bash is refused.

use std::ffi::{c_void, OsStr, OsString};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};

use serde::{Deserialize, Serialize};
use windows_sys::Win32::Foundation::{
    CloseHandle, LocalFree, SetHandleInformation, HANDLE, HANDLE_FLAG_INHERIT,
    INVALID_HANDLE_VALUE, WAIT_OBJECT_0,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
    GetNamedSecurityInfoW, SetEntriesInAclW, SetNamedSecurityInfoW, DENY_ACCESS, EXPLICIT_ACCESS_W,
    GRANT_ACCESS, NO_MULTIPLE_TRUSTEE, SDDL_REVISION_1, SE_FILE_OBJECT, TRUSTEE_IS_SID,
    TRUSTEE_IS_UNKNOWN, TRUSTEE_W,
};
use windows_sys::Win32::Security::Isolation::{
    CreateAppContainerProfile, DeleteAppContainerProfile,
    DeriveAppContainerSidFromAppContainerName, GetAppContainerFolderPath,
};
use windows_sys::Win32::Security::{
    AclSizeInformation, AddAce, CreateWellKnownSid, DeriveCapabilitySidsFromName, EqualSid,
    FreeSid, GetAce, GetAclInformation, GetLengthSid, GetSecurityDescriptorSacl,
    GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, InitializeAcl,
    TokenAppContainerSid, TokenIntegrityLevel, TokenIsAppContainer,
    WinCapabilityInternetClientServerSid, WinCapabilityInternetClientSid,
    WinCapabilityPrivateNetworkClientServerSid, ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, ACL_REVISION,
    ACL_SIZE_INFORMATION, CONTAINER_INHERIT_ACE, DACL_SECURITY_INFORMATION, INHERITED_ACE,
    LABEL_SECURITY_INFORMATION, NO_INHERITANCE, OBJECT_INHERIT_ACE, PSID, SECURITY_CAPABILITIES,
    SID_AND_ATTRIBUTES, TOKEN_APPCONTAINER_INFORMATION, TOKEN_INFORMATION_CLASS,
    TOKEN_MANDATORY_LABEL, TOKEN_QUERY, WELL_KNOWN_SID_TYPE,
};
use windows_sys::Win32::System::Com::CoTaskMemFree;
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, IsProcessInJob, JobObjectBasicUIRestrictions,
    JobObjectExtendedLimitInformation, SetInformationJobObject, TerminateJobObject,
    JOBOBJECT_BASIC_UI_RESTRICTIONS, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOB_OBJECT_UILIMIT_DESKTOP, JOB_OBJECT_UILIMIT_DISPLAYSETTINGS, JOB_OBJECT_UILIMIT_EXITWINDOWS,
    JOB_OBJECT_UILIMIT_GLOBALATOMS, JOB_OBJECT_UILIMIT_HANDLES, JOB_OBJECT_UILIMIT_READCLIPBOARD,
    JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS, JOB_OBJECT_UILIMIT_WRITECLIPBOARD,
};
use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD};
use windows_sys::Win32::System::Threading::{
    CreateProcessW, DeleteProcThreadAttributeList, GetCurrentProcess, GetExitCodeProcess,
    InitializeProcThreadAttributeList, OpenProcessToken, ResumeThread, TerminateProcess,
    UpdateProcThreadAttribute, WaitForSingleObject, CREATE_NO_WINDOW, CREATE_SUSPENDED,
    CREATE_UNICODE_ENVIRONMENT, EXTENDED_STARTUPINFO_PRESENT, INFINITE, PROCESS_INFORMATION,
    PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES,
    STARTF_USESTDHANDLES, STARTUPINFOEXW,
};

use super::acl::{self, Ace, Level, Who};
use super::cmdline;
use super::policy::{is_within, Policy};
use super::shell::Shell;

/// The capability every project's container holds, for the toolchains and
/// caches granted once per machine.
const TOOLCHAIN_CAPABILITY: &str = "ocgenSandboxToolchain";
/// `HRESULT_FROM_WIN32(ERROR_ALREADY_EXISTS)`.
const ALREADY_EXISTS: i32 = 0x8007_00B7_u32 as i32;
const SE_GROUP_ENABLED: u32 = 4;
const ALLOWED: u8 = 0;
const DENIED: u8 = 1;
const MANDATORY_LABEL: u8 = 0x11;
/// The Low integrity level's RID (`S-1-16-4096`), an AppContainer's.
const LOW_RID: u32 = 0x1000;

fn wide(s: impl AsRef<OsStr>) -> Vec<u16> {
    s.as_ref().encode_wide().chain([0]).collect()
}

fn last_error() -> String {
    std::io::Error::last_os_error().to_string()
}

/// The AppContainer profile for `project`: `ocgen.sandbox.` and a hash of its
/// path (case-insensitive, as Windows compares paths).
pub fn profile_name(project: &Path) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in crate::paths::plain(project)
        .to_string_lossy()
        .to_lowercase()
        .bytes()
    {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("ocgen.sandbox.{h:016x}")
}

/// A SID, copied out of whatever Windows allocated it in.
pub struct Sid(Vec<u8>);

impl Sid {
    /// # Safety
    /// `p` points to a valid SID.
    unsafe fn copy(p: PSID) -> Self {
        let n = GetLengthSid(p) as usize;
        Sid(std::slice::from_raw_parts(p as *const u8, n).to_vec())
    }
    fn ptr(&self) -> PSID {
        self.0.as_ptr() as PSID
    }
    fn string(&self) -> String {
        let mut s: *mut u16 = null_mut();
        // SAFETY: a valid SID; the string is freed with LocalFree.
        unsafe {
            if ConvertSidToStringSidW(self.ptr(), &mut s) == 0 {
                return String::new();
            }
            let n = (0..).take_while(|&i| *s.add(i) != 0).count();
            let out = String::from_utf16_lossy(std::slice::from_raw_parts(s, n));
            LocalFree(s as _);
            out
        }
    }
}

/// The project's container SID, creating its profile the first time.
fn package_sid(name: &str) -> Result<Sid, String> {
    let n = wide(name);
    let about = wide("ocgen: a hook's sandboxed check or formatter");
    let mut sid: PSID = null_mut();
    // SAFETY: NUL-terminated strings; the SID is copied, then freed.
    unsafe {
        let mut hr =
            CreateAppContainerProfile(n.as_ptr(), n.as_ptr(), about.as_ptr(), null(), 0, &mut sid);
        if hr == ALREADY_EXISTS {
            hr = DeriveAppContainerSidFromAppContainerName(n.as_ptr(), &mut sid);
        }
        if hr < 0 || sid.is_null() {
            return Err(format!(
                "can't make the AppContainer profile {name} (0x{hr:08x})"
            ));
        }
        let out = Sid::copy(sid);
        FreeSid(sid);
        Ok(out)
    }
}

/// The toolchain capability's SID.
fn toolchain_sid() -> Result<Sid, String> {
    let n = wide(TOOLCHAIN_CAPABILITY);
    let (mut groups, mut ng, mut sids, mut ns) = (null_mut(), 0u32, null_mut(), 0u32);
    // SAFETY: Windows allocates both arrays and each SID with LocalAlloc.
    unsafe {
        if DeriveCapabilitySidsFromName(n.as_ptr(), &mut groups, &mut ng, &mut sids, &mut ns) == 0
            || ns == 0
        {
            return Err(format!(
                "can't derive the capability {TOOLCHAIN_CAPABILITY}: {}",
                last_error()
            ));
        }
        let out = Sid::copy(*sids);
        for (arr, count) in [(groups, ng), (sids, ns)] {
            for i in 0..count as usize {
                LocalFree(*arr.add(i) as _);
            }
            LocalFree(arr as _);
        }
        Ok(out)
    }
}

fn well_known(kind: WELL_KNOWN_SID_TYPE) -> Result<Sid, String> {
    let mut buf = vec![0u8; 68];
    let mut n = buf.len() as u32;
    // SAFETY: a buffer of SECURITY_MAX_SID_SIZE bytes.
    if unsafe { CreateWellKnownSid(kind, null_mut(), buf.as_mut_ptr() as PSID, &mut n) } == 0 {
        return Err(format!("can't make a capability SID: {}", last_error()));
    }
    buf.truncate(n as usize);
    Ok(Sid(buf))
}

// ------------------------------------------------------------- entries --

/// The explicit entries in a DACL: (allowed or denied, mask, flags, SID bytes).
///
/// # Safety
/// `acl` is null or a valid ACL.
unsafe fn explicit_aces(acl: *const ACL) -> Vec<(u8, u32, u8, Vec<u8>)> {
    let mut out = Vec::new();
    if acl.is_null() {
        return out;
    }
    let mut info: ACL_SIZE_INFORMATION = std::mem::zeroed();
    if GetAclInformation(
        acl,
        &mut info as *mut _ as *mut c_void,
        std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
        AclSizeInformation,
    ) == 0
    {
        return out;
    }
    for i in 0..info.AceCount {
        let mut p: *mut c_void = null_mut();
        if GetAce(acl, i, &mut p) == 0 {
            continue;
        }
        let h = &*(p as *const ACE_HEADER);
        if h.AceFlags as u32 & INHERITED_ACE != 0
            || !matches!(h.AceType, ALLOWED | DENIED | MANDATORY_LABEL)
        {
            continue;
        }
        let a = &*(p as *const ACCESS_ALLOWED_ACE);
        let sid = Sid::copy(&a.SidStart as *const u32 as PSID);
        out.push((h.AceType, a.Mask, h.AceFlags, sid.0));
    }
    out
}

/// Make sure `ace` is on its path for `sid`; true when it had to be added.
fn ensure(ace: &Ace, sid: &Sid) -> Result<bool, String> {
    let path = wide(&ace.path);
    let inherit = ace.inherit && ace.path.is_dir();
    // SAFETY: plain Win32 security calls; everything they allocate is freed.
    unsafe {
        let (mut dacl, mut sd): (*mut ACL, *mut c_void) = (null_mut(), null_mut());
        let rc = GetNamedSecurityInfoW(
            path.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            &mut dacl,
            null_mut(),
            &mut sd,
        );
        if rc != 0 {
            return Err(format!(
                "can't read the permissions of {}: error {rc}",
                ace.path.display()
            ));
        }
        let kind = if ace.deny { DENIED } else { ALLOWED };
        let has = explicit_aces(dacl).iter().any(|(t, m, _, s)| {
            *t == kind && m & ace.mask == ace.mask && EqualSid(s.as_ptr() as PSID, sid.ptr()) != 0
        });
        if has {
            LocalFree(sd);
            return Ok(false);
        }
        let ea = EXPLICIT_ACCESS_W {
            grfAccessPermissions: ace.mask,
            grfAccessMode: if ace.deny { DENY_ACCESS } else { GRANT_ACCESS },
            grfInheritance: if inherit {
                OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE
            } else {
                NO_INHERITANCE
            },
            Trustee: TRUSTEE_W {
                pMultipleTrustee: null_mut(),
                MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
                TrusteeForm: TRUSTEE_IS_SID,
                TrusteeType: TRUSTEE_IS_UNKNOWN,
                ptstrName: sid.ptr() as *mut u16,
            },
        };
        let mut new: *mut ACL = null_mut();
        let rc = SetEntriesInAclW(1, &ea, dacl, &mut new);
        LocalFree(sd);
        if rc != 0 {
            return Err(format!(
                "can't build the permissions of {}: error {rc}",
                ace.path.display()
            ));
        }
        let rc = SetNamedSecurityInfoW(
            path.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            new,
            null(),
        );
        LocalFree(new as _);
        if rc != 0 {
            return Err(format!(
                "can't set the permissions of {}: error {rc}",
                ace.path.display()
            ));
        }
    }
    Ok(true)
}

/// Make sure `path` carries `level` as its own mandatory label.
fn ensure_label(path: &Path, level: Level) -> Result<bool, String> {
    let rid: u32 = match level {
        Level::Low => 0x1000,
        Level::Medium => 0x2000,
    };
    let w = wide(path);
    // SAFETY: plain Win32 security calls; everything they allocate is freed.
    unsafe {
        let (mut sacl, mut sd): (*mut ACL, *mut c_void) = (null_mut(), null_mut());
        if GetNamedSecurityInfoW(
            w.as_ptr(),
            SE_FILE_OBJECT,
            LABEL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            null_mut(),
            &mut sacl,
            &mut sd,
        ) == 0
        {
            let has = explicit_aces(sacl).iter().any(|(t, _, _, s)| {
                *t == MANDATORY_LABEL
                    && s.len() >= 4
                    && u32::from_le_bytes(s[s.len() - 4..].try_into().unwrap_or([0; 4])) == rid
            });
            LocalFree(sd);
            if has {
                return Ok(false);
            }
        }
        let flags = if path.is_dir() { "OICI" } else { "" };
        let who = if level == Level::Low { "LW" } else { "ME" };
        let sddl = wide(format!("S:(ML;{flags};NW;;;{who})"));
        let mut label_sd: *mut c_void = null_mut();
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut label_sd,
            null_mut(),
        ) == 0
        {
            return Err(format!("can't make a label: {}", last_error()));
        }
        let (mut present, mut defaulted, mut label): (i32, i32, *mut ACL) = (0, 0, null_mut());
        GetSecurityDescriptorSacl(label_sd, &mut present, &mut label, &mut defaulted);
        let rc = SetNamedSecurityInfoW(
            w.as_ptr(),
            SE_FILE_OBJECT,
            LABEL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            null(),
            label,
        );
        LocalFree(label_sd);
        if rc != 0 {
            return Err(format!("can't label {}: error {rc}", path.display()));
        }
    }
    Ok(true)
}

/// Take every explicit entry for `sids` off `path`.
fn remove(path: &Path, sids: &[&Sid]) -> Result<(), String> {
    let w = wide(path);
    // SAFETY: the new ACL is built in a buffer as big as the old one.
    unsafe {
        let (mut dacl, mut sd): (*mut ACL, *mut c_void) = (null_mut(), null_mut());
        if GetNamedSecurityInfoW(
            w.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            &mut dacl,
            null_mut(),
            &mut sd,
        ) != 0
            || dacl.is_null()
        {
            return Ok(());
        }
        let mut info: ACL_SIZE_INFORMATION = std::mem::zeroed();
        GetAclInformation(
            dacl,
            &mut info as *mut _ as *mut c_void,
            std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
            AclSizeInformation,
        );
        let mut buf = vec![0u64; (info.AclBytesInUse as usize).div_ceil(8) + 1];
        let new = buf.as_mut_ptr() as *mut ACL;
        InitializeAcl(new, (buf.len() * 8) as u32, ACL_REVISION);
        let mut dropped = false;
        for i in 0..info.AceCount {
            let mut p: *mut c_void = null_mut();
            if GetAce(dacl, i, &mut p) == 0 {
                continue;
            }
            let h = &*(p as *const ACE_HEADER);
            let a = &*(p as *const ACCESS_ALLOWED_ACE);
            let ours = h.AceFlags as u32 & INHERITED_ACE == 0
                && matches!(h.AceType, ALLOWED | DENIED)
                && sids
                    .iter()
                    .any(|s| EqualSid(&a.SidStart as *const u32 as PSID, s.ptr()) != 0);
            if ours {
                dropped = true;
            } else {
                AddAce(new, ACL_REVISION, u32::MAX, p, h.AceSize as u32);
            }
        }
        LocalFree(sd);
        if dropped {
            let rc = SetNamedSecurityInfoW(
                w.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                new,
                null(),
            );
            if rc != 0 {
                return Err(format!(
                    "can't reset the permissions of {}: error {rc}",
                    path.display()
                ));
            }
        }
    }
    Ok(())
}

// ------------------------------------------------------------ manifest --

/// Why nothing runs through Git Bash in an AppContainer: its runtime needs a
/// shared object folder an AppContainer may not create (`0xC0000022`), and
/// a command that dies of it may still end with 0 — run fine once, then not —
/// so a check would pass without having run, and a command Claude runs would
/// seem to have run.
pub const NO_GIT_BASH: &str = "Git Bash can't run reliably in an AppContainer (its runtime needs a shared object folder a container may not create, and a command that dies of it can still end with 0), so ocgen can't sandbox Bash commands on native Windows, and it never runs them unsandboxed. Use the PowerShell tool (CLAUDE_CODE_USE_POWERSHELL_TOOL=1): ocgen sandboxes its commands";

/// What a shell's start-up check runs (`exit 7`) must end with. A shell that
/// dies while starting can still end with 0 (Git Bash's runtime can't start
/// in an AppContainer, and its launcher reports success), so success alone
/// doesn't show it ran.
const PROBE_EXIT: u32 = 7;

/// The start-up check's version: shells found to start by an older check (one
/// that took any 0 for success) are checked again.
const PROBE_VERSION: u32 = 2;

/// What ocgen put on the files for one project's container.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Manifest {
    pub project: PathBuf,
    pub profile: String,
    /// Paths with an entry for the project's container.
    pub package: Vec<PathBuf>,
    /// Paths with an entry for the shared toolchain capability.
    pub toolchain: Vec<PathBuf>,
    /// Paths labelled Low or Medium.
    pub labels: Vec<PathBuf>,
    /// Whether Git Bash's `sh` (path, modified time) starts in a container.
    pub shell: Option<(PathBuf, u64, bool)>,
    /// The same for Git Bash's `bash`, which runs the commands Claude runs.
    #[serde(default)]
    pub bash: Option<(PathBuf, u64, bool)>,
    /// The same for PowerShell.
    #[serde(default)]
    pub powershell: Option<(PathBuf, u64, bool)>,
    /// The start-up check that found them ([`PROBE_VERSION`]).
    #[serde(default)]
    pub probe: u32,
}

impl Manifest {
    /// Forget what an older start-up check found.
    fn current(mut self) -> Self {
        if self.probe != PROBE_VERSION {
            self.shell = None;
            self.bash = None;
            self.powershell = None;
            self.probe = PROBE_VERSION;
        }
        self
    }
}

pub fn manifest_dir() -> Option<PathBuf> {
    dirs::data_local_dir().map(|d| d.join("ocgen").join("sandbox"))
}

fn manifest_path(profile: &str) -> Option<PathBuf> {
    manifest_dir().map(|d| d.join(format!("{profile}.json")))
}

fn load(profile: &str) -> Manifest {
    manifest_path(profile)
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str::<Manifest>(&s).ok())
        .unwrap_or_default()
        .current()
}

fn save(m: &Manifest) {
    if let Some(p) = manifest_path(&m.profile) {
        let _ = std::fs::create_dir_all(p.parent().unwrap_or(Path::new(".")));
        let _ = std::fs::write(&p, serde_json::to_string_pretty(m).unwrap_or_default());
    }
}

/// Hold the machine-wide lock while permissions change (hooks run at once).
fn lock() -> Option<std::fs::File> {
    let d = manifest_dir()?;
    std::fs::create_dir_all(&d).ok()?;
    let f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(d.join(".lock"))
        .ok()?;
    f.lock().ok()?;
    Some(f)
}

/// Put `p`'s entries and labels on the files (what is there already stays).
pub fn grant(p: &Policy) -> Result<(Sid, Sid, Manifest), String> {
    let profile = profile_name(&p.project);
    let package = package_sid(&profile)?;
    let toolchain = toolchain_sid()?;
    let _lock = lock();
    let mut m = load(&profile);
    m.project = p.project.clone();
    m.profile = profile;
    let plan = acl::plan(p);
    // Allows first: a deny on a folder must not stop the allow above it from
    // reaching the rest of the folder.
    let mut aces: Vec<&Ace> = plan.aces.iter().filter(|a| !a.deny).collect();
    aces.extend(plan.aces.iter().filter(|a| a.deny));
    for a in aces {
        let sid = match a.who {
            Who::Package => &package,
            Who::Toolchain => &toolchain,
        };
        ensure(a, sid)?;
        let list = match a.who {
            Who::Package => &mut m.package,
            Who::Toolchain => &mut m.toolchain,
        };
        super::policy::push_new(list, a.path.clone());
    }
    for (path, level) in &plan.labels {
        ensure_label(path, *level)?;
        super::policy::push_new(&mut m.labels, path.clone());
    }
    save(&m);
    Ok((package, toolchain, m))
}

/// Take ocgen's entries off the files for `project`'s container and delete
/// its profile — and with `all`, every project's and the toolchain grants.
pub fn reset(project: Option<&Path>) -> Result<Vec<String>, String> {
    let _lock = lock();
    let mut done = Vec::new();
    let manifests: Vec<Manifest> = match project {
        Some(p) => vec![load(&profile_name(p))],
        None => manifest_dir()
            .and_then(|d| std::fs::read_dir(d).ok())
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .filter_map(|e| std::fs::read_to_string(e.path()).ok())
            .filter_map(|s| serde_json::from_str(&s).ok())
            .collect(),
    };
    let toolchain = toolchain_sid()?;
    for m in manifests.iter().filter(|m| !m.profile.is_empty()) {
        let package = package_sid(&m.profile)?;
        for path in &m.package {
            remove(path, &[&package])?;
        }
        if project.is_none() {
            for path in &m.toolchain {
                remove(path, &[&toolchain])?;
            }
        }
        let n = wide(&m.profile);
        // SAFETY: a NUL-terminated name.
        unsafe { DeleteAppContainerProfile(n.as_ptr()) };
        if let Some(f) = manifest_path(&m.profile) {
            let _ = std::fs::remove_file(f);
        }
        done.push(format!("{} ({})", m.project.display(), m.profile));
    }
    // Labels stay: Low on a project folder only lets other Low processes write
    // there, and taking it off could undo a label set by someone else.
    Ok(done)
}

// ------------------------------------------------------------ checking --

/// Why ocgen doesn't use an AppContainer on a machine with UAC off.
pub const UAC_OFF: &str = "UAC is off here (EnableLUA is 0 in HKLM\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Policies\\System), and ocgen doesn't rely on an AppContainer without it: turn UAC on to sandbox commands here";

/// Whether UAC is on: `EnableLUA` isn't 0 (a missing value means on).
pub fn uac_on() -> bool {
    let key = wide(r"SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System");
    let name = wide("EnableLUA");
    let mut value: u32 = 1;
    let mut size = std::mem::size_of::<u32>() as u32;
    // SAFETY: a DWORD value read into a DWORD of the size given.
    let rc = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_DWORD,
            null_mut(),
            &mut value as *mut u32 as *mut c_void,
            &mut size,
        )
    };
    rc != 0 || value != 0
}

/// Whether an AppContainer can sandbox commands here, and if not, why.
pub fn probe() -> Result<(), String> {
    if uac_on() {
        Ok(())
    } else {
        Err(UAC_OFF.to_string())
    }
}

/// Whether a started process is contained as asked: in an AppContainer, the
/// project's, at Low integrity or below — and if not, why.
fn contained(in_container: bool, ours: bool, rid: Option<u32>) -> Result<(), String> {
    if !in_container {
        return Err("it isn't in an AppContainer".into());
    }
    if !ours {
        return Err("it's in another AppContainer than the project's".into());
    }
    match rid {
        Some(r) if r <= LOW_RID => Ok(()),
        Some(r) => Err(format!("it runs at integrity level {r:#x}, above Low")),
        None => Err("its integrity level can't be read".into()),
    }
}

/// One kind of information about `token`, in a buffer aligned for it.
///
/// # Safety
/// `token` is a token handle opened with `TOKEN_QUERY`.
unsafe fn token_info(token: HANDLE, class: TOKEN_INFORMATION_CLASS) -> Option<Vec<u64>> {
    let mut n = 0u32;
    GetTokenInformation(token, class, null_mut(), 0, &mut n);
    if n == 0 {
        return None;
    }
    let mut buf = vec![0u64; (n as usize).div_ceil(8)];
    let ok = GetTokenInformation(token, class, buf.as_mut_ptr() as *mut c_void, n, &mut n) != 0;
    ok.then_some(buf)
}

/// Whether `process` (started, not yet running) runs in `package` at Low
/// integrity, as its token says — and if not, why.
///
/// # Safety
/// `process` is a process handle with `PROCESS_QUERY_INFORMATION` access.
unsafe fn check_token(process: HANDLE, package: &Sid) -> Result<(), String> {
    let mut token: HANDLE = null_mut();
    if OpenProcessToken(process, TOKEN_QUERY, &mut token) == 0 {
        return Err(format!("its token can't be read: {}", last_error()));
    }
    let token = Owned(token);
    let in_container =
        token_info(token.0, TokenIsAppContainer).is_some_and(|b| *(b.as_ptr() as *const u32) != 0);
    let ours = in_container
        && token_info(token.0, TokenAppContainerSid).is_some_and(|b| {
            let sid = (*(b.as_ptr() as *const TOKEN_APPCONTAINER_INFORMATION)).TokenAppContainer;
            !sid.is_null() && EqualSid(sid, package.ptr()) != 0
        });
    let rid = token_info(token.0, TokenIntegrityLevel).and_then(|b| {
        let sid = (*(b.as_ptr() as *const TOKEN_MANDATORY_LABEL)).Label.Sid;
        if sid.is_null() {
            return None;
        }
        let count = *GetSidSubAuthorityCount(sid);
        (count > 0).then(|| *GetSidSubAuthority(sid, u32::from(count) - 1))
    });
    contained(in_container, ours, rid)
}

// ------------------------------------------------------------- running --

/// Close a handle when dropped.
struct Owned(HANDLE);
impl Drop for Owned {
    fn drop(&mut self) {
        if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
            // SAFETY: a handle this code opened.
            unsafe { CloseHandle(self.0) };
        }
    }
}

/// A job that ends every process in it when closed, with no way out and
/// (where a job this process is in allows it) no desktop or clipboard.
fn job(ui: bool) -> Result<Owned, String> {
    // SAFETY: plain job object calls on a job this code owns.
    unsafe {
        let job = Owned(CreateJobObjectW(null(), null()));
        if job.0.is_null() {
            return Err(format!("can't make a job object: {}", last_error()));
        }
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags =
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION;
        if SetInformationJobObject(
            job.0,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const c_void,
            std::mem::size_of_val(&info) as u32,
        ) == 0
        {
            return Err(format!("can't limit the job object: {}", last_error()));
        }
        if ui {
            let r = JOBOBJECT_BASIC_UI_RESTRICTIONS {
                UIRestrictionsClass: JOB_OBJECT_UILIMIT_DESKTOP
                    | JOB_OBJECT_UILIMIT_DISPLAYSETTINGS
                    | JOB_OBJECT_UILIMIT_EXITWINDOWS
                    | JOB_OBJECT_UILIMIT_GLOBALATOMS
                    | JOB_OBJECT_UILIMIT_HANDLES
                    | JOB_OBJECT_UILIMIT_READCLIPBOARD
                    | JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS
                    | JOB_OBJECT_UILIMIT_WRITECLIPBOARD,
            };
            SetInformationJobObject(
                job.0,
                JobObjectBasicUIRestrictions,
                &r as *const _ as *const c_void,
                std::mem::size_of_val(&r) as u32,
            );
        }
        Ok(job)
    }
}

/// Whether this process runs in a job already (Claude Code may hold hooks in
/// one).
pub fn in_job() -> bool {
    let mut r = 0;
    // SAFETY: the current process's pseudo handle.
    unsafe { IsProcessInJob(GetCurrentProcess(), null_mut(), &mut r) != 0 && r != 0 }
}

/// Start `app` with `line` in the container, suspended, in a fresh job; run
/// it to the end (or `timeout_ms`) and return its exit code.
fn run_contained(
    package: &Sid,
    caps: &[Sid],
    app: &Path,
    line: &str,
    env: &[(OsString, OsString)],
    dir: &Path,
    timeout_ms: u32,
) -> Result<u32, String> {
    let mut attrs: Vec<SID_AND_ATTRIBUTES> = caps
        .iter()
        .map(|c| SID_AND_ATTRIBUTES {
            Sid: c.ptr(),
            Attributes: SE_GROUP_ENABLED,
        })
        .collect();
    let sc = SECURITY_CAPABILITIES {
        AppContainerSid: package.ptr(),
        Capabilities: if attrs.is_empty() {
            null_mut()
        } else {
            attrs.as_mut_ptr()
        },
        CapabilityCount: attrs.len() as u32,
        Reserved: 0,
    };
    // The child gets this process's stdin, stdout and stderr, and nothing else.
    let mut handles: Vec<HANDLE> = Vec::new();
    for h in [
        std::io::stdin().as_raw_handle(),
        std::io::stdout().as_raw_handle(),
        std::io::stderr().as_raw_handle(),
    ] {
        let h = h as HANDLE;
        if !h.is_null() && h != INVALID_HANDLE_VALUE && !handles.contains(&h) {
            // SAFETY: this process's own std handles.
            unsafe { SetHandleInformation(h, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) };
            handles.push(h);
        }
    }
    let mut block: Vec<u16> = Vec::new();
    for (k, v) in env {
        block.extend(k.encode_wide());
        block.push(u16::from(b'='));
        block.extend(v.encode_wide());
        block.push(0);
    }
    block.extend([0, 0]);
    let app_w = wide(app);
    let dir_w = wide(crate::paths::plain(dir));
    let mut line_w = wide(line);
    // SAFETY: the attribute list lives in a buffer sized by Windows and outlives
    // CreateProcessW; every handle is closed by `Owned`.
    unsafe {
        let mut size = 0usize;
        InitializeProcThreadAttributeList(null_mut(), 2, 0, &mut size);
        let mut list_buf = vec![0u64; size.div_ceil(8) + 1];
        let list = list_buf.as_mut_ptr() as *mut c_void;
        if InitializeProcThreadAttributeList(list, 2, 0, &mut size) == 0 {
            return Err(format!(
                "can't set up the process attributes: {}",
                last_error()
            ));
        }
        let mut ok = UpdateProcThreadAttribute(
            list,
            0,
            PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES as usize,
            &sc as *const _ as *const c_void,
            std::mem::size_of::<SECURITY_CAPABILITIES>(),
            null_mut(),
            null(),
        ) != 0;
        if ok && !handles.is_empty() {
            ok = UpdateProcThreadAttribute(
                list,
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                handles.as_ptr() as *const c_void,
                handles.len() * std::mem::size_of::<HANDLE>(),
                null_mut(),
                null(),
            ) != 0;
        }
        if !ok {
            DeleteProcThreadAttributeList(list);
            return Err(format!("can't set up the AppContainer: {}", last_error()));
        }
        let mut si: STARTUPINFOEXW = std::mem::zeroed();
        si.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
        si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        si.StartupInfo.hStdInput = std::io::stdin().as_raw_handle() as HANDLE;
        si.StartupInfo.hStdOutput = std::io::stdout().as_raw_handle() as HANDLE;
        si.StartupInfo.hStdError = std::io::stderr().as_raw_handle() as HANDLE;
        si.lpAttributeList = list;
        let mut pi: PROCESS_INFORMATION = std::mem::zeroed();
        let made = CreateProcessW(
            app_w.as_ptr(),
            line_w.as_mut_ptr(),
            null(),
            null(),
            1,
            EXTENDED_STARTUPINFO_PRESENT
                | CREATE_SUSPENDED
                | CREATE_UNICODE_ENVIRONMENT
                | CREATE_NO_WINDOW,
            block.as_ptr() as *const c_void,
            dir_w.as_ptr(),
            &si.StartupInfo,
            &mut pi,
        );
        let why = last_error();
        DeleteProcThreadAttributeList(list);
        if made == 0 {
            return Err(format!(
                "can't start {} in the AppContainer: {why}",
                app.display()
            ));
        }
        let (process, thread) = (Owned(pi.hProcess), Owned(pi.hThread));
        // Before it runs: a container that didn't take would run it unsandboxed.
        if let Err(why) = check_token(process.0, package) {
            TerminateProcess(process.0, 1);
            return Err(format!(
                "{} didn't start contained ({why}), so ocgen ended it before it ran: it never runs a command unsandboxed",
                app.display()
            ));
        }
        // A job this process is in may forbid a nested job's UI limits.
        let mut job_h = job(true)?;
        if AssignProcessToJobObject(job_h.0, process.0) == 0 {
            job_h = job(false)?;
            if AssignProcessToJobObject(job_h.0, process.0) == 0 {
                let why = last_error();
                TerminateProcess(process.0, 1);
                return Err(format!("can't put the command in a job object: {why}"));
            }
        }
        ResumeThread(thread.0);
        if WaitForSingleObject(process.0, timeout_ms) != WAIT_OBJECT_0 {
            TerminateJobObject(job_h.0, 1);
            return Err("timed out".into());
        }
        let mut code = 1u32;
        GetExitCodeProcess(process.0, &mut code);
        drop(job_h);
        Ok(code)
    }
}

/// The container's own folder (`…\Packages\<SID>\AC`), where it may write.
fn container_folder(package: &Sid) -> Option<PathBuf> {
    let s = wide(package.string());
    let mut out: *mut u16 = null_mut();
    // SAFETY: a NUL-terminated SID string; the path is freed with CoTaskMemFree.
    unsafe {
        if GetAppContainerFolderPath(s.as_ptr(), &mut out) < 0 || out.is_null() {
            return None;
        }
        let n = (0..).take_while(|&i| *out.add(i) != 0).count();
        let p = PathBuf::from(String::from_utf16_lossy(std::slice::from_raw_parts(out, n)));
        CoTaskMemFree(out as _);
        Some(p)
    }
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let exts = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
    crate::paths::find_in(&path, name, &exts)
}

/// Whether `prog` with `args` (`exit 7`) starts in the container and ends with
/// [`PROBE_EXIT`]: remembered in `slot` per program (path and modified time).
fn starts(
    slot: &mut Option<(PathBuf, u64, bool)>,
    prog: &Path,
    args: &[&str],
    package: &Sid,
    caps: &[Sid],
    env: &[(OsString, OsString)],
    dir: &Path,
) -> bool {
    let modified = std::fs::metadata(prog)
        .and_then(|x| x.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs());
    if let Some((p, t, ok)) = slot {
        if p == prog && *t == modified {
            return *ok;
        }
    }
    let mut line = vec![prog.to_string_lossy().into_owned()];
    line.extend(args.iter().map(|a| a.to_string()));
    let ok = run_contained(
        package,
        caps,
        prog,
        &cmdline::command_line(&line),
        env,
        dir,
        20_000,
    ) == Ok(PROBE_EXIT);
    *slot = Some((prog.to_path_buf(), modified, ok));
    ok
}

/// The PowerShell Claude Code runs commands with: `pwsh`, else Windows
/// PowerShell.
fn powershell() -> Option<PathBuf> {
    find_on_path("pwsh").or_else(|| {
        let root = std::env::var_os("SystemRoot")?;
        let p = PathBuf::from(root).join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
        p.is_file().then_some(p)
    })
}

/// A program to start: its path, its command line, and the variables the
/// command assigns it.
struct Launch {
    app: PathBuf,
    line: String,
    assign: Vec<(String, String)>,
}

/// How to start `cmd` without a shell — or why it needs one.
fn native(cmd: &str, dir: &Path) -> Result<Launch, String> {
    let a = cmdline::split(cmd).map_err(|e| {
        format!("{e} (on Windows ocgen runs it without a shell: Git Bash can't run reliably in an AppContainer): use a script or a simpler command")
    })?;
    let prog = &a.args[0];
    let exts = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
    let found = if prog.contains(['/', '\\']) {
        // A path: in the command's folder, as named or with an extension.
        let p = dir.join(prog);
        let named = (p.extension().is_some() && p.is_file()).then(|| p.clone());
        named.or_else(|| {
            let folder = p.parent()?.as_os_str().to_owned();
            let name = p.file_name()?.to_string_lossy().into_owned();
            crate::paths::find_in(&folder, &name, &exts)
        })
    } else {
        find_on_path(prog)
    };
    let program = found.ok_or_else(|| format!("`{prog}` isn't on PATH"))?;
    let ext = program
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "exe" | "com" => {
            let mut args = vec![program.to_string_lossy().into_owned()];
            args.extend(a.args[1..].iter().cloned());
            Ok(Launch {
                line: cmdline::command_line(&args),
                app: program,
                assign: a.assign,
            })
        }
        "cmd" | "bat" => {
            for arg in &a.args[1..] {
                cmdline::batch_safe(arg)?;
            }
            let comspec = std::env::var("ComSpec").unwrap_or_else(|_| r"C:\Windows\System32\cmd.exe".into());
            let mut inner = vec![program.to_string_lossy().into_owned()];
            inner.extend(a.args[1..].iter().cloned());
            let line = format!(
                "{} /d /e:off /v:off /s /c \"{}\"",
                cmdline::quote(&comspec),
                cmdline::command_line(&inner)
            );
            Ok(Launch {
                app: PathBuf::from(comspec),
                line,
                assign: a.assign,
            })
        }
        _ => Err(format!(
            "`{}` isn't a program Windows starts by itself (.exe, .com, .cmd, .bat): use a script or a simpler command",
            program.display()
        )),
    }
}

/// Run `cmd` under `p` in the AppContainer; its exit code.
pub fn run(p: &Policy, cmd: &str) -> Result<i32, String> {
    let (package, toolchain, _) = grant(p)?;
    let mut caps = vec![toolchain];
    if p.network {
        for k in [
            WinCapabilityInternetClientSid,
            WinCapabilityInternetClientServerSid,
            WinCapabilityPrivateNetworkClientServerSid,
        ] {
            caps.push(well_known(k)?);
        }
    }
    // A temporary folder of its own, in the container's folder.
    let tmp = container_folder(&package).map(|ac| {
        ac.join("Temp").join(format!(
            "run-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ))
    });
    let mut env: Vec<(OsString, OsString)> = std::env::vars_os().collect();
    if let Some(t) = tmp.as_ref().filter(|t| std::fs::create_dir_all(t).is_ok()) {
        env.retain(|(k, _)| {
            !["TEMP", "TMP", "TMPDIR"]
                .iter()
                .any(|v| k.eq_ignore_ascii_case(v))
        });
        for v in ["TEMP", "TMP", "TMPDIR"] {
            env.push((v.into(), t.clone().into_os_string()));
        }
    }
    let result = (|| {
        let Launch { app, line, assign } = native(cmd, &p.dir)?;
        let mut env = env.clone();
        for (k, v) in assign {
            env.retain(|(x, _)| !x.eq_ignore_ascii_case(OsStr::new(&k)));
            env.push((k.into(), v.into()));
        }
        run_contained(&package, &caps, &app, &line, &env, &p.dir, INFINITE).map(|c| c as i32)
    })();
    if let Some(t) = tmp {
        let _ = std::fs::remove_dir_all(t);
    }
    result
}

/// Check again whether PowerShell starts in the project's container (granting
/// `p` first): each one found, and whether it does.
pub fn check_shells(p: &Policy) -> Result<Vec<(PathBuf, bool)>, String> {
    let (package, toolchain, mut m) = grant(p)?;
    let caps = vec![toolchain];
    let env: Vec<(OsString, OsString)> = std::env::vars_os().collect();
    let mut out = Vec::new();
    if let Some(ps) = powershell() {
        m.powershell = None;
        let args = ["-NoProfile", "-NonInteractive", "-Command", "exit 7"];
        let ok = starts(&mut m.powershell, &ps, &args, &package, &caps, &env, &p.dir);
        out.push((ps, ok));
    }
    save(&m);
    Ok(out)
}

/// Run `cmd`, which Claude runs in `sh`, under `p` in the AppContainer; its
/// exit code. Its temporary folder lasts, as the system's does: a later
/// command may use what an earlier one left there.
pub fn run_agent(p: &Policy, sh: Shell, cmd: &str) -> Result<i32, String> {
    let (package, toolchain, mut m) = grant(p)?;
    let mut caps = vec![toolchain];
    if p.network {
        for k in [
            WinCapabilityInternetClientSid,
            WinCapabilityInternetClientServerSid,
            WinCapabilityPrivateNetworkClientServerSid,
        ] {
            caps.push(well_known(k)?);
        }
    }
    let mut env: Vec<(OsString, OsString)> = std::env::vars_os()
        .filter(|(k, _)| {
            let k = k.to_string_lossy();
            !p.env_remove.iter().any(|v| k.eq_ignore_ascii_case(v))
                && !k.eq_ignore_ascii_case(super::shell::COMMAND_VAR)
        })
        .collect();
    if let Some(t) = container_folder(&package).map(|ac| ac.join("Temp").join("agent")) {
        if std::fs::create_dir_all(&t).is_ok() {
            env.retain(|(k, _)| {
                !["TEMP", "TMP", "TMPDIR"]
                    .iter()
                    .any(|v| k.eq_ignore_ascii_case(v))
            });
            for v in ["TEMP", "TMP", "TMPDIR"] {
                env.push((v.into(), t.clone().into_os_string()));
            }
        }
    }
    let never = "and ocgen never runs it unsandboxed";
    let (app, line) = match sh {
        Shell::Bash => return Err(NO_GIT_BASH.to_string()),
        Shell::PowerShell => {
            let ps = powershell().ok_or(format!("PowerShell isn't found, {never}"))?;
            let args = [
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
            ];
            let mut probe = args.to_vec();
            probe.extend(["-Command", "exit 7"]);
            if !starts(
                &mut m.powershell,
                &ps,
                &probe,
                &package,
                &caps,
                &env,
                &p.dir,
            ) {
                save(&m);
                return Err(format!(
                    "{} can't start in an AppContainer here, so ocgen can't sandbox the command, {never}",
                    ps.display()
                ));
            }
            let utf16: Vec<u8> = super::shell::powershell_script(cmd)
                .encode_utf16()
                .flat_map(u16::to_le_bytes)
                .collect();
            let mut line: Vec<String> = vec![ps.to_string_lossy().into_owned()];
            line.extend(args.iter().map(|a| a.to_string()));
            line.extend(["-EncodedCommand".into(), super::shell::base64(&utf16)]);
            (ps, cmdline::command_line(&line))
        }
    };
    save(&m);
    run_contained(&package, &caps, &app, &line, &env, &p.dir, INFINITE).map(|c| c as i32)
}

/// Whether every folder `p` grants or protects has had its entry put on once
/// (the project's own, and the toolchains' shared ones) — taking care of
/// what is missing could take minutes, too long for one command — and `sh`
/// can run in it (PowerShell, unless found unable to start; never Git Bash);
/// if not, why.
pub fn ready(p: &Policy, sh: Shell) -> Result<(), String> {
    if sh == Shell::Bash {
        return Err(NO_GIT_BASH.to_string());
    }
    let m = load(&profile_name(&p.project));
    let name = "PowerShell";
    if let Some((prog, _, false)) = &m.powershell {
        if prog.is_file() {
            return Err(format!(
                "{name} ({}) can't start in an AppContainer on this machine, so ocgen can't sandbox the command, and it never runs it unsandboxed. `ocgen sandbox status` shows the sandbox; after a {name} update, `ocgen sandbox grant` checks again.",
                prog.display()
            ));
        }
    }
    let missing = acl::plan(p).aces.into_iter().find(|a| {
        let done = match a.who {
            Who::Package => &m.package,
            Who::Toolchain => &m.toolchain,
        };
        a.inherit && a.path.is_dir() && !done.iter().any(|d| super::policy::same(d, &a.path))
    });
    match missing {
        None => Ok(()),
        Some(a) => Err(format!(
            "the project's sandbox (its AppContainer) has no permissions on {} yet. Run `ocgen sandbox grant` in a terminal in the project — once; a large project takes a while — then run the command again.",
            a.path.display()
        )),
    }
}

/// One line on the container for `ocgen sandbox status`.
pub fn describe(p: &Policy) -> String {
    let profile = profile_name(&p.project);
    let m = load(&profile);
    let granted = if m.package.iter().any(|x| is_within(&p.project, x)) {
        "the project is granted"
    } else {
        "the project isn't granted yet (the first run, or `ocgen sandbox grant`, does it)"
    };
    let shell = match &m.powershell {
        Some((ps, _, true)) => format!(
            "runs the hooks' commands without a shell, Claude's PowerShell commands through {}, and refuses its Bash commands",
            ps.display()
        ),
        Some((_, _, false)) => "runs the hooks' commands without a shell; PowerShell can't start in it, so Claude's commands are refused".into(),
        None => "runs the hooks' commands without a shell; Claude's PowerShell commands not run yet; its Bash commands are refused".into(),
    };
    let job = if in_job() {
        "; hooks run inside a job already"
    } else {
        ""
    };
    format!("profile {profile}; {granted}; {shell}{job}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_an_older_start_up_check_found_is_checked_again() {
        let old: Manifest = serde_json::from_str(
            r#"{"project":"C:/p","profile":"x","package":[],"toolchain":[],"labels":[],"shell":["C:/Git/bin/sh.exe",1,true]}"#,
        )
        .unwrap();
        let m = old.current();
        assert_eq!((m.shell, m.probe), (None, PROBE_VERSION));
        let now = Manifest {
            shell: Some((PathBuf::from("C:/Git/bin/sh.exe"), 1, false)),
            probe: PROBE_VERSION,
            ..Manifest::default()
        }
        .current();
        assert!(now.shell.is_some(), "kept");
    }

    #[test]
    fn only_the_projects_container_at_low_integrity_counts_as_contained() {
        assert_eq!(contained(true, true, Some(LOW_RID)), Ok(()));
        // An AppContainer below Low (untrusted) is still contained.
        assert_eq!(contained(true, true, Some(0)), Ok(()));
        for (in_container, ours, rid, why) in [
            (false, false, Some(0x3000), "isn't in an AppContainer"),
            (true, false, Some(LOW_RID), "another AppContainer"),
            (true, true, Some(0x2000), "0x2000, above Low"),
            (true, true, None, "can't be read"),
        ] {
            let e = contained(in_container, ours, rid).unwrap_err();
            assert!(e.contains(why), "{e}");
        }
    }

    /// The command ocgen starts runs in the project's container at Low
    /// integrity, as its token says (where an AppContainer can be used).
    #[test]
    fn a_started_command_is_checked_against_its_token() {
        if probe().is_err() {
            eprintln!("skipped: {UAC_OFF}");
            return;
        }
        let t = tempfile::tempdir().unwrap();
        let dir = crate::paths::plain(&t.path().canonicalize().unwrap());
        let p = Policy {
            project: dir.clone(),
            dir: dir.clone(),
            write: vec![dir.clone()],
            ..Policy::default()
        };
        let (package, toolchain, _) = grant(&p).unwrap();
        let comspec = std::env::var("ComSpec").unwrap();
        let env: Vec<(OsString, OsString)> = std::env::vars_os().collect();
        let code = run_contained(
            &package,
            &[toolchain],
            Path::new(&comspec),
            "cmd /d /c exit 3",
            &env,
            &dir,
            60_000,
        );
        assert_eq!(code, Ok(3));
        let _ = reset(Some(&dir));
    }

    #[test]
    fn profile_names_ignore_case_and_the_verbatim_prefix() {
        let a = profile_name(Path::new(r"C:\Users\X\proj"));
        assert_eq!(a, profile_name(Path::new(r"\\?\c:\users\x\PROJ")));
        assert!(a.starts_with("ocgen.sandbox.") && a.len() == 30, "{a}");
        assert_ne!(a, profile_name(Path::new(r"C:\Users\X\other")));
    }
}
