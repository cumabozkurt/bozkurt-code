// Desktop entry point (only for `tauri dev` on a computer); phones use the mobile entry point in lib.rs.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    bozkurt_mobile_lib::run()
}
