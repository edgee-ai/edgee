//! macOS menubar (status-bar) tray icon and its right-click menu.
//!
//! The tray must be created on the main thread *after* the NSApplication
//! exists, so [`build`] is called from inside eframe's app-creation closure
//! (see [`crate::app::EdgeeApp::new`]), not before `run_native`.

use anyhow::{Context, Result};
use tray_icon::menu::{Menu, MenuId, MenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

/// Owns the live tray icon plus the ids of the menu entries we react to.
/// Keep this alive for the lifetime of the app — dropping it removes the icon.
pub struct Tray {
    #[allow(dead_code)]
    tray: TrayIcon,
    pub open_id: MenuId,
    pub quit_id: MenuId,
}

pub fn build() -> Result<Tray> {
    let menu = Menu::new();
    let open = MenuItem::new("Open Edgee", true, None);
    let quit = MenuItem::new("Quit Edgee", true, None);
    menu.append(&open).context("append Open item")?;
    menu.append(&quit).context("append Quit item")?;

    let open_id = open.id().clone();
    let quit_id = quit.id().clone();

    let tray = TrayIconBuilder::new()
        .with_tooltip("Edgee")
        .with_icon(icon()?)
        .with_menu(Box::new(menu))
        .build()
        .context("build tray icon")?;

    Ok(Tray {
        tray,
        open_id,
        quit_id,
    })
}

/// A simple procedural glyph — a filled Edgee-purple disc — so the skeleton has
/// no binary assets. A real (template) icon lands with the packaging phase.
fn icon() -> Result<Icon> {
    const SIZE: u32 = 32;
    const PURPLE: [u8; 3] = [124, 92, 252];

    let center = (SIZE as f32 - 1.0) / 2.0;
    let radius = SIZE as f32 * 0.46;

    let mut rgba = vec![0u8; (SIZE * SIZE * 4) as usize];
    for y in 0..SIZE {
        for x in 0..SIZE {
            let dx = x as f32 - center;
            let dy = y as f32 - center;
            let dist = (dx * dx + dy * dy).sqrt();
            // 1px feathered edge for a less jagged disc.
            let alpha = ((radius - dist) + 0.5).clamp(0.0, 1.0);
            if alpha > 0.0 {
                let i = ((y * SIZE + x) * 4) as usize;
                rgba[i] = PURPLE[0];
                rgba[i + 1] = PURPLE[1];
                rgba[i + 2] = PURPLE[2];
                rgba[i + 3] = (alpha * 255.0) as u8;
            }
        }
    }

    Icon::from_rgba(rgba, SIZE, SIZE).context("build tray icon image")
}
