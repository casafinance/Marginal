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

/// Opens Windows' own "How do you want to open this file?" dialog, scoped to .pdf.
/// This is the one supported way to change the default app without admin rights —
/// Windows protects the default-app registry keys with a hidden verification hash,
/// so writing them directly doesn't stick; only a choice made through this dialog
/// (or Settings) produces a hash Windows trusts. The dialog just needs *a* .pdf path
/// to anchor to, so we point it at whatever's open, or a tiny placeholder if nothing is.
#[tauri::command]
fn set_default_pdf_app(path: Option<String>) -> Result<(), String> {
    let anchor = match path {
        Some(p) if std::path::Path::new(&p).exists() => p,
        _ => {
            let tmp = std::env::temp_dir().join("Marginal-SetDefault.pdf");
            if !tmp.exists() {
                std::fs::write(&tmp, b"%PDF-1.4\n%%EOF").map_err(|e| e.to_string())?;
            }
            tmp.to_string_lossy().into_owned()
        }
    };
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("rundll32.exe")
            .args(["shell32.dll,OpenAs_RunDLL", &anchor])
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = anchor; // no-op on other platforms
    }
    Ok(())
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
        .invoke_handler(tauri::generate_handler![take_opened_pdf, set_default_pdf_app])
        .run(tauri::generate_context!())
        .expect("error while running Marginal");
}
