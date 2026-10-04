//! The live viewer: a small loopback HTTP server (`ocgen notes serve <dir>`, std
//! only) that serves a project's ledger pages and keeps one Server-Sent Events
//! connection per open tab. That connection is how ocgen knows — on every OS —
//! whether a page is already open: an update reloads the open tabs, and only
//! when there are none does the hook open a new one.
//!
//! Run for an /intent drafts directory, the same server is the issue draft's
//! editor ([`super::draft`]): it serves an editor page per draft, saves what the
//! page sends back to the file (only over the version the page last saw, and
//! only from the page itself: its `Origin`), renders the preview, and tells open
//! tabs when the file changes on disk.
//!
//! One server per notes directory, found through `.viewer.json` (port, token,
//! pid). The token is in every URL and the `Host` header must be loopback, so
//! other local pages can't read the notes. The server exits after a long idle
//! spell with no tabs, or when another server takes over the directory.

use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::is_slug;

/// Bump when the server's HTTP contract changes; a hook never talks to a server
/// of another protocol (it asks it to quit and starts its own).
pub const VIEWER_PROTOCOL: u32 = 1;
/// The server's address card, in the notes directory.
pub const INFO_FILE: &str = ".viewer.json";
const APP: &str = "ocgen-notes";

/// What `.viewer.json` holds.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Info {
    pub app: String,
    pub protocol: u32,
    pub port: u16,
    pub pid: u32,
    pub token: String,
    /// The notes directory the server is for (see [`root_key`]).
    pub root: String,
}

impl Info {
    /// The page URL for ledger `slug`.
    pub fn url(&self, slug: &str) -> String {
        format!("http://127.0.0.1:{}/{}/{slug}.html", self.port, self.token)
    }
}

/// What the server did with an update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Told this many open tabs to reload.
    Reloaded(usize),
    /// No tab is connected right now, but one was just opened or is reloading:
    /// it will pick up the update when it connects. Don't open another.
    Pending,
    /// No tab shows the page: open one.
    Open,
}

/// The identity of a notes directory, as the server reports it.
pub fn root_key(dir: &Path) -> String {
    let p = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    let s = crate::paths::for_shell(&p);
    if cfg!(windows) {
        s.to_lowercase()
    } else {
        s
    }
}

pub fn read_info(dir: &Path) -> Option<Info> {
    serde_json::from_str(&fs::read_to_string(dir.join(INFO_FILE)).ok()?).ok()
}

// ------------------------------------------------------------------ client --

/// A one-shot HTTP/1.1 request to the loopback server: `(status, body)`.
fn request(port: u16, method: &str, path: &str, timeout: Duration) -> Option<(u16, String)> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_millis(300)).ok()?;
    s.set_read_timeout(Some(timeout)).ok()?;
    s.set_write_timeout(Some(timeout)).ok()?;
    write!(
        s,
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\nContent-Length: 0\r\n\r\n"
    )
    .ok()?;
    let mut buf = String::new();
    s.read_to_string(&mut buf).ok()?;
    let status = buf.split(' ').nth(1)?.parse().ok()?;
    let body = buf
        .split_once("\r\n\r\n")
        .map(|x| x.1.to_string())
        .unwrap_or_default();
    Some((status, body))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ping {
    /// A live server for this directory, speaking this protocol.
    Ours,
    /// A live viewer for this directory, but of another protocol.
    Other,
    /// Nothing usable answers.
    Dead,
}

pub fn ping(info: &Info, root: &str) -> Ping {
    let Some((200, body)) = request(
        info.port,
        "GET",
        &format!("/{}/ping", info.token),
        Duration::from_secs(1),
    ) else {
        return Ping::Dead;
    };
    let v: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    if v["app"] != APP || v["root"] != root {
        Ping::Dead
    } else if v["protocol"] == VIEWER_PROTOCOL {
        Ping::Ours
    } else {
        Ping::Other
    }
}

