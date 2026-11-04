//! Tray icon and menu.

use tauri::AppHandle;
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

use crate::window;

/// Fails when this desktop can't show one; never panics.
pub fn create(app: &AppHandle) -> anyhow::Result<()> {
    #[cfg(target_os = "linux")]
    if !indicator_library_installed() {
        anyhow::bail!("install libayatana-appindicator3 to get one");
    }
    let open = MenuItem::with_id(app, "open", "Open OmniPanel", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit OmniPanel", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &PredefinedMenuItem::separator(app)?, &quit])?;
    let macos = cfg!(target_os = "macos");
    let icon = if macos { Some(template_icon()) } else { app.default_window_icon().cloned() };
    let mut tray = TrayIconBuilder::with_id("main")
        .tooltip("OmniPanel")
        .menu(&menu)
        .icon_as_template(macos)
        .show_menu_on_left_click(macos)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" => window::show(app),
            "quit" => crate::request_quit(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                window::show(tray.app_handle());
            }
        });
    if let Some(icon) = icon {
        tray = tray.icon(icon);
    }
    tray.build(app)?;
    Ok(())
}

/// The tray crate loads this library at runtime and panics when it's missing,
/// which would abort the app (`panic = "abort"`). Same names it tries.
#[cfg(target_os = "linux")]
fn indicator_library_installed() -> bool {
    use std::ffi::{CStr, c_char, c_int, c_void};
    const RTLD_LAZY: c_int = 1;
    unsafe extern "C" {
        fn dlopen(file: *const c_char, flags: c_int) -> *mut c_void;
    }
    let names: [&CStr; 4] = [
        c"libayatana-appindicator3.so.1",
        c"libappindicator3.so.1",
        c"libayatana-appindicator3.so",
        c"libappindicator3.so",
    ];
    // SAFETY: the names are NUL-terminated. The handle stays open, the tray crate uses the same library.
    names.iter().any(|name| !unsafe { dlopen(name.as_ptr(), RTLD_LAZY) }.is_null())
}

/// The logo's 3x3 key grid as a 44x44 macOS template image (alpha only).
fn template_icon() -> Image<'static> {
    const SIZE: usize = 44;
    const KEY: usize = 11;
    const GAP: usize = 4;
    const RADIUS: f32 = 2.5;
    let origin = (SIZE - (3 * KEY + 2 * GAP)) / 2;
    let mut rgba = vec![0u8; SIZE * SIZE * 4];
    for row in 0..3 {
        for col in 0..3 {
            let alpha = if row == 1 && col == 1 { 255 } else { 150 };
            let (x0, y0) = (origin + col * (KEY + GAP), origin + row * (KEY + GAP));
            for y in 0..KEY {
                for x in 0..KEY {
                    // distance outside the rounded corner, for a soft edge
                    let dx = (RADIUS - x as f32 - 0.5).max(x as f32 + 0.5 - (KEY as f32 - RADIUS)).max(0.0);
                    let dy = (RADIUS - y as f32 - 0.5).max(y as f32 + 0.5 - (KEY as f32 - RADIUS)).max(0.0);
                    let coverage = (RADIUS + 0.5 - (dx * dx + dy * dy).sqrt()).clamp(0.0, 1.0);
                    let i = ((y0 + y) * SIZE + x0 + x) * 4;
                    rgba[i + 3] = (alpha as f32 * coverage) as u8;
                }
            }
        }
    }
    Image::new_owned(rgba, SIZE as u32, SIZE as u32)
}
