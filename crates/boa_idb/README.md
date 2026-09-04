# boa_idb

The public IndexedDB binding crate. It exposes the JavaScript API and connects
Boa values and events to the core storage engine.

Limitations and backend behavior are defined by the workspace specification;
backend-specific persistence lives in the sibling crates. Run its tests with
`cargo test -p boa_idb`.
