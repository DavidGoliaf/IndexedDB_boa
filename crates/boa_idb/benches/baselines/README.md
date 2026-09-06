# Labelled-host performance baselines (M7-B P1-5)

`m7b-label-ref.json` is the checked-in criterion-means reference for the
blocking >10% regression gate (`scripts/bench_compare.py compare`).

## Host roles

- `labelled`: the pinned, immutable-config benchmark host (dedicated,
  quiet, fixed OS/CPU/storage/Rust/profile). Only measurements from this
  host are release evidence, and only this host runs the blocking gate.
- `interim`: any other machine (like the one that produced the current
  file). Used for harness validation and diagnostics — never enforcement.

The current file is **interim** (loaded Windows dev box; back-to-back runs
vary 1.1–1.8x there, which is exactly why enforcement needs the pinned
host).

## (Re-)baselining the labelled host

On the pinned host, from a clean tree at the pinned commit:

```sh
BOA_IDB_BASELINE_ROLE=labelled \
python3 scripts/bench_compare.py write \
  --baseline crates/boa_idb/benches/baselines/m7b-label-ref.json
```

Commit the result. The nightly/CI `bench-regression` job (self-hosted
`bench` label) then enforces it: any scenario whose fresh mean exceeds
1.10x the reference fails the check.

## Runner provisioning (maintainer)

Register a self-hosted Linux x64 runner with the labels `self-hosted`
and `bench`, pinned OS image, disabled SMT-variance sources (fixed
frequency governor, no co-tenants), and the stable Rust toolchain. See
`.github/workflows/bench-regression.yml`.
