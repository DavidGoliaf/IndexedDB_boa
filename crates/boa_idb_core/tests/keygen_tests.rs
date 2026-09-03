//! Integration tests for the key generator (§2.11).
//!
//! Covers nested savepoints and the `2^53 - 1` boundary from the outside;
//! single-step cases live in the inline unit tests of `engine::keygen`.

#![allow(clippy::float_cmp)]

use boa_idb_core::engine::keygen::KeyGenerator;

const MAX_KEY: f64 = 9_007_199_254_740_991.0;

#[test]
fn nested_savepoints_unwind_in_order() {
    let mut kg = KeyGenerator::new(1.0);
    kg.begin_request(); // L0
    assert_eq!(kg.generate().unwrap(), 1.0);
    kg.begin_request(); // L1
    assert_eq!(kg.generate().unwrap(), 2.0);
    // Committing L1 merges its effects into L0...
    kg.commit_request().unwrap();
    assert_eq!(kg.current(), 3.0);
    // ...so rolling back L0 undoes everything.
    kg.rollback_request().unwrap();
    assert_eq!(kg.current(), 1.0);
}

#[test]
fn inner_rollback_preserves_outer() {
    let mut kg = KeyGenerator::new(10.0);
    kg.begin_request(); // L0
    assert_eq!(kg.generate().unwrap(), 10.0);
    kg.begin_request(); // L1
    assert_eq!(kg.generate().unwrap(), 11.0);
    kg.rollback_request().unwrap();
    assert_eq!(kg.current(), 11.0);
    kg.commit_request().unwrap();
    assert_eq!(kg.current(), 11.0);
}

#[test]
fn max_key_boundary_from_outside() {
    let mut kg = KeyGenerator::new(MAX_KEY - 1.0);
    assert_eq!(kg.generate().unwrap(), MAX_KEY - 1.0);
    assert_eq!(kg.generate().unwrap(), MAX_KEY);
    assert!(kg.generate().is_err());
}

#[test]
fn explicit_key_below_current_is_ignored() {
    let mut kg = KeyGenerator::new(100.0);
    kg.possibly_update(42.0);
    assert_eq!(kg.current(), 100.0);
    kg.possibly_update(100.0);
    assert_eq!(kg.current(), 101.0);
}
