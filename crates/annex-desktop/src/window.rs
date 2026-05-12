//! Windows-only window chrome and WebView2 permission handling.
//!
//! `setup_webview2_media_permissions` registers a `PermissionRequested`
//! handler that explicitly allows Camera, Microphone, and ClipboardRead so
//! `getUserMedia()` doesn't silently return `NotAllowedError`. Without it the
//! WebView2 default for these permissions is "deny silently".
//!
//! `set_dark_window_border` flips DWM attributes so the title bar and border
//! match the rest of the Tauri-rendered chrome (dark on Windows 10 20H1+,
//! pure black on Windows 11 22000+). Both calls are no-ops on older Windows
//! builds — the HRESULT is logged but otherwise ignored.

/// Force the window to use dark mode chrome and a black border.
///
/// Register a WebView2 `PermissionRequested` handler so that camera,
/// microphone, and related media-capture permissions are explicitly allowed
/// instead of relying on WebView2's default (which silently denies them).
///
/// Handled permission types (WebView2 `COREWEBVIEW2_PERMISSION_KIND` enum):
///   - Camera (3)
///   - Microphone (4)
///   - ClipboardRead (6)
///
/// All other permission requests are left at the default state.
///
/// This must be called during setup, before getUserMedia() is invoked from
/// the frontend. Without this handler, WebView2 silently returns
/// `NotAllowedError` for media capture.
#[cfg(target_os = "windows")]
pub(crate) fn setup_webview2_media_permissions(window: &tauri::WebviewWindow) {
    let result = window.with_webview(|wv| {
        unsafe {
            use webview2_com::Microsoft::Web::WebView2::Win32::*;
            use webview2_com::PermissionRequestedEventHandler;

            let webview = match wv.controller().CoreWebView2() {
                Ok(wv2) => wv2,
                Err(e) => {
                    tracing::warn!("could not access CoreWebView2 — media permission handler not installed: {e}");
                    return;
                }
            };

            // Permission kind constants from WebView2 SDK:
            // Camera = 3, Microphone = 4, ClipboardRead = 6
            let handler = PermissionRequestedEventHandler::create(Box::new(
                move |_sender, args| -> windows::core::Result<()> {
                    if let Some(args) = args {
                        let mut kind = COREWEBVIEW2_PERMISSION_KIND::default();
                        args.PermissionKind(&mut kind)?;
                        let mut uri = windows_core::PWSTR::null();
                        args.Uri(&mut uri)?;
                        let uri_str = if uri.is_null() {
                            String::new()
                        } else {
                            uri.to_string().unwrap_or_default()
                        };

                        // COREWEBVIEW2_PERMISSION_KIND values:
                        // Camera = 3, Microphone = 4, ClipboardRead = 6
                        if kind.0 == 3 || kind.0 == 4 || kind.0 == 6 {
                            let kind_name = match kind.0 {
                                3 => "Camera",
                                4 => "Microphone",
                                6 => "ClipboardRead",
                                _ => "Unknown",
                            };
                            tracing::info!(kind = kind_name, uri = %uri_str, "WebView2 permission allowed");
                            args.SetState(COREWEBVIEW2_PERMISSION_STATE_ALLOW)?;
                        } else {
                            tracing::debug!(kind = kind.0, uri = %uri_str, "WebView2 permission left at default");
                        }
                    }
                    Ok(())
                },
            ));

            let mut token: i64 = 0;
            match webview.add_PermissionRequested(&handler, &mut token) {
                Ok(()) => {
                    tracing::info!("WebView2 PermissionRequested handler installed for Camera/Microphone/ClipboardRead");
                }
                Err(e) => {
                    tracing::warn!("failed to register PermissionRequested handler: {e}");
                }
            }
        }
    });
    if let Err(e) = result {
        tracing::warn!("with_webview failed for media permission setup: {e}");
    }
}

/// Two DWM attributes are set:
///   1. `DWMWA_USE_IMMERSIVE_DARK_MODE` (20) — forces the title bar and
///      window border to use dark-mode colors regardless of the system
///      theme.  Available on Windows 10 20H1 (build 18985) and later.
///      Without this, Windows uses the user's system accent color for
///      the border (which may be orange, blue, etc.).
///   2. `DWMWA_BORDER_COLOR` (34) — overrides the border to pure black.
///      Only available on Windows 11 build 22000+.  Ignored on Win10.
///
/// Both calls are harmless if unsupported — the HRESULT is logged but
/// does not affect the application.
#[cfg(target_os = "windows")]
pub(crate) fn set_dark_window_border(window: &tauri::WebviewWindow) {
    #[link(name = "dwmapi")]
    extern "system" {
        fn DwmSetWindowAttribute(
            hwnd: isize,
            dw_attribute: u32,
            pv_attribute: *const std::ffi::c_void,
            cb_attribute: u32,
        ) -> i32;
    }

    use raw_window_handle::HasWindowHandle;
    let handle = match window.window_handle() {
        Ok(h) => h,
        Err(e) => {
            eprintln!("failed to get window handle for border color: {e}");
            return;
        }
    };
    let hwnd = match handle.as_raw() {
        raw_window_handle::RawWindowHandle::Win32(h) => h.hwnd.get() as isize,
        _ => return,
    };

    // SAFETY: hwnd is a valid window handle obtained from Tauri.
    // Both calls pass correctly-sized u32 values and are harmless if the
    // attribute is unsupported on the current Windows version.
    unsafe {
        // 1. Dark mode title bar + border (Windows 10 20H1+).
        const DWMWA_USE_IMMERSIVE_DARK_MODE: u32 = 20;
        let enabled: u32 = 1;
        let hr = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            std::ptr::addr_of!(enabled).cast(),
            std::mem::size_of::<u32>() as u32,
        );
        if hr != 0 {
            eprintln!("DwmSetWindowAttribute(DWMWA_USE_IMMERSIVE_DARK_MODE) returned 0x{hr:08X}");
        }

        // 2. Override border to pure black (Windows 11 22000+).
        const DWMWA_BORDER_COLOR: u32 = 34;
        let black: u32 = 0x00000000; // COLORREF 0x00BBGGRR
        let hr = DwmSetWindowAttribute(
            hwnd,
            DWMWA_BORDER_COLOR,
            std::ptr::addr_of!(black).cast(),
            std::mem::size_of::<u32>() as u32,
        );
        if hr != 0 {
            // Expected on Windows 10 where DWMWA_BORDER_COLOR is unsupported.
            eprintln!("DwmSetWindowAttribute(DWMWA_BORDER_COLOR) returned 0x{hr:08X}");
        }
    }
}
