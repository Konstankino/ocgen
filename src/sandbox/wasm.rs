//! Every OS, last of all: a command that is a WASI module (`*.wasm`) runs in
//! wasmtime, with no sockets and only its folder opened to it — the folder as
//! `/`, read-only where a protected path lies (as Landlock does it: see
//! [`split`]), writable everywhere else. Anything else isn't a WASI module, so
//! this sandbox refuses it.

use std::path::{Path, PathBuf};

use wasmtime::{Engine, Linker, Module, Store};
use wasmtime_wasi::p1::WasiP1Ctx;
use wasmtime_wasi::{FsPerms, I32Exit, WasiCtxBuilder};

use super::cmdline;
use super::policy::{is_within, list_dir, split, Policy};

/// The module `cmd` runs, if it is one: its first word (after `NAME=value`
/// assignments) names a `*.wasm` file, in `dir` when relative.
pub fn module(cmd: &str, dir: &Path) -> Result<(PathBuf, cmdline::Argv), String> {
    let a = cmdline::split(cmd).map_err(|e| format!("not a WASI module: {e}"))?;
    let prog = &a.args[0];
    if !prog.ends_with(".wasm") {
        return Err(format!("`{prog}` isn't a WASI module (*.wasm)"));
    }
    let path = dir.join(prog);
    if !path.is_file() {
        return Err(format!("{} doesn't exist", path.display()));
    }
    Ok((path, a))
}

/// Whether this sandbox can run `cmd` under `p`: a WASI module, with nothing
/// hidden inside a grant (WASI can't take a path inside an open folder back).
pub fn admits(p: &Policy, cmd: &str) -> Result<(), String> {
    module(cmd, &p.dir)?;
    if p.hide.iter().any(|h| is_within(h, &p.dir)) {
        return Err("a hidden path lies in the command's folder".into());
    }
    Ok(())
}

/// The folders the module gets: (host folder, guest path, writable), its
/// folder first (as `/`).
pub fn preopens(p: &Policy) -> Vec<(PathBuf, String, bool)> {
    let s = split(std::slice::from_ref(&p.dir), &p.protect, &list_dir);
    let guest = |h: &Path| -> Option<String> {
        let rel = h.strip_prefix(&p.dir).ok()?;
        let rel = rel.to_string_lossy().replace('\\', "/");
        Some(format!("/{rel}").replace("//", "/"))
    };
    let mut out = vec![(p.dir.clone(), "/".to_string(), !s.spine.contains(&p.dir))];
    for d in &s.spine {
        if d != &p.dir {
            out.extend(guest(d).map(|g| (d.clone(), g, false)));
        }
    }
    for w in s.write.iter().filter(|w| w.is_dir() && *w != &p.dir) {
        out.extend(guest(w).map(|g| (w.clone(), g, true)));
    }
    out
}

/// Run `cmd`'s module under `p`; its exit code.
pub fn run(p: &Policy, cmd: &str) -> Result<i32, String> {
    let (path, a) = module(cmd, &p.dir)?;
    let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    // A component (`\0asm`, then a layer other than a core module's 1).
    if bytes.len() >= 8 && bytes[..4] == *b"\0asm" && bytes[4..8] != [1, 0, 0, 0] {
        return Err(format!(
            "{} is a WebAssembly component; this sandbox runs WASI preview 1 modules",
            path.display()
        ));
    }
    let err = |e: wasmtime::Error| format!("{}: {e:#}", path.display());
    let engine = Engine::default();
    let module = Module::new(&engine, &bytes).map_err(err)?;
    let mut linker: Linker<WasiP1Ctx> = Linker::new(&engine);
    wasmtime_wasi::p1::add_to_linker_sync(&mut linker, |t| t).map_err(err)?;
    let mut b = WasiCtxBuilder::new();
    b.inherit_stdout().inherit_stderr().args(&a.args);
    for (k, v) in std::env::vars() {
        if !a.assign.iter().any(|(x, _)| *x == k) {
            b.env(k, v);
        }
    }
    for (k, v) in &a.assign {
        b.env(k, v);
    }
    for (host, guest, write) in preopens(p) {
        let perms = if write {
            FsPerms::ReadWrite
        } else {
            FsPerms::ReadOnly
        };
        b.preopened_dir(&host, &guest, perms).map_err(err)?;
    }
    let mut store = Store::new(&engine, b.build_p1());
    let instance = linker.instantiate(&mut store, &module).map_err(err)?;
    let start = instance
        .get_typed_func::<(), ()>(&mut store, "_start")
        .map_err(err)?;
    match start.call(&mut store, ()) {
        Ok(()) => Ok(0),
        Err(e) => match e.downcast_ref::<I32Exit>() {
            Some(exit) => Ok(exit.0),
            None => {
                eprintln!("wasm trap: {e:#}");
                Ok(1)
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_wasi_modules_are_admitted() {
        let t = tempfile::tempdir().unwrap();
        std::fs::write(t.path().join("fmt.wasm"), b"\0asm").unwrap();
        let p = Policy {
            dir: t.path().to_path_buf(),
            ..Policy::default()
        };
        assert!(admits(&p, "fmt.wasm --check src").is_ok());
        assert!(admits(&p, "X=1 ./fmt.wasm").is_ok());
        for cmd in ["cargo test", "missing.wasm", "fmt.wasm | cat"] {
            assert!(admits(&p, cmd).is_err(), "{cmd}");
        }
    }
}
