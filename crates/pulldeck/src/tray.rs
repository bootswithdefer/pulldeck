//! The system tray icon.
//!
//! This speaks the StatusNotifierItem protocol over D-Bus directly (KDE, COSMIC and others
//! implement it) instead of going through Qt: on a KDE session Qt's tray support builds a
//! `QMenu`, which needs `QApplication` from Qt Widgets, and aborts under the `QGuiApplication`
//! this app uses. The icon only reports state and asks the window to show, hide or quit; QML
//! decides what those mean.

use crate::backend::{qobject, runtime};
use cxx_qt::{CxxQtThread, CxxQtType, Threading};
use cxx_qt_lib::QString;
use ksni::menu::{MenuItem, StandardItem};
use ksni::{Icon, Status, ToolTip, TrayMethods};
use std::pin::Pin;
use std::sync::Arc;

/// Side length of the icons, which are raw RGBA files rendered from the SVGs beside them.
const SIZE: i32 = 64;
const IDLE: &[u8] = include_bytes!("../qml/icons/tray.rgba");
const ATTENTION: &[u8] = include_bytes!("../qml/icons/tray-attention.rgba");

/// Converts straight RGBA bytes (what the icon files hold) to the ARGB in network byte order
/// that StatusNotifierItem expects.
fn argb(rgba: &[u8]) -> Vec<u8> {
    let (pixels, _) = rgba.as_chunks::<4>();
    pixels
        .iter()
        .flat_map(|&[r, g, b, a]| [a, r, g, b])
        .collect()
}

pub(crate) struct DeckTray {
    attention: bool,
    tooltip: String,
    backend: CxxQtThread<qobject::PrBackend>,
}

impl ksni::Tray for DeckTray {
    fn id(&self) -> String {
        "pulldeck".into()
    }

    fn title(&self) -> String {
        "pulldeck".into()
    }

    fn status(&self) -> Status {
        if self.attention {
            Status::NeedsAttention
        } else {
            Status::Active
        }
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        vec![Icon {
            width: SIZE,
            height: SIZE,
            data: argb(if self.attention { ATTENTION } else { IDLE }),
        }]
    }

    fn tool_tip(&self) -> ToolTip {
        ToolTip {
            title: self.tooltip.clone(),
            ..Default::default()
        }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        self.backend.queue(|b| b.tray_toggle_requested()).ok();
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        vec![
            StandardItem {
                label: "Show or hide".into(),
                activate: Box::new(|t: &mut Self| {
                    t.backend.queue(|b| b.tray_toggle_requested()).ok();
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Quit".into(),
                activate: Box::new(|t: &mut Self| {
                    t.backend.queue(|b| b.tray_quit_requested()).ok();
                }),
                ..Default::default()
            }
            .into(),
        ]
    }
}

pub(crate) type TrayHandle = Arc<ksni::Handle<DeckTray>>;

impl qobject::PrBackend {
    /// Puts the icon in the tray if the desktop has one. Without a StatusNotifier host (a bare
    /// window manager) nothing happens and closing the window still quits.
    pub fn start_tray(self: Pin<&mut Self>) {
        if self.tray.is_some() {
            return;
        }
        let tray = DeckTray {
            attention: self.tray_attention,
            tooltip: self.tray_tooltip.clone(),
            backend: self.qt_thread(),
        };
        let qt_thread = self.qt_thread();
        runtime().spawn(async move {
            match tray.spawn().await {
                Ok(handle) => {
                    qt_thread
                        .queue(move |mut backend| {
                            backend.as_mut().rust_mut().tray = Some(Arc::new(handle));
                            // The icon can bring the window back, so closing it may hide it.
                            qobject::set_quit_on_last_window_closed(false);
                            backend.sync_tray();
                        })
                        .ok();
                }
                Err(error) => tracing::info!(%error, "no system tray; running without one"),
            }
        });
    }

    /// Updates what the tray shows. Safe to call before the tray exists.
    pub fn set_tray_state(mut self: Pin<&mut Self>, attention: bool, tooltip: &QString) {
        let tooltip = tooltip.to_string();
        if self.tray_attention == attention && self.tray_tooltip == tooltip {
            return;
        }
        self.as_mut().rust_mut().tray_attention = attention;
        self.as_mut().rust_mut().tray_tooltip = tooltip;
        self.sync_tray();
    }

    fn sync_tray(&self) {
        let Some(handle) = self.tray.clone() else {
            return;
        };
        let (attention, tooltip) = (self.tray_attention, self.tray_tooltip.clone());
        runtime().spawn(async move {
            handle
                .update(|t| {
                    t.attention = attention;
                    t.tooltip = tooltip;
                })
                .await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icons_are_square_rgba_of_the_declared_size() {
        for icon in [IDLE, ATTENTION] {
            assert_eq!(icon.len(), (SIZE * SIZE * 4) as usize);
        }
    }

    #[test]
    fn rgba_becomes_argb_in_network_order() {
        assert_eq!(
            argb(&[1, 2, 3, 4, 5, 6, 7, 8]),
            vec![4, 1, 2, 3, 8, 5, 6, 7]
        );
    }

    #[test]
    fn the_attention_icon_differs_from_the_idle_one() {
        assert_ne!(IDLE, ATTENTION);
    }
}
