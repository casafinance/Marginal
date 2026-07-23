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

/// Minimal standard-base64 encoder. Inline (rather than pulling in a crate) so the
/// dependency set — and therefore the CI build cache — stays unchanged.
fn b64_encode(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0];
        let b1 = *chunk.get(1).unwrap_or(&0);
        let b2 = *chunk.get(2).unwrap_or(&0);
        let n = ((b0 as u32) << 16) | ((b1 as u32) << 8) | (b2 as u32);
        out.push(T[((n >> 18) & 63) as usize] as char);
        out.push(T[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 { T[((n >> 6) & 63) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[(n & 63) as usize] as char } else { '=' });
    }
    out
}

/// Read a PDF file into a JSON payload the frontend can load: { name, b64 }.
fn read_pdf(path: &str) -> Option<serde_json::Value> {
    let bytes = std::fs::read(path).ok()?;
    let name = std::path::Path::new(path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("document.pdf")
        .to_string();
    Some(serde_json::json!({ "name": name, "b64": b64_encode(&bytes) }))
}

/// Frontend calls this once on startup to get the file we were opened with (if any).
#[tauri::command]
fn take_opened_pdf(state: tauri::State<OpenedFile>) -> Option<serde_json::Value> {
    let path = state.0.lock().ok()?.take()?;
    read_pdf(&path)
}

/// Opens Windows' own "How do you want to open this file?" dialog, scoped to .pdf.
/// NOTE: this reliably works for a file type Windows has *no* handler for yet, but
/// .pdf almost always already has one (Edge, Adobe, etc.), and on some Windows builds
/// this trick silently does nothing — or reopens the current default — once a handler
/// already exists. Kept as a "quick way" that sometimes works; Settings (below) is the
/// dependable path and what the app leads with.
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

/// Opens Windows Settings straight to the Default Apps page, via the documented
/// `ms-settings:defaultapps` URI. This is the reliable path: it gets someone to the
/// right screen every time; the last couple of clicks (searching ".pdf", picking
/// Marginal) still have to be theirs, since that's the same protection that blocks
/// any silent registry trick from sticking.
#[tauri::command]
fn open_default_apps_settings() -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/C", "start", "", "ms-settings:defaultapps"])
            .spawn()
            .map_err(|e| e.to_string())?;
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
        .invoke_handler(tauri::generate_handler![take_opened_pdf, set_default_pdf_app, open_default_apps_settings])
        .run(tauri::generate_context!())
        .expect("error while running Marginal");
}