/// Tell the server ledger `slug` changed. `force` opens a tab even right after
/// one was opened (an explicit `ocgen notes open`).
pub fn reload(info: &Info, slug: &str, force: bool) -> Option<Action> {
    let force = if force { "&force=1" } else { "" };
    let (code, body) = request(
        info.port,
        "POST",
        &format!("/{}/reload?topic={slug}{force}", info.token),
        Duration::from_secs(3),
    )?;
    if code != 200 {
        return None;
    }
    let v: Value = serde_json::from_str(&body).ok()?;
    match v["action"].as_str()? {
        "reloaded" => Some(Action::Reloaded(v["tabs"].as_u64()? as usize)),
        "pending" => Some(Action::Pending),
        "open" => Some(Action::Open),
        _ => None,
    }
}

/// Ask the server to exit.
pub fn quit(info: &Info) {
    let _ = request(
        info.port,
        "POST",
        &format!("/{}/quit", info.token),
        Duration::from_secs(1),
    );
}

/// The live server for `dir`, starting one if needed. `None` when none could
/// be started (no ocgen binary to run, a sandbox, …): callers fall back.
pub fn ensure(dir: &Path, env: &HashMap<String, String>) -> Option<Info> {
    let root = root_key(dir);
    if let Some(info) = read_info(dir) {
        match ping(&info, &root) {
            Ping::Ours => return Some(info),
            Ping::Other => quit(&info),
            Ping::Dead => {}
        }
    }
    let exe = server_exe()?;
    let mut c = Command::new(exe);
    c.args(["notes", "serve"])
        .arg(crate::paths::plain(dir))
        .current_dir(std::env::temp_dir())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // The caller's view of the environment wins (it may differ from ours).
    for (k, v) in env {
        if k.starts_with("OCGEN_NOTES_") {
            c.env(k, v);
        }
    }
    spawn_detached(&mut c).ok()?;
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(50));
        if let Some(info) = read_info(dir) {
            if ping(&info, &root) == Ping::Ours {
                return Some(info);
            }
        }
    }
    None
}

/// The binary to run the server with: this one, if it is ocgen (not, say, a
/// test harness calling the library).
fn server_exe() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    (exe.file_stem()? == "ocgen").then_some(exe)
}

// -------------------------------------------------------- detached spawns --

/// Make `c` outlive this process and not hold on to its output: Claude Code
/// waits for a hook's stdout to close, so a child that inherits it would make
/// every hook wait for the server to exit.
pub(crate) fn detach(c: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        c.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        win::no_inherit_std_handles();
        c.creation_flags(win::CREATE_NO_WINDOW | win::CREATE_NEW_PROCESS_GROUP);
    }
    #[cfg(not(any(unix, windows)))]
    let _ = c;
}

fn spawn_detached(c: &mut Command) -> std::io::Result<()> {
    detach(c);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // Leave Claude Code's job object too, when it allows that; otherwise the
        // server dies with the hook.
        c.creation_flags(
            win::DETACHED_PROCESS | win::CREATE_NEW_PROCESS_GROUP | win::CREATE_BREAKAWAY_FROM_JOB,
        );
        if c.spawn().is_ok() {
            return Ok(());
        }
        c.creation_flags(win::DETACHED_PROCESS | win::CREATE_NEW_PROCESS_GROUP);
    }
    c.spawn().map(|_| ())
}

#[cfg(windows)]
mod win {
    use std::ffi::c_void;

    pub const DETACHED_PROCESS: u32 = 0x0000_0008;
    pub const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    pub const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
    pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const HANDLE_FLAG_INHERIT: u32 = 0x1;
    const STD_HANDLES: [u32; 3] = [-10i32 as u32, -11i32 as u32, -12i32 as u32];

    #[link(name = "kernel32")]
    extern "system" {
        fn GetStdHandle(n: u32) -> *mut c_void;
        fn SetHandleInformation(h: *mut c_void, mask: u32, flags: u32) -> i32;
    }

    /// Rust spawns with handle inheritance on, so a detached child would get
    /// copies of this process's stdio pipes. Mark them non-inheritable first.
    pub fn no_inherit_std_handles() {
        for n in STD_HANDLES {
            // SAFETY: plain Win32 calls on this process's own std handles.
            unsafe {
                let h = GetStdHandle(n);
                if !h.is_null() && h as isize != -1 {
                    SetHandleInformation(h, HANDLE_FLAG_INHERIT, 0);
                }
            }
        }
    }
}

