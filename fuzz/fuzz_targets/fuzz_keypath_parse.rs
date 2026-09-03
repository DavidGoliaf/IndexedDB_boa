#![no_main]
use libfuzzer_sys::fuzz_target;
use boa_idb_core::key::path::KeyPath;

fuzz_target!(|data: &str| {
    let _ = KeyPath::parse(data);
});
