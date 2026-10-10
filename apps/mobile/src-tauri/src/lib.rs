//! BOZKURT CODE mobile companion: the shared React UI (packages/ui) in remote mode.
//! The first screen pairs with a desktop daemon (QR scan or host + 8-digit code → POST /remote/pair);
//! afterwards the same screens drive that daemon over the LAN with a bearer token.

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_deep_link::init());

    #[cfg(mobile)]
    let builder = builder.plugin(tauri_plugin_barcode_scanner::init());

    builder
        .run(tauri::generate_context!())
        .expect("error while running BOZKURT CODE mobile");
}