// ------------------------------------------------------------------ server --

struct Client {
    id: u64,
    stream: TcpStream,
}

/// What a server serves: /inquire ledger pages, or /intent issue draft editors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Notes,
    Drafts,
}

struct State {
    clients: HashMap<String, Vec<Client>>,
    /// Drafts: the revision each draft's tabs were last told of.
    revs: HashMap<String, String>,
    opened: HashMap<String, Instant>,
    left: HashMap<String, Instant>,
    last_activity: Instant,
    next_id: u64,
}

struct Server {
    kind: Kind,
    dir: PathBuf,
    token: String,
    port: u16,
    root: String,
    /// After opening a tab, how long to wait for it to connect.
    grace: Duration,
    /// After a tab disconnects (it may be reloading), how long to wait for it.
    left_grace: Duration,
    idle: Duration,
    state: Mutex<State>,
    quit: AtomicBool,
}

fn env_u64(key: &str) -> Option<u64> {
    std::env::var(key).ok().and_then(|v| v.trim().parse().ok())
}

/// A random 128-bit hex token (std's hasher keys come from the OS RNG).
fn random_token() -> String {
    use std::hash::{BuildHasher, Hasher};
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut out = String::new();
    for i in 0..2u8 {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u128(nanos);
        h.write_u32(std::process::id());
        h.write_u8(i);
        out.push_str(&format!("{:016x}", h.finish()));
    }
    out
}

/// Close every descriptor this process inherited besides stdio. A sibling
/// spawned at the same moment can leak a pipe into the hook that starts us
/// (macOS sets close-on-exec only after creating a pipe); holding it open would
/// make whoever reads that pipe — Claude Code waiting on a hook — wait for us.
#[cfg(unix)]
fn close_inherited_fds() {
    extern "C" {
        fn close(fd: i32) -> i32;
    }
    let fds: Vec<i32> = fs::read_dir("/dev/fd")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().to_str()?.parse().ok())
        .filter(|&fd| fd > 2)
        .collect();
    for fd in fds {
        // SAFETY: nothing in this process owns these yet (called first thing);
        // the directory listing's own descriptor is already closed (EBADF).
        unsafe {
            close(fd);
        }
    }
}

/// Run the viewer for notes directory `dir` until it is idle or superseded.
/// Only a `.claude/notes` directory — the server writes `.viewer.json` there and
/// renders pages next to their ledgers — or an /intent drafts directory, whose
/// drafts it edits.
pub fn serve(dir: &Path) -> Result<()> {
    #[cfg(unix)]
    close_inherited_fds();
    let kind = if super::is_notes_dir(dir) {
        Kind::Notes
    } else if super::draft::is_drafts_dir(dir) {
        Kind::Drafts
    } else {
        anyhow::bail!(
            "{} is not a .claude/notes or {} directory",
            dir.display(),
            super::draft::DIR
        );
    };
    let dir = dir.canonicalize()?;
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let port = listener.local_addr()?.port();
    let grace = Duration::from_millis(env_u64("OCGEN_NOTES_GRACE_MS").unwrap_or(15_000));
    let srv = Arc::new(Server {
        kind,
        root: root_key(&dir),
        dir,
        token: random_token(),
        port,
        grace,
        left_grace: grace.min(Duration::from_secs(3)),
        idle: Duration::from_secs(env_u64("OCGEN_NOTES_IDLE_SECS").unwrap_or(1800)),
        state: Mutex::new(State {
            clients: HashMap::new(),
            revs: HashMap::new(),
            opened: HashMap::new(),
            left: HashMap::new(),
            last_activity: Instant::now(),
            next_id: 0,
        }),
        quit: AtomicBool::new(false),
    });
    {
        let srv = Arc::clone(&srv);
        // A thread per connection, up to a limit: each open tab holds one, and a
        // local process opening connections it never finishes can't pile up more.
        let open = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        std::thread::spawn(move || {
            for s in listener.incoming().flatten() {
                if open.load(Ordering::SeqCst) >= MAX_CONNECTIONS {
                    continue; // dropped: closed at once
                }
                open.fetch_add(1, Ordering::SeqCst);
                let (srv, open) = (Arc::clone(&srv), Arc::clone(&open));
                std::thread::spawn(move || {
                    srv.handle(s);
                    open.fetch_sub(1, Ordering::SeqCst);
                });
            }
        });
    }
    let info = Info {
        app: APP.into(),
        protocol: VIEWER_PROTOCOL,
        port,
        pid: std::process::id(),
        token: srv.token.clone(),
        root: srv.root.clone(),
    };
    if !claim(&srv.dir, &info)? {
        return Ok(()); // another server has this directory
    }
    srv.watch(&info);
    if read_info(&srv.dir).is_some_and(|i| i.pid == info.pid && i.token == info.token) {
        let _ = fs::remove_file(srv.dir.join(INFO_FILE));
    }
    Ok(())
}

