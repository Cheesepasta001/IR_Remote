// Wiring only. Everything lives in the library so that `src-tauri/tests/` can
// exercise it: state.rs (pure), api_client.rs (HTTP), auth.rs (keychain),
// poller.rs (background probe), core.rs (shared state).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    ir_remote_lib::run()
}
