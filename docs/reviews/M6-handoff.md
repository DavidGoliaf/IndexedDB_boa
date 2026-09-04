# Handoff: M6 — Structured clone hardening and dependency hygiene

Work order: `tasks/06_TASK_STRUCTURED_CLONE_HARDENING_AND_DEPENDENCY_HYGIENE.md`  
Branch: `task/m6-structured-clone-hardening`

## Delivered

- Replaced the forgeable JS symbol brand (`__boa_platform_clone_brand` on
  instances) with a Rust-side `PlatformCloneRegistry` stored in Context host
  data: a `WeakMap` for cloneable WPT shims and a `WeakSet` for
  `MessageChannel` / `MessagePort`.
- Shim constructors call a temporary native registrar
  (`__boa_register_platform_clone`) that is sealed after bootstrap; user script
  cannot brand forged objects afterwards.
- Registry checks use method callables captured at install time, so later
  `WeakMap`/`WeakSet` prototype poisoning cannot forge membership.
- Native DOM `Event` rejection uses unforgeable `EventDataHelper` JsData
  downcast instead of `instanceof` / constructor-name heuristics.
- Plain objects that only forge prototype, constructor, or
  `constructor.name` no longer receive platform clone behavior or platform
  `DataCloneError`.
- Added regression tests in
  `crates/boa_idb_wpt/tests/platform_clone_hardening_tests.rs` that use the
  same `environment::prepare` path as the WPT runner.
- Removed unused `ISC` and `CC0-1.0` entries from `deny.toml`. `MPL-2.0` and
  `Unicode-3.0` remain per ADR-008. No `Cargo.lock` or dependency version
  changes.

## Verification

The following local checks passed:

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test -p boa_idb --tests
cargo test -p boa_idb_wpt --tests
cargo test --workspace
cargo doc --workspace --no-deps
cargo run -p boa_idb_wpt --bin boa-idb-wpt -- --backend memory --timeout 30 --quiet --check-expectations
cargo run -p boa_idb_wpt --bin boa-idb-wpt -- --backend sqlite --timeout 30 --quiet --check-expectations
$env:CARGO_DENY_DB_PATH = Join-Path $PWD "target\cargo-deny-advisories"
cargo deny fetch db
cargo deny check
```

Results:

- Full WPT strict checks on memory and SQLite: 482 matched, 0 unexpected,
  482/482 PASS, exit code 0.
- `cargo deny check`: `advisories ok`, `bans ok`, `licenses ok`, `sources ok`.
  No `license-not-encountered`. Remaining `[duplicate]` warnings for
  transitive multi-version crates are observed debt and intentionally out of
  scope for this task.
- Platform-clone hardening suite: 7/7 PASS.

## Decisions and deviations

No new dependency, SCF-v1 format change, or public IDB API change. No ADR was
required: identity moved from a JS-visible symbol property to Context-local
registry membership granted only by the sealed native registrar, matching the
existing `IDBKeyRange` WeakSet identity pattern.

Boa 0.22 provides sufficient isolation for this design: after
`seal_platform_clone_registry`, neither `globalThis.__boa_register_platform_clone`
nor residual brand symbols on shim instances are available to forge platform
clone behavior. No entry was needed in `QUESTIONS.md`.

## Scope exclusions (unchanged)

- WPT subset expansion
- Boa / rusqlite upgrades
- Unifying duplicate transitive dependency versions
- SCF-v1 format changes
- New browser API surface
