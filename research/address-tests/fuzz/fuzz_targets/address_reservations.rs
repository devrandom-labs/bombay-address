#![no_main]
libfuzzer_sys::fuzz_target!(|data: &[u8]| address_tests::reservations::fuzz_entry(data));
