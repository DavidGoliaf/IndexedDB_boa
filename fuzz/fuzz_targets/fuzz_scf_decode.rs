#![no_main]
use libfuzzer_sys::fuzz_target;
use boa_idb_core::clone::decode::decode_scf;
use boa_idb_core::limits::LimitConfig;

fuzz_target!(|data: &[u8]| {
    let _ = decode_scf(data, &LimitConfig::default());
});