/// Write `.viewer.json` unless a live server already owns the directory.
/// `create_new` makes the claim atomic, so concurrent starts end with one server.
fn claim(dir: &Path, info: &Info) -> Result<bool> {
    let path = dir.join(INFO_FILE);
    let body = serde_json::to_string(info)?;
    for _ in 0..40 {
        let mut o = fs::OpenOptions::new();
        o.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o600);
        }
        match o.open(&path) {
            Ok(mut f) => {
                f.write_all(body.as_bytes())?;
                return Ok(true);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                match read_info(dir) {
                    Some(other) => match ping(&other, &info.root) {
                        Ping::Ours => return Ok(false),
                        Ping::Other => {
                            quit(&other);
                            take_over(&path);
                        }
                        Ping::Dead => take_over(&path),
                    },
                    None => {
                        // Unreadable: being written right now, or garbage.
                        let young = fs::metadata(&path)
                            .and_then(|m| m.modified())
                            .ok()
                            .and_then(|t| t.elapsed().ok())
                            .is_some_and(|age| age < Duration::from_secs(2));
                        if young {
                            std::thread::sleep(Duration::from_millis(100));
                        } else {
                            take_over(&path);
                        }
                    }
                }
            }
            Err(e) => return Err(e.into()),
        }
    }
    Ok(false)
}

/// Move a stale `.viewer.json` aside (only one contender's rename succeeds).
fn take_over(path: &Path) {
    let aside = path.with_extension(format!("json.stale-{}", std::process::id()));
    if fs::rename(path, &aside).is_ok() {
        let _ = fs::remove_file(aside);
    }
}

struct Request {
    method: String,
    target: String,
    host: String,
    /// The `Origin` header, if any (browsers send it with every POST).
    origin: Option<String>,
    /// The body; `None` when it was larger than [`MAX_BODY`] (and not read).
    body: Option<Vec<u8>>,
}

/// The most a request's line and headers may take.
const MAX_HEAD: u64 = 16 * 1024;
/// The largest body accepted: a draft, or the text to preview.
const MAX_BODY: usize = 1024 * 1024;
/// The most connections served at once (open tabs included).
const MAX_CONNECTIONS: usize = 64;

/// Read a request from `s`. Its body is read only when `wanted(host, target)`
/// says the request may be served: a stranger can't make the server wait for,
/// or hold, a body it will refuse anyway.
fn read_request(s: &TcpStream, wanted: impl Fn(&str, &str) -> bool) -> Option<Request> {
    let mut r = BufReader::new(s);
    let mut left = MAX_HEAD;
    let mut line = |r: &mut BufReader<&TcpStream>| -> Option<String> {
        let mut l = String::new();
        let n = r.by_ref().take(left).read_line(&mut l).ok()?;
        left = left.checked_sub(n as u64)?;
        (n > 0 && l.ends_with('\n')).then_some(l)
    };
    let first = line(&mut r)?;
    let mut parts = first.split_whitespace();
    let method = parts.next()?.to_string();
    let target = parts.next()?.to_string();
    let (mut host, mut origin, mut length) = (String::new(), None, 0usize);
    loop {
        let h = line(&mut r)?;
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            let (k, v) = (k.trim(), v.trim());
            if k.eq_ignore_ascii_case("host") {
                host = v.to_string();
            } else if k.eq_ignore_ascii_case("origin") {
                origin = Some(v.to_string());
            } else if k.eq_ignore_ascii_case("content-length") {
                length = v.parse().ok()?;
            }
        }
    }
    let body = if length > MAX_BODY {
        None
    } else if !wanted(&host, &target) {
        Some(Vec::new())
    } else {
        let mut b = vec![0; length];
        r.read_exact(&mut b).ok()?;
        Some(b)
    };
    Some(Request {
        method,
        target,
        host,
        origin,
        body,
    })
}

