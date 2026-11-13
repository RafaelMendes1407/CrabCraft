#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() > 16 * 1024 {
        return;
    }
    let _ = serde_json::from_slice::<rc_core::session::Session>(data);
    if let Ok(text) = std::str::from_utf8(data) {
        let _ = rc_core::session::parse_env(text);
    }
});
