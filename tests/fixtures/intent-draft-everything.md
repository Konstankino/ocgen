## Intent
<!-- 2-3 lines. What should be true afterwards. -->
Deploys keep the cache warm, so the first requests after a release stay fast.
Links: [ADR-0007](docs/adr/ADR-0007-cache.md), follows up #123 and owner/repo#45.

**Blocks prod?** No
**Depends on the platform view** (what we need to protect)? Yes — the _read path_ and *write path*.

Press <kbd>Ctrl</kbd>+<kbd>S</kbd> to save.<br>
A second line after an HTML break, and ~~an old idea~~.

## Options and trade-offs
1. **A. Version the keys** — cheap; a stale read for one TTL.
2. **B. Warm on deploy** — see <https://example.com/warm> and https://example.com/bare.
3. **Do nothing.** Cold cache after each deploy.

| Option | Cost | Risk |
|--------|-----:|:----:|
| A      | low  | Medium |
| B      | high | Low  |

> Quoted from the incident review: cold caches cost 40 s of p99.

```rust
fn warm(keys: &[Key]) -> Result<()> {
    keys.iter().try_for_each(|k| cache.touch(k))
}
```

* A bullet written with a star
* and a second one with `inline code`

![latency chart](https://example.com/p99.png)

## Needs from @alice and @bob-smith
- [ ] @alice — pending
- [x] @bob-smith — approved on 2026-10-01
- [ ] No reply yet from @carol_dev

<!--
A comment over
three lines.
-->

<details><summary>Plan (fill in once the intent is agreed)</summary>

- **Changes:** templates, code, and tests touched
- **How we know it worked:** p99 after deploy under 200 ms
- **Revert:** `git revert` the [change][pr]
</details>

[pr]: https://github.com/owner/repo/pull/7
