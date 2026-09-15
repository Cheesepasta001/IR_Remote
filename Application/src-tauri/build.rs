fn main() {
    // `api_client.rs` bakes IR_REMOTE_BASE_URL in via `option_env!`, which is
    // read at compile time. Without this, changing the variable would not
    // invalidate the cached build and the APK would keep the old address.
    println!("cargo:rerun-if-env-changed=IR_REMOTE_BASE_URL");

    tauri_build::build()
}
