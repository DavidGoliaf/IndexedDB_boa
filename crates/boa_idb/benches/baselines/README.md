# Labelled-host performance baselines (M7-B P1-1)

`m7b-label-baseline.json` (created on the pinned host by the procedure
below — NOT the interim `m7b-label-ref.json`) is the release reference
for the blocking >10% gate (`scripts/bench_compare.py compare`).

## Host identity: `BOA_IDB_BENCH_HOST_ID`

The comparator binds every labelled baseline to one pinned machine via a
stable host id, and refuses to compare across machines. The id is set by
the runner owner in the runner's PROTECTED configuration — never in the
workflow file or any PR-controlled file:

- GitHub Actions self-hosted runner: add the variable to the runner
  machine's environment (e.g. `/etc/environment` on the pinned VM, or the
  `env:` block of the runner service unit), or — preferably — define a
  repository/environment variable named `BOA_IDB_BENCH_HOST_ID` and
  reference it only as `${{ vars.BOA_IDB_BENCH_HOST_ID }}` (as
  `bench-regression.yml` does). PR code cannot change `vars.*`.
- Suggested value: a non-secret inventory name, e.g.
  `boa-bench-01`. The handoff records this id (not a secret); the secret
  is the runner's registration token, which never appears in the repo.

Without the variable on the executing machine, `compare` exits 2 before
running any bench. A labelled `write` without it is likewise refused, so
an operator cannot accidentally stamp a labelled baseline from an
unidentified host.

Diagnostic fingerprint fields (`os_release`, `cpu`) are recorded in every
baseline for investigations, but they are NOT the admission criterion —
only `host_id` is.

## Initial baseline capture (on the pinned host only)

From a clean tree at the pinned commit, with `BOA_IDB_BENCH_HOST_ID`
present in the shell (inherited from the protected runner config):

```sh
BOA_IDB_BASELINE_ROLE=labelled \
python3 scripts/bench_compare.py write \
  --baseline crates/boa_idb/benches/baselines/m7b-label-baseline.json
```

Verify, then compare on the same host (must PASS, same machine, same id):

```sh
python3 scripts/bench_compare.py compare \
  --baseline crates/boa_idb/benches/baselines/m7b-label-baseline.json
```

Commit the new file (never rename or hand-edit `m7b-label-ref.json`;
it stays `interim`/diagnostic). Only then add `bench-regression` to
required checks.

## Runner provisioning (maintainer)

Register a self-hosted Linux x64 runner with the labels `self-hosted`
and `bench`, pinned OS image, disabled SMT-variance sources (fixed
frequency governor, no co-tenants), the stable Rust toolchain, and the
protected `BOA_IDB_BENCH_HOST_ID` variable above. See
`.github/workflows/bench-regression.yml`.
