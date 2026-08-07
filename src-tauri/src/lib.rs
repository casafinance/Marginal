use std::sync::Mutex;
use tauri::Emitter;
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
fn first_openable_file(args: &[String]) -> Option<String> {
    args.iter()
        .find(|a| {
            let lower = a.to_lowercase();
            lower.ends_with(".pdf") || lower.ends_with(".docx")
        })
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

/// Read a file the app was asked to open into a JSON payload the frontend can load:
/// { name, b64 }. A .docx is converted to PDF first — the frontend's launch-time load
/// path only ever expects PDF bytes back, same as every other path into the app.
fn read_openable_file(path: &str) -> Option<serde_json::Value> {
    let is_docx = path.to_lowercase().ends_with(".docx");
    let bytes = std::fs::read(path).ok()?;
    let stem = std::path::Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("document")
        .to_string();
    if is_docx {
        let pdf_bytes = convert_docx_bytes(&stem, &bytes).ok()?;
        // No "path" here on purpose: what we're handing back is a temporary converted
        // PDF, not the .docx on disk, so it must not become a Save target.
        Some(serde_json::json!({ "name": format!("{}.pdf", stem), "b64": b64_encode(&pdf_bytes) }))
    } else {
        let name = std::path::Path::new(path)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("document.pdf")
            .to_string();
        Some(serde_json::json!({ "name": name, "path": path, "b64": b64_encode(&bytes) }))
    }
}

/// Frontend calls this once on startup to get the file we were opened with (if any).
/// Returns Result, not a bare Option: an async command that borrows State (as this one
/// does) is required to by Tauri's command macro, since the generated future needs a
/// 'static bound that an `Option`-returning signature can't satisfy here.
#[tauri::command]
async fn take_opened_pdf(state: tauri::State<'_, OpenedFile>) -> Result<Option<serde_json::Value>, String> {
    // Async on purpose: for a .docx this runs the Word conversion, which waits on
    // PowerShell for up to 45s. On the main thread that would freeze the window at
    // startup, exactly when the person is waiting to see their document.
    let path = {
        let mut guard = state.0.lock().map_err(|e| e.to_string())?;
        guard.take()
    };
    let Some(path) = path else { return Ok(None); };
    Ok(read_openable_file(&path))
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
/// Returns the chosen path if saved (so the frontend can remember it for a future true
/// Save), or None if the person cancelled the dialog — not an error.
#[tauri::command]
async fn save_file_as(
    app: tauri::AppHandle,
    window: tauri::Window,
    default_name: String,
    filter_name: String,
    filter_ext: String,
    b64: String,
) -> Result<Option<String>, String> {
    let bytes = b64_decode(&b64)?;
    let exts: Vec<&str> = filter_ext.split(',').map(|s| s.trim()).collect();
    let mut builder = app
        .dialog()
        .file()
        .set_file_name(&default_name)
        .add_filter(&filter_name, &exts);
    // Centre the dialog over Marginal rather than letting Windows place it wherever.
    #[cfg(any(windows, target_os = "macos"))]
    {
        builder = builder.set_parent(&window);
    }
    let picked = builder.blocking_save_file();
    match picked {
        Some(file_path) => {
            let path = file_path.into_path().map_err(|e| e.to_string())?;
            std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;
            Ok(Some(path.to_string_lossy().to_string()))
        }
        None => Ok(None), // person cancelled — not an error
    }
}

/// True "Save" — writes straight to an already-known path, no dialog. Only ever called
/// with a path this session already produced (via save_file_as or the native Open
/// dialog), so this doesn't need its own picker or validation beyond the write itself.
#[tauri::command]
async fn write_to_path(path: String, b64: String) -> Result<(), String> {
    let bytes = b64_decode(&b64)?;
    std::fs::write(&path, &bytes).map_err(|e| e.to_string())
}

/// Shows a native "Open" dialog (multi-select) and returns each chosen file's real path
/// plus its bytes — the path is what a future "Save" (vs. "Save As") would write back to,
/// which the plain HTML file input can never provide (browsers never expose a real path,
/// for the same security reasons in every browser, Tauri's webview included).
#[tauri::command]
async fn open_files_dialog(
    app: tauri::AppHandle,
    window: tauri::Window,
) -> Result<Vec<serde_json::Value>, String> {
    let mut builder = app
        .dialog()
        .file()
        .add_filter("PDF and Word Documents", &["pdf", "docx"]);
    #[cfg(any(windows, target_os = "macos"))]
    {
        builder = builder.set_parent(&window);
    }
    let picked = builder.blocking_pick_files();
    let mut out = Vec::new();
    if let Some(paths) = picked {
        for file_path in paths {
            let path = file_path.into_path().map_err(|e| e.to_string())?;
            let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("document").to_string();
            out.push(serde_json::json!({
                "name": name,
                "path": path.to_string_lossy().to_string(),
                "b64": b64_encode(&bytes),
            }));
        }
    }
    Ok(out)   // empty vec if the person cancelled — not an error
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

/// Runs a child process but doesn't wait forever — Word's COM automation in particular
/// is known to hang indefinitely if it hits an unexpected dialog (format warnings,
/// update checks, etc.), and a stuck conversion shouldn't be able to freeze the app.
fn run_with_timeout(cmd: &mut std::process::Command, timeout: std::time::Duration) -> bool {
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(_) => return false,
    };
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    return false;
                }
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
            Err(_) => return false,
        }
    }
}