fn respond(s: &mut TcpStream, code: u16, ctype: &str, extra: &str, body: &[u8]) {
    let reason = match code {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        413 => "Payload Too Large",
        _ => "Error",
    };
    let head = format!(
        "HTTP/1.1 {code} {reason}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\n\
         Referrer-Policy: no-referrer\r\n{extra}Connection: close\r\n\r\n",
        body.len()
    );
    let _ = s.write_all(head.as_bytes());
    let _ = s.write_all(body);
    let _ = s.flush();
}

fn plain(s: &mut TcpStream, code: u16, msg: &str) {
    respond(s, code, "text/plain; charset=utf-8", "", msg.as_bytes());
}

fn json_reply(s: &mut TcpStream, v: &Value) {
    respond(s, 200, "application/json", "", v.to_string().as_bytes());
}

fn query(q: &str) -> HashMap<&str, &str> {
    q.split('&').filter_map(|kv| kv.split_once('=')).collect()
}

const CSP: &str = "Content-Security-Policy: default-src 'none'; img-src data: https:; \
style-src 'unsafe-inline'; script-src 'unsafe-inline' https://cdn.jsdelivr.net; \
connect-src 'self'; font-src data:; base-uri 'none'; form-action 'none'; frame-ancestors 'none'\r\n";

/// A draft editor's policy: its one script (by nonce) and nothing from outside —
/// a draft's text can't run anything or load anything (an image URL could carry
/// data out past the sandbox's network rules).
fn draft_csp(nonce: &str) -> String {
    format!(
        "Content-Security-Policy: default-src 'none'; img-src data:; style-src 'unsafe-inline'; \
         script-src 'nonce-{nonce}'; connect-src 'self'; base-uri 'none'; form-action 'none'; \
         frame-ancestors 'none'\r\n"
    )
}

/// The live-reload client, added to every page the server serves (the file on
/// disk stays static, so it also opens fine without the server).
fn inject(page: &str, slug: &str, rev: &str) -> String {
    let script = format!(
        r#"<script>
(function () {{
  var errors = 0;
  var es = new EventSource("events?topic={slug}&rev={rev}");
  es.addEventListener("reload", function () {{ es.close(); location.reload(); }});
  es.onopen = function () {{ errors = 0; document.body.removeAttribute("data-viewer"); }};
  es.onerror = function () {{ if (++errors > 5) document.body.setAttribute("data-viewer", "offline"); }};
  window.addEventListener("pageshow", function (e) {{ if (e.persisted) location.reload(); }});
}})();
</script>
"#
    );
    match page.rfind("</body>") {
        Some(i) => format!("{}{script}{}", &page[..i], &page[i..]),
        None => format!("{page}{script}"),
    }
}

/// The event that tells a draft's tabs the file is now at revision `rev`.
fn changed_event(rev: &str) -> String {
    format!("event: changed\ndata: {rev}\n\n")
}

fn newer(a: &Path, b: &Path) -> bool {
    let m = |p: &Path| fs::metadata(p).and_then(|m| m.modified()).ok();
    match (m(a), m(b)) {
        (Some(a), Some(b)) => a > b,
        (Some(_), None) => true,
        _ => false,
    }
}

impl Server {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn touch(&self) {
        self.lock().last_activity = Instant::now();
    }

