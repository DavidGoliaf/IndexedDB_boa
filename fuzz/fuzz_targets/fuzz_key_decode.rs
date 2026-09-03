#![no_main]
use libfuzzer_sys::fuzz_target;
use boa_idb_core::key::encode::decode_key;

fuzz_target!(|data: &[u8]| {
    let _ = decode_key(data);
});
