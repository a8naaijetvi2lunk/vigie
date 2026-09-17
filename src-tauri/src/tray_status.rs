//! Icône du tray : frames préparées une fois, aucun I/O dans la boucle d'animation.
use crate::{config::ConfigState, sessions::SessionsState, AppState};
use std::time::Duration;
use tauri::Manager;

fn system_animations_enabled() -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SystemParametersInfoW, SPI_GETCLIENTAREAANIMATION,
    };
    let mut enabled = 0i32;
    unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            (&mut enabled as *mut i32).cast(),
            0,
        ) != 0
            && enabled != 0
    }
}

pub fn spawn(app: tauri::AppHandle) {
    std::thread::spawn(move || {
        let base = tauri::include_image!("icons/vigie/32x32.png");
        let frames: Vec<_> = (0..8)
            .map(|phase| {
                let mut rgba = base.rgba().to_vec();
                let glow =
                    0.65 + 0.35 * ((phase as f64 / 8.0) * std::f64::consts::TAU).sin().powi(2);
                for pixel in rgba.chunks_exact_mut(4) {
                    if pixel[0] > 120 && pixel[0] > pixel[1].saturating_add(40) && pixel[1] < 150 {
                        for (channel, bg) in pixel[..3].iter_mut().zip([245., 241., 234.]) {
                            *channel = (*channel as f64 * glow + bg * (1.0 - glow)).round() as u8;
                        }
                    }
                }
                tauri::image::Image::new_owned(rgba, base.width(), base.height())
            })
            .collect();
        let mut warning = base.rgba().to_vec();
        let mut stale = warning.clone();
        for pixel in warning.chunks_exact_mut(4) {
            if pixel[0] < 100 && pixel[1] < 120 {
                pixel[..3].copy_from_slice(&[169, 59, 38]);
            }
        }
        for pixel in stale.chunks_exact_mut(4) {
            if pixel[0] < 220 {
                pixel[..3].copy_from_slice(&[107, 99, 89]);
            }
        }
        let warning = tauri::image::Image::new_owned(warning, base.width(), base.height());
        let stale = tauri::image::Image::new_owned(stale, base.width(), base.height());
        let mut last = String::new();
        let mut last_tooltip = String::new();
        let mut phase = 0;
        loop {
            let config = app.state::<ConfigState>().0.lock().unwrap().clone();
            let snapshot = app.state::<AppState>().0.lock().unwrap().clone();
            let sessions = app.state::<SessionsState>().0.lock().unwrap().clone();
            let active = sessions.iter().filter(|s| s.active).count();
            let now = crate::util::now_epoch();
            let valid: Vec<_> = snapshot
                .providers
                .iter()
                .filter(|p| {
                    (if p.id == "claude" {
                        config.providers.claude
                    } else {
                        config.providers.codex
                    }) && p.note.is_none()
                        && !p.is_stale(now)
                })
                .collect();
            let critical = valid
                .iter()
                .any(|p| p.windows.iter().any(|w| w.used_percent >= 90.0));
            let unavailable = valid.is_empty();
            let animate = active > 0
                && config.animations
                && system_animations_enabled()
                && !critical
                && !unavailable;
            let key = if critical {
                "warning".into()
            } else if unavailable {
                "stale".into()
            } else if animate {
                format!("active-{phase}")
            } else {
                "idle".into()
            };
            if let Some(tray) = app.tray_by_id("main") {
                if key != last {
                    let icon = if critical {
                        warning.clone()
                    } else if unavailable {
                        stale.clone()
                    } else if animate {
                        frames[phase].clone()
                    } else {
                        base.clone()
                    };
                    let _ = tray.set_icon(Some(icon));
                    last = key;
                }
                let summary = snapshot
                    .providers
                    .iter()
                    .map(|p| {
                        format!(
                            "{} {}",
                            if p.id == "claude" { "Claude" } else { "Codex" },
                            if p.is_stale(now) || p.note.is_some() {
                                "à actualiser".into()
                            } else {
                                p.windows
                                    .first()
                                    .map(|w| format!("{:.0}%", w.used_percent))
                                    .unwrap_or("en attente".into())
                            }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(" · ");
                let tooltip = format!("Vigie · {active} session(s) active(s)\n{summary}");
                if tooltip != last_tooltip {
                    let _ = tray.set_tooltip(Some(&tooltip));
                    last_tooltip = tooltip;
                }
            }
            phase = (phase + 1) % frames.len();
            std::thread::sleep(Duration::from_millis(if animate { 300 } else { 1000 }));
        }
    });
}
