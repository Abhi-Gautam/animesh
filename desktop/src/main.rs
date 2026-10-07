mod bridge;

use tauri::Manager;

fn main() {
    if std::env::args().nth(1).as_deref() == Some("--version") {
        println!("animesh-desktop {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    let paths = match animesh::paths::AppPaths::production() {
        Ok(paths) => paths,
        Err(error) => {
            eprintln!("Animesh Command Center: {error}");
            std::process::exit(2);
        }
    };
    let app = tauri::Builder::default()
        .manage(paths)
        .invoke_handler(tauri::generate_handler![
            bridge::view,
            bridge::search_titles,
            bridge::resolve_search,
            bridge::follow_title,
            bridge::drop_title,
            bridge::refresh,
            bridge::open_source,
            bridge::start_service,
            bridge::skill_status,
            bridge::install_skill,
        ])
        .setup(|app| {
            let handle = app.handle().clone();
            let paths = app.state::<animesh::paths::AppPaths>().inner().clone();
            tauri::async_runtime::spawn(bridge::watch_connection(handle, paths));
            Ok(())
        });
    if let Err(error) = app.run(tauri::generate_context!()) {
        eprintln!("Animesh Command Center: {error}");
        std::process::exit(2);
    }
}
