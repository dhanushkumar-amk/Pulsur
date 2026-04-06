#![no_main]
use libfuzzer_sys::fuzz_target;
use http_server::parse_header_section;

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        let _ = parse_header_section(s);
    }
});
