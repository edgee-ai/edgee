//! The egui application: a compact popover showing auth status, with the tray
//! menu wired to show/hide the window and quit.

use std::sync::mpsc::{self, Receiver};
use std::thread;

use eframe::egui;
use tray_icon::menu::{MenuEvent, MenuId};

use edgee_cli::config;

use crate::tray::{self, Tray};

/// Authentication snapshot derived from the active profile's credentials.
#[derive(Default)]
struct AuthState {
    logged_in: bool,
    email: Option<String>,
    org_slug: Option<String>,
}

impl AuthState {
    fn load() -> Self {
        let creds = match config::read() {
            Ok(c) => c,
            Err(_) => return Self::default(),
        };
        let logged_in = creds
            .user_token
            .as_deref()
            .map(|t| !t.is_empty())
            .unwrap_or(false);
        Self {
            logged_in,
            email: creds.email.filter(|e| !e.is_empty()),
            org_slug: creds.org_slug.filter(|s| !s.is_empty()),
        }
    }
}

pub struct EdgeeApp {
    tray: Tray,
    /// Menu clicks forwarded off the global tray-menu channel by a helper thread.
    menu_rx: Receiver<MenuId>,
    profile: String,
    auth: AuthState,
}

impl EdgeeApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> anyhow::Result<Self> {
        let tray = tray::build()?;

        // The tray-menu event channel is a single-consumer global. Drain it on a
        // helper thread, forward ids to the app, and wake the egui loop — so menu
        // clicks are handled even while the window is hidden.
        let (tx, menu_rx) = mpsc::channel();
        let ctx = cc.egui_ctx.clone();
        thread::spawn(move || {
            let receiver = MenuEvent::receiver();
            while let Ok(event) = receiver.recv() {
                if tx.send(event.id).is_err() {
                    break;
                }
                ctx.request_repaint();
            }
        });

        Ok(Self {
            tray,
            menu_rx,
            profile: config::active_profile_name(),
            auth: AuthState::load(),
        })
    }

    fn show_window(&mut self, ctx: &egui::Context) {
        self.auth = AuthState::load();
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }
}

impl eframe::App for EdgeeApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Tray menu actions.
        while let Ok(id) = self.menu_rx.try_recv() {
            if id == self.tray.open_id {
                self.show_window(ctx);
            } else if id == self.tray.quit_id {
                std::process::exit(0);
            }
        }

        // A menubar app hides on window-close instead of terminating.
        if ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }

        egui::CentralPanel::default().show(ctx, |ui| self.ui(ui));
    }
}

impl EdgeeApp {
    fn ui(&self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            ui.heading("Edgee");
        });
        ui.add_space(4.0);
        ui.separator();
        ui.add_space(8.0);

        // Account section.
        if self.auth.logged_in {
            let who = self.auth.email.as_deref().unwrap_or("Logged in");
            ui.horizontal(|ui| {
                ui.colored_label(egui::Color32::from_rgb(64, 192, 96), "●");
                ui.label(egui::RichText::new(who).strong());
            });
            if let Some(org) = &self.auth.org_slug {
                ui.label(egui::RichText::new(format!("org · {org}")).weak());
            }
            ui.label(egui::RichText::new(format!("profile · {}", self.profile)).weak());
        } else {
            ui.horizontal(|ui| {
                ui.colored_label(egui::Color32::from_rgb(220, 96, 96), "●");
                ui.label("Not logged in");
            });
            ui.label(egui::RichText::new("Run `edgee auth login` to connect.").weak());
        }

        ui.add_space(12.0);
        ui.separator();
        ui.add_space(8.0);

        // Placeholders for the next phases.
        ui.label(egui::RichText::new("Stats").strong());
        ui.label(egui::RichText::new("— coming in the next step —").weak());
        ui.add_space(12.0);
        ui.label(egui::RichText::new("Launch & Relay").strong());
        ui.label(egui::RichText::new("— coming in the next step —").weak());
    }
}
