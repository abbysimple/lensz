//! Lensz in the notification area (system tray): clicking the icon opens the
//! window, the menu switches the camera or quits, and closing the window keeps Lensz (and its
//! camera) running there. Also keeps Lensz to one instance: starting it again shows the window.

use std::sync::mpsc::{Receiver, Sender, channel};

use eframe::egui;
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError, HANDLE};
use windows::Win32::System::Threading::{CreateEventW, CreateMutexW, OpenEventW, EVENT_MODIFY_STATE, INFINITE, SetEvent, WaitForSingleObject};
use windows::core::w;

pub enum TrayCmd {
    Show,
    ToggleVirtualCamera,
    Quit,
}

pub struct Tray {
    _icon: TrayIcon,
    vcam_item: CheckMenuItem,
    pub cmds: Receiver<TrayCmd>,
}

impl Tray {
    /// Must be created on the UI thread (its window messages are pumped by the event loop).
    pub fn new(ctx: &egui::Context, vcam_on: bool, show_signal: Option<HANDLE>) -> Option<Self> {
        let (tx, cmds) = channel();
        let open = MenuItem::new("Open Lensz", true, None);
        let vcam_item = CheckMenuItem::new("Virtual Camera", true, vcam_on, None);
        let quit = MenuItem::new("Quit Lensz", true, None);
        let menu = Menu::new();
        menu.append_items(&[&open, &vcam_item, &PredefinedMenuItem::separator(), &quit]).ok()?;

        let app = crate::icons::app_icon();
        let icon = Icon::from_rgba(app.rgba, app.width, app.height).ok()?;
        let tray = TrayIconBuilder::new()
            .with_tooltip("Lensz")
            .with_icon(icon)
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .build()
            .ok()?;

        let send = |tx: &Sender<TrayCmd>, ctx: &egui::Context, cmd| {
            let _ = tx.send(cmd);
            ctx.request_repaint();
        };
        let (tx2, ctx2) = (tx.clone(), ctx.clone());
        TrayIconEvent::set_event_handler(Some(move |e| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
                send(&tx2, &ctx2, TrayCmd::Show);
            }
        }));
        let (tx3, ctx3) = (tx.clone(), ctx.clone());
        let (open_id, vcam_id, quit_id) = (open.id().clone(), vcam_item.id().clone(), quit.id().clone());
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            if e.id == open_id {
                send(&tx3, &ctx3, TrayCmd::Show);
            } else if e.id == vcam_id {
                send(&tx3, &ctx3, TrayCmd::ToggleVirtualCamera);
            } else if e.id == quit_id {
                send(&tx3, &ctx3, TrayCmd::Quit);
            }
        }));

        // A second Lensz started: show this one.
        if let Some(ev) = show_signal {
            let (tx4, ctx4) = (tx.clone(), ctx.clone());
            let ev = ev.0 as isize;
            std::thread::spawn(move || {
                loop {
                    if unsafe { WaitForSingleObject(HANDLE(ev as _), INFINITE) }.0 != 0 {
                        break;
                    }
                    send(&tx4, &ctx4, TrayCmd::Show);
                }
            });
        }
        Some(Self { _icon: tray, vcam_item, cmds })
    }

    /// Keep the menu's check mark in step with the window's switch.
    pub fn set_virtual_camera(&self, on: bool) {
        self.vcam_item.set_checked(on);
    }
}

/// Claim the single Lensz instance. Returns the "show yourself" event to listen on, or None if
/// Lensz is already running (it has then been asked to show its window; this process should exit).
pub fn single_instance() -> Option<HANDLE> {
    unsafe {
        let _mutex = CreateMutexW(None, false, w!("Local\\Lensz_single_instance"));
        if GetLastError() == ERROR_ALREADY_EXISTS {
            if let Ok(ev) = OpenEventW(EVENT_MODIFY_STATE, false, w!("Local\\Lensz_show")) {
                let _ = SetEvent(ev);
            }
            return None;
        }
        // The mutex handle stays open (leaked) for the life of the process.
        CreateEventW(None, false, false, w!("Local\\Lensz_show")).ok()
    }
}
