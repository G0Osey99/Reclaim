//! In-crate UniFFI bindgen driver. `scripts/build-app.sh` runs this against the
//! compiled library to emit the Swift bindings:
//!   cargo run --features cli --bin uniffi-bindgen -- \
//!     generate --library <libreclaim_ffi.dylib> --language swift --out-dir <dir>

fn main() {
    uniffi::uniffi_bindgen_main()
}
