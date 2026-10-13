fn main() {
    // cosmwasm-std 2.x declares host imports (db_next, ...) as plain `extern "C"`.
    // Newer rustc no longer passes --allow-undefined to rust-lld for wasm targets,
    // so these must be explicitly allowed to become wasm imports.
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("wasm32") {
        println!("cargo:rustc-link-arg-cdylib=--allow-undefined");
    }
}
