#![no_main]
use libfuzzer_sys::fuzz_target;
use boa_idb_fs::{decode_frame, recover_committed_frames};

fuzz_target!(|data: &[u8]| {
    // WAL recovery state machine over arbitrary bytes: torn frames,
    // CONTINUES/COMMIT chains, sequence gaps and garbage must never panic;
    // recovery reports the committed prefix, nothing more.
    let recovered = recover_committed_frames(data);
    for frame in &recovered.frames {
        for op in &frame.ops {
            std::hint::black_box(op);
        }
    }
    // A successfully decoded first frame never consumes past the input.
    if let Ok(Some((_, n))) = decode_frame(data) {
        assert!(n <= data.len());
    }
});
