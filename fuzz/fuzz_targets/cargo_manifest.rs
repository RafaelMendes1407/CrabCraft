#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() > 16 * 1024 {
        return;
    }
    if let Ok(text) = std::str::from_utf8(data) {
        let _ = rc_cargo::inspect::inspect(text, None);
    }
});