    fn handle(&self, mut s: TcpStream) {
        let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
        let port = self.port;
        let host_ok =
            |h: &str| h == format!("127.0.0.1:{port}") || h == format!("localhost:{port}");
        let prefix = format!("/{}/", self.token);
        let Some(req) = read_request(&s, |h, t| host_ok(h) && t.starts_with(&prefix)) else {
            return;
        };
        if !host_ok(&req.host) {
            return plain(&mut s, 403, "forbidden");
        }
        let (path, q) = req.target.split_once('?').unwrap_or((&req.target, ""));
        if path.contains('%') || path.contains("..") {
            return plain(&mut s, 404, "not found");
        }
        let Some(rest) = path
            .strip_prefix('/')
            .and_then(|p| p.strip_prefix(self.token.as_str()))
            .and_then(|p| p.strip_prefix('/'))
        else {
            return plain(&mut s, 404, "not found");
        };
        // Only requests that passed the Host and token checks count as use:
        // anything probing the port must not keep an idle viewer alive.
        self.touch();
        let q = query(q);
        let topic = q.get("topic").copied().filter(|t| is_slug(t));
        match (req.method.as_str(), rest) {
            ("GET", "ping") => json_reply(
                &mut s,
                &json!({ "app": APP, "protocol": VIEWER_PROTOCOL, "root": self.root }),
            ),
            ("POST", "reload") => match topic {
                Some(t) => {
                    let action = self.decide(t, q.get("force") == Some(&"1"));
                    let v = match action {
                        Action::Reloaded(n) => json!({ "action": "reloaded", "tabs": n }),
                        Action::Pending => json!({ "action": "pending" }),
                        Action::Open => json!({ "action": "open" }),
                    };
                    json_reply(&mut s, &v);
                }
                None => plain(&mut s, 400, "bad topic"),
            },
            ("POST", "quit") => {
                json_reply(&mut s, &json!({ "action": "quit" }));
                self.quit.store(true, Ordering::SeqCst);
            }
            ("GET", "events") => match topic {
                Some(t) => self.events(s, t, q.get("rev").copied().unwrap_or("")),
                None => plain(&mut s, 400, "bad topic"),
            },
            ("POST", "render") if self.kind == Kind::Drafts => self.preview(s, &req),
            (_, "ping" | "reload" | "quit" | "events" | "render") => {
                plain(&mut s, 405, "method not allowed")
            }
            ("GET", page) if page.ends_with(".html") => {
                match page.strip_suffix(".html").filter(|p| is_slug(p)) {
                    Some(slug) => self.page(s, slug),
                    None => plain(&mut s, 404, "not found"),
                }
            }
            (method, file) if self.kind == Kind::Drafts => {
                match (method, file.strip_suffix(".md").filter(|p| is_slug(p))) {
                    ("GET", Some(slug)) => self.load_draft(s, slug),
                    ("POST", Some(slug)) => {
                        self.save_draft(s, slug, &req, q.get("rev").copied().unwrap_or(""))
                    }
                    _ => plain(&mut s, 404, "not found"),
                }
            }
            _ => plain(&mut s, 404, "not found"),
        }
    }

    /// Whether a request comes from this server's own pages (a POST from any
    /// other page, or from none, carries another `Origin` or none).
    fn same_origin(&self, req: &Request) -> bool {
        let port = self.port;
        req.origin.as_deref().is_some_and(|o| {
            o == format!("http://127.0.0.1:{port}") || o == format!("http://localhost:{port}")
        })
    }

    /// The draft file `slug` names: a regular file in the drafts directory.
    fn draft_file(&self, slug: &str) -> Option<PathBuf> {
        let p = self.dir.join(format!("{slug}.md"));
        fs::symlink_metadata(&p)
            .is_ok_and(|m| m.file_type().is_file())
            .then_some(p)
    }

    /// The preview of the text the page sends.
    fn preview(&self, mut s: TcpStream, req: &Request) {
        if !self.same_origin(req) {
            return plain(&mut s, 403, "forbidden");
        }
        let Some(body) = &req.body else {
            return plain(&mut s, 413, "too large");
        };
        let html = super::draft::preview(&String::from_utf8_lossy(body));
        respond(&mut s, 200, "text/html; charset=utf-8", "", html.as_bytes());
    }

    /// A draft's text and revision.
    fn load_draft(&self, mut s: TcpStream, slug: &str) {
        let Some(text) = self
            .draft_file(slug)
            .and_then(|p| fs::read_to_string(p).ok())
        else {
            return plain(&mut s, 404, "not found");
        };
        let rev = super::rev(text.as_bytes());
        json_reply(&mut s, &json!({ "text": text, "rev": rev }));
    }

