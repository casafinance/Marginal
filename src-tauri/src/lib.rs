use std::sync::Mutex;
use tauri::{Emitter, Manager};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_updater::UpdaterExt;

/// Holds the path of a PDF the app was launched with (via double-click / "open with"),
/// until the frontend asks for it.
struct OpenedFile(Mutex<Option<String>>);

/// Holds an update found by `check_for_update`, until `install_update` consumes it.
/// (The frontend can't hold a reference to Rust's `Update` object across the two calls,
/// so we keep it here between them.)
struct PendingUpdate(Mutex<Option<tauri_plugin_updater::Update>>);

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

/// The matching decoder — verified against Python's base64 module across every length
/// and padding case before shipping. Used for "Save As": the frontend sends file bytes
/// as base64 (compact, one JSON string) rather than a JSON array of numbers.
fn b64_decode(s: &str) -> Result<Vec<u8>, String> {
    fn val(c: u8) -> Option<u32> {
        match c {
            b'A'..=b'Z' => Some((c - b'A') as u32),
            b'a'..=b'z' => Some((c - b'a' + 26) as u32),
            b'0'..=b'9' => Some((c - b'0' + 52) as u32),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let s = s.trim_end_matches('=');
    let mut out = Vec::with_capacity(s.len() * 3 / 4 + 3);
    let mut buf: u32 = 0;
    let mut bits: u32 = 0;
    for &c in s.as_bytes() {
        let v = val(c).ok_or_else(|| "invalid base64 character in payload".to_string())?;
        buf = (buf << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xFF) as u8);
        }
    }
    Ok(out)
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

/// Shows a native "Save As" dialog and writes the given bytes to wherever the person
/// chooses. Calls the dialog plugin's Rust API directly rather than going through its
/// JS package — that package only ships as an ES module with no plain-<script> global,
/// so `window.__TAURI__.dialog` doesn't actually exist without a JS bundler (verified
/// against Tauri's own source: the withGlobalTauri bundle only exposes
/// app/core/dpi/event/image/menu/mocks/path/tray/webview/webviewWindow/window).
/// Returns true if saved, false if the person cancelled the dialog (not an error).
#[tauri::command]
fn save_file_as(
    app: tauri::AppHandle,
    default_name: String,
    filter_name: String,
    filter_ext: String,
    b64: String,
) -> Result<bool, String> {
    let bytes = b64_decode(&b64)?;
    let exts: Vec<&str> = filter_ext.split(',').map(|s| s.trim()).collect();
    let picked = app
        .dialog()
        .file()
        .set_file_name(&default_name)
        .add_filter(&filter_name, &exts)
        .blocking_save_file();
    match picked {
        Some(file_path) => {
            let path = file_path.into_path().map_err(|e| e.to_string())?;
            std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;
            Ok(true)
        }
        None => Ok(false), // person cancelled — not an error
    }
}

/// Checks for a newer release. Same "call the Rust API directly" fix as save_file_as —
/// `window.__TAURI__.updater` doesn't exist for the same reason. Stashes the found
/// Update in app state so `install_update` can use it without round-tripping the whole
/// object through JSON.
#[tauri::command]
async fn check_for_update(
    app: tauri::AppHandle,
    state: tauri::State<'_, PendingUpdate>,
) -> Result<Option<serde_json::Value>, String> {
    let updater = app.updater().map_err(|e| e.to_string())?;
    let found = updater.check().await.map_err(|e| e.to_string())?;
    let mut slot = state.0.lock().map_err(|e| e.to_string())?;
    match found {
        Some(update) => {
            let info = serde_json::json!({ "version": update.version });
            *slot = Some(update);
            Ok(Some(info))
        }
        None => {
            *slot = None;
            Ok(None)
        }
    }
}

/// Downloads and installs the update found by check_for_update. On Windows the
/// installer restarts the app automatically once done (that's the plugin's own
/// default) — nothing else needs to happen here after a successful install.
#[tauri::command]
async fn install_update(state: tauri::State<'_, PendingUpdate>) -> Result<(), String> {
    let update = state.0.lock().map_err(|e| e.to_string())?.take();
    match update {
        Some(update) => update
            .download_and_install(|_chunk, _total| {}, || {})
            .await
            .map_err(|e| e.to_string()),
        None => Err("No update ready to install — check for one first.".into()),
    }
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
        .plugin(tauri_plugin_dialog::init())
        .manage(OpenedFile(Mutex::new(launched_with)))
        .manage(PendingUpdate(Mutex::new(None)))
        .invoke_handler(tauri::generate_handler![
            take_opened_pdf,
            set_default_pdf_app,
            open_default_apps_settings,
            save_file_as,
            check_for_update,
            install_update
        ])
        .run(tauri::generate_context!())
        .expect("error while running Marginal");
}
