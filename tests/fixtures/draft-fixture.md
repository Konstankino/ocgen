<!-- Issue: https://github.com/acme/app/issues/12 -->
<!-- Status: Proposed -->
## Intent
The cache goes cold on every deploy, so the first request after a release
pays the whole warm-up: versioned keys keep the old entries readable
until the new ones are written.

## Tasks
- [x] Version the cache keys with the release id
- [ ] Warm the new keys from the old ones on start
- [ ] Drop the old keys a day later

## Where it shows

| Place | Today | After |
|---|---|---|
| `src/cache/keys.rs:14` | one key per entry | `<release>/<key>` |
| `src/deploy/warm.rs:3` | nothing | copies the old entries |

## Needs from
- @bob: a release id the deploy script can read
- "No approvers configured" is not the case here: see below & keep it <b>honest</b>
