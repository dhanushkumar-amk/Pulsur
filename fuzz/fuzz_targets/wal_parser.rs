#![no_main]
use libfuzzer_sys::fuzz_target;
use queue::{Queue, replay_wal_from_bytes};

fuzz_target!(|data: &[u8]| {
    let mut queue = Queue::new();
    let _ = replay_wal_from_bytes(data, &mut queue);
});
