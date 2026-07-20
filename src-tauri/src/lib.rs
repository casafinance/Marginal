use std::sync::Mutex;
use tauri::{Emitter, Manager};

/// Holds the path of a PDF the app was launched with (via double-click / "open with"),
/// until the frontend asks for it.
struct OpenedFile(Mutex<Option<String>>);

/// Pull the first `.pdf` path out of a set of command-line args, if any.
fn first_pdf(args: &[String]) -> Option<String> {
    args.iter()
        .find(|a| a.to_lowercase().ends_with(".pdf"))
        .cloned()
}

/// Read a PDF file into a JSON payload the frontend can load: { name, bytes }.
fn read_pdf(path: &str) -> Option<serde_json::Value> {
    let bytes = std::fs::read(path).ok()?;
    let name = std::path::Path::new(path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("document.pdf")
        .to_string();
    Some(serde_json::json!({ "name": name, "bytes": bytes }))
}

/// Frontend calls this once on startup to get the file we were opened with (if any).
#[tauri::command]
fn take_opened_pdf(state: tauri::State<OpenedFile>) -> Option<serde_json::Value> {
    let path = state.0.lock().ok()?.take()?;
    read_pdf(&path)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Path of the PDF this launch was asked to open (double-click in Explorer, etc.)
    let launched_with = first_pdf(&std::env::args().collect::<Vec<_>>());

    tauri::Builder::default()
        // If Marginal is already running and Windows opens another PDF with it,
        // that second launch forwards its args here instead of starting a new window.
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            if let Some(path) = first_pdf(&argv) {
                if let Some(payload) = read_pdf(&path) {
                    let _ = app.emit("open-pdf", payload);
                }
            }
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.set_focus();
            }
        }))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .manage(OpenedFile(Mutex::new(launched_with)))
        .invoke_handler(tauri::generate_handler![take_opened_pdf])
        .run(tauri::generate_context!())
        .expect("error while running Marginal");
}
