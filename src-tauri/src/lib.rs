//! GameVault core library.

pub mod commands;
pub mod db;
pub mod meta;
pub mod platform;
pub mod scan;
pub mod transfer;

/// Build and run the desktop application.
pub fn run() {
    let db = db::Db::open(&db::Db::default_path())
        .expect("could not open the catalogue database");

    tauri::Builder::default()
        .manage(commands::AppState { db: std::sync::Mutex::new(db) })
        .invoke_handler(tauri::generate_handler![
            commands::refresh_drives,
            commands::scan_path,
            commands::list_items,
            commands::set_verdict,
            commands::set_title,
            commands::list_duplicates,
            commands::storage_breakdown,
            commands::catalog_path,
            commands::transfer::preflight_transfer,
            commands::transfer::start_transfer,
            commands::transfer::cancel_transfer,
            commands::transfer::pause_transfer,
            commands::transfer::get_item,
            commands::transfer::reveal_item,
            commands::meta::credential_status,
            commands::meta::save_credentials,
            commands::meta::clear_credentials,
            commands::meta::enrich_library,
            commands::meta::lock_metadata,
            commands::console::read_console_dir,
            commands::reset_app_data,
        ])
        .run(tauri::generate_context!())
        .expect("error while running GameVault");
}