    /// Save the page's text to draft `slug` — only over revision `base`, the
    /// version the page last saw; otherwise answer 409 with what the file holds.
    fn save_draft(&self, mut s: TcpStream, slug: &str, req: &Request, base: &str) {
        if !self.same_origin(req) {
            return plain(&mut s, 403, "forbidden");
        }
        let Some(path) = self.draft_file(slug) else {
            return plain(&mut s, 404, "not found");
        };
        let Some(body) = &req.body else {
            return plain(&mut s, 413, "too large");
        };
        let Ok(text) = std::str::from_utf8(body) else {
            return plain(&mut s, 400, "not UTF-8");
        };
        // One save at a time, and the tabs hear of it in the same step.
        let mut st = self.lock();
        let on_disk = fs::read_to_string(&path).unwrap_or_default();
        let current = super::rev(on_disk.as_bytes());
        if current != base {
            drop(st);
            let v = json!({ "text": on_disk, "rev": current });
            return respond(
                &mut s,
                409,
                "application/json",
                "",
                v.to_string().as_bytes(),
            );
        }
        if let Err(e) = super::write_if_changed(&path, text.as_bytes()) {
            drop(st);
            return plain(&mut s, 500, &format!("{e:#}"));
        }
        let new = super::rev(text.as_bytes());
        st.revs.insert(slug.to_string(), new.clone());
        Self::tell(&mut st, slug, &changed_event(&new));
        drop(st);
        json_reply(&mut s, &json!({ "rev": new }));
    }

    /// Send `msg` to every open tab of `topic`, dropping the ones gone; how
    /// many are left.
    fn tell(st: &mut State, topic: &str, msg: &str) -> usize {
        match st.clients.get_mut(topic) {
            Some(list) => {
                list.retain_mut(|c| c.stream.write_all(msg.as_bytes()).is_ok());
                list.len()
            }
            None => 0,
        }
    }

    /// The current revision of draft `slug` ("" when it is gone).
    fn draft_rev(&self, slug: &str) -> String {
        self.draft_file(slug)
            .and_then(|p| fs::read(p).ok())
            .map(|b| super::rev(&b))
            .unwrap_or_default()
    }

    fn page(&self, mut s: TcpStream, slug: &str) {
        if self.kind == Kind::Drafts {
            return self.draft_page(s, slug);
        }
        let html = self.dir.join(format!("{slug}.html"));
        let md = self.dir.join(format!("{slug}.md"));
        // Keep the page in step even with edits the hook didn't see. `serve`
        // checked this is a notes directory; `slug` is a checked slug.
        if newer(&md, &html) {
            let _ = super::render_ledger(&md, slug);
        }
        let Ok(bytes) = fs::read(&html) else {
            return plain(&mut s, 404, "not found");
        };
        let page = inject(&String::from_utf8_lossy(&bytes), slug, &super::rev(&bytes));
        respond(
            &mut s,
            200,
            "text/html; charset=utf-8",
            CSP,
            page.as_bytes(),
        );
    }

    /// The editor page for draft `slug`, rendered fresh from the file.
    fn draft_page(&self, mut s: TcpStream, slug: &str) {
        let Some(path) = self.draft_file(slug) else {
            return plain(&mut s, 404, "not found");
        };
        let Ok(text) = fs::read_to_string(&path) else {
            return plain(&mut s, 404, "not found");
        };
        let nonce = random_token();
        let source = format!("{}/{slug}.md", super::draft::DIR);
        let language = super::answer_language(&path);
        match super::draft::page(
            &text,
            &source,
            &super::rev(text.as_bytes()),
            &nonce,
            &language,
            &super::draft::approvers_for(&path),
        ) {
            Ok(page) => respond(
                &mut s,
                200,
                "text/html; charset=utf-8",
                &draft_csp(&nonce),
                page.as_bytes(),
            ),
            Err(e) => plain(&mut s, 500, &format!("{e:#}")),
        }
    }