/// Converts docx bytes to PDF bytes via Word's own COM automation (the standard way to
/// script Word — it has no command-line export flag the way LibreOffice does). Hardened
/// against the specific things that make Word automation hang waiting for a click that
/// will never come: DisplayAlerts is off, format-conversion confirmation is off, and the
/// document is closed without prompting to save. Shared by both the in-app "Insert/Open
/// docx" flow (bytes arrive from JS) and double-click / "Open With" (bytes are read from
/// the launch path) — same conversion either way, just a different bytes source.
fn convert_docx_bytes(stem: &str, bytes: &[u8]) -> Result<Vec<u8>, String> {
    let work_dir = std::env::temp_dir().join(format!("marginal-convert-{}", std::process::id()));
    std::fs::create_dir_all(&work_dir).map_err(|e| e.to_string())?;
    let input_path = work_dir.join(format!("{}.docx", stem));
    let output_path = work_dir.join(format!("{}.pdf", stem));
    let cleanup = || { let _ = std::fs::remove_dir_all(&work_dir); };

    if let Err(e) = std::fs::write(&input_path, bytes) {
        cleanup();
        return Err(e.to_string());
    }

    let ps_script = format!(
        "$ErrorActionPreference = 'Stop'; \
         $word = New-Object -ComObject Word.Application; \
         $word.Visible = $false; \
         $word.DisplayAlerts = 0; \
         try {{ \
            $doc = $word.Documents.Open('{input}', $false, $false, $false); \
            $doc.Fields.Update(); \
            $doc.Repaginate(); \
            $doc.SaveAs([ref]'{output}', [ref]17); \
            $doc.Close([ref]$false) \
         }} finally {{ $word.Quit() }}",
        input = input_path.display(),
        output = output_path.display()
    );
    let mut cmd = std::process::Command::new("powershell");
    cmd.args(["-NoProfile", "-NonInteractive", "-Command", &ps_script]);
    let converted = run_with_timeout(&mut cmd, std::time::Duration::from_secs(45)) && output_path.exists();

    if !converted {
        cleanup();
        return Err("Couldn't convert that file using Word. Make sure Word can open it normally, then try again.".into());
    }

    let pdf_bytes = match std::fs::read(&output_path) {
        Ok(b) => b,
        Err(e) => { cleanup(); return Err(e.to_string()); }
    };
    cleanup();
    Ok(pdf_bytes)
}

#[tauri::command]
async fn convert_docx_to_pdf(name: String, b64: String) -> Result<serde_json::Value, String> {
    let bytes = b64_decode(&b64)?;
    let stem = std::path::Path::new(&name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("document")
        .to_string();
    let pdf_bytes = convert_docx_bytes(&stem, &bytes)?;
    Ok(serde_json::json!({ "name": format!("{}.pdf", stem), "b64": b64_encode(&pdf_bytes) }))
}

/// Opens another Marginal window by launching a second copy of the app. Each window is
/// its own process, which is what makes "one window per PDF" work with no cross-window
/// bookkeeping: Windows already hands each launch its own file argument.
#[tauri::command]
fn open_in_new_window(path: Option<String>) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut cmd = std::process::Command::new(exe);
    if let Some(p) = path {
        cmd.arg(p);
    }
    cmd.spawn().map_err(|e| e.to_string())?;
    Ok(())
}

/// Closes this window for real. The close button is intercepted (see on_window_event) so
/// unsaved work can be caught first; once the person has decided, the frontend calls this.
#[tauri::command]
fn force_close(window: tauri::Window) -> Result<(), String> {
    window.destroy().map_err(|e| e.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Path of the file this launch was asked to open (double-click in Explorer, "Open
    // With", or a path handed over by open_in_new_window). A .docx is converted to PDF
    // automatically, same as every other route into the app.
    let launched_with = first_openable_file(&std::env::args().collect::<Vec<_>>());

    tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_dialog::init())
        .manage(OpenedFile(Mutex::new(launched_with)))
        .manage(PendingUpdate(Mutex::new(None)))
        // Don't let the window close immediately -- the frontend needs a chance to warn
        // about unsaved edits. It calls force_close once the person has decided.
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.emit("close-requested", ());
            }
        })
        .invoke_handler(tauri::generate_handler![
            take_opened_pdf,
            set_default_pdf_app,
            open_default_apps_settings,
            save_file_as,
            write_to_path,
            open_files_dialog,
            open_in_new_window,
            force_close,
            check_for_update,
            install_update,
            convert_docx_to_pdf
        ])
        .run(tauri::generate_context!())
        .expect("error while running Marginal");
}
