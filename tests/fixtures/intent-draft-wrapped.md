## Intent

`.claude/` is untracked, so a clone or `--worktree` session runs with no approval, plan, confidence
or idle gate, while the tracked `CLAUDE.md` states the gates are
active — Critical (F1). Evidence and full
findings: `docs/adr/ADR-1-cache.md`.

High: the generator's only input is untracked (F4); the
approval gate's `aws` branch is incomplete (F8);
`CLAUDE.md` contradicts the rules (F7). Medium: ungated `.git/config`,
`WebSearch` past the fetch guard, *a no-op idle
gate*, six more.

**Blocks prod?** No — nothing has been deployed.
**Depends on the platform view?** Yes — F2 covers S3 and KMS deletion.

## Options and trade-offs

- **A. Commit `.claude/` in full.** Generated artifacts in history; the
  only option closing F1 and F4.
- **Do nothing.** Worktree sessions keep running ungated (F1).

> Quoted from the review: a gate that is wrapped
> by hand reads as two lines.

Deploys keep the cache warm.
Links: docs/adr/ADR-0001-cache.md
See also
https://example.com/doc

Press <kbd>Ctrl</kbd>+<kbd>S</kbd> to save.<br>
A second line after an HTML break.
Two spaces end this one  
and a backslash ends this one\
so neither is a soft break.

```text
code is
never joined
```

## Needs from @alice
- [ ] @alice — pending

<details><summary>Plan</summary>

- **Changes:** templates, code, and tests touched
</details>