    fn events(&self, mut s: TcpStream, topic: &str, rev: &str) {
        let head =
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-store\r\n\
                    X-Accel-Buffering: no\r\nConnection: keep-alive\r\n\r\nretry: 1000\n\n";
        if s.write_all(head.as_bytes()).is_err() {
            return;
        }
        let Ok(mut writer) = s.try_clone() else {
            return;
        };
        let _ = writer.set_write_timeout(Some(Duration::from_secs(1)));
        let current = match self.kind {
            Kind::Notes => fs::read(self.dir.join(format!("{topic}.html")))
                .map(|b| super::rev(&b))
                .unwrap_or_default(),
            Kind::Drafts => self.draft_rev(topic),
        };
        let id = {
            let mut st = self.lock();
            st.next_id += 1;
            let id = st.next_id;
            // The page is older than the file (it loaded mid-update): refresh now.
            if !rev.is_empty() && rev != current {
                let msg = match self.kind {
                    Kind::Notes => "event: reload\ndata: 1\n\n".to_string(),
                    Kind::Drafts => changed_event(&current),
                };
                let _ = writer.write_all(msg.as_bytes());
            }
            if self.kind == Kind::Drafts {
                st.revs.entry(topic.to_string()).or_insert(current);
            }
            st.clients
                .entry(topic.to_string())
                .or_default()
                .push(Client { id, stream: writer });
            id
        };
        // Block until the tab goes away (closed, navigated or reloading).
        let _ = s.set_read_timeout(None);
        let mut buf = [0u8; 256];
        loop {
            match s.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
        }
        let mut st = self.lock();
        if let Some(list) = st.clients.get_mut(topic) {
            list.retain(|c| c.id != id);
        }
        st.left.insert(topic.to_string(), Instant::now());
        st.last_activity = Instant::now();
    }

    /// Reload `topic`'s tabs, or say whether a new one should be opened —
    /// decided under one lock, so concurrent updates never open two tabs.
    fn decide(&self, topic: &str, force: bool) -> Action {
        // A draft's tabs are told its revision, never reloaded: they may hold edits.
        let msg = match self.kind {
            Kind::Notes => "event: reload\ndata: 1\n\n".to_string(),
            Kind::Drafts => changed_event(&self.draft_rev(topic)),
        };
        let mut st = self.lock();
        let now = Instant::now();
        st.last_activity = now;
        let n = Self::tell(&mut st, topic, &msg);
        let recent = |m: &HashMap<String, Instant>, d: Duration| {
            m.get(topic).is_some_and(|t| now.duration_since(*t) < d)
        };
        if n > 0 {
            Action::Reloaded(n)
        } else if !force && (recent(&st.opened, self.grace) || recent(&st.left, self.left_grace)) {
            Action::Pending
        } else {
            st.opened.insert(topic.to_string(), now);
            Action::Open
        }
    }

    /// Keep connections alive and decide when to exit.
    fn watch(&self, info: &Info) {
        let mut last_ping = Instant::now();
        loop {
            std::thread::sleep(Duration::from_millis(200));
            if self.quit.load(Ordering::SeqCst) {
                return;
            }
            let mut st = self.lock();
            if last_ping.elapsed() >= Duration::from_secs(5) {
                last_ping = Instant::now();
                for list in st.clients.values_mut() {
                    list.retain_mut(|c| c.stream.write_all(b": keepalive\n\n").is_ok());
                }
            }
            let tabs: usize = st.clients.values().map(Vec::len).sum();
            if tabs == 0 && st.last_activity.elapsed() >= self.idle {
                return;
            }
            let open: Vec<String> = match self.kind {
                Kind::Drafts => st
                    .clients
                    .iter()
                    .filter(|(_, l)| !l.is_empty())
                    .map(|(t, _)| t.clone())
                    .collect(),
                Kind::Notes => Vec::new(),
            };
            drop(st);
            // Drafts change under open tabs (Claude, another editor): tell them.
            for topic in open {
                let now = self.draft_rev(&topic);
                let mut st = self.lock();
                if st.revs.get(&topic) != Some(&now) {
                    st.revs.insert(topic.clone(), now.clone());
                    Self::tell(&mut st, &topic, &changed_event(&now));
                }
            }
            if !self.dir.is_dir() {
                return;
            }
            // Superseded: hooks now talk to another server.
            match read_info(&self.dir) {
                Some(i) if i.pid == info.pid && i.token == info.token => {}
                _ => return,
            }
        }
    }
}
