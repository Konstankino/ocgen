;; A WASI (preview 1) module for tests/hook_sandbox.rs: what it may do in its
;; folder. Its preopened folders: fd 3 = "/" (read-only), fd 4 = "/.claude"
;; (read-only), fd 5 = "/src" (writable). Exit 0 when the sandbox held:
;; 1 = it couldn't create src/out.txt, 2 = it truncated .claude/settings.json,
;; 3 = it created top.txt.
(module
  (import "wasi_snapshot_preview1" "path_open"
    (func $open (param i32 i32 i32 i32 i32 i64 i64 i32 i32) (result i32)))
  (import "wasi_snapshot_preview1" "proc_exit" (func $exit (param i32)))
  (memory (export "memory") 1)
  (data (i32.const 16) "out.txt")
  (data (i32.const 32) "settings.json")
  (data (i32.const 48) "top.txt")
  (func (export "_start")
    ;; oflags 1 = CREAT, 8 = TRUNC; rights 64 = fd_write; the new fd goes to 0.
    (if (i32.ne
          (call $open (i32.const 5) (i32.const 0) (i32.const 16) (i32.const 7)
            (i32.const 1) (i64.const 64) (i64.const 0) (i32.const 0) (i32.const 0))
          (i32.const 0))
      (then (call $exit (i32.const 1))))
    (if (i32.eqz
          (call $open (i32.const 4) (i32.const 0) (i32.const 32) (i32.const 13)
            (i32.const 8) (i64.const 64) (i64.const 0) (i32.const 0) (i32.const 0)))
      (then (call $exit (i32.const 2))))
    (if (i32.eqz
          (call $open (i32.const 3) (i32.const 0) (i32.const 48) (i32.const 7)
            (i32.const 1) (i64.const 64) (i64.const 0) (i32.const 0) (i32.const 0)))
      (then (call $exit (i32.const 3))))
    (call $exit (i32.const 0))))
