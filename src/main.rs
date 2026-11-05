//! dbml-ui — desktop DBML diagram editor built on iced.

#![forbid(unsafe_code)] // the whole editor is written in safe Rust
mod app;
#[cfg(all(unix, not(target_os = "macos")))]
mod platform;
mod canvas;
mod clipboard;
mod config;
mod editor;
mod session;
mod theme;
mod view;

/// The window/taskbar icon: icons/app-icon-128.png (the favicon mark)
/// decoded at boot.
fn window_icon() -> Option<iced::window::Icon> {
    let png = include_bytes!("../icons/app-icon-128.png");
    let img = image::load_from_memory(png).ok()?.into_rgba8();
    let (width, height) = (img.width(), img.height());
    iced::window::icon::from_rgba(img.into_raw(), width, height).ok()
}

fn main() -> Result<(), iced::Error> {
    let run = || -> Result<(), iced::Error> {
        // dbml-ui [FILE]
        let restarted = std::env::var_os("DBML_UI_RESTARTED").is_some();
        let arg_file = std::env::args().nth(1);
        // an explicit file argument opens as-is; everything else starts
        // from the splash screen
        let mut app = match &arg_file {
            Some(path) => {
                let mut app = app::App::default();
                if let Err(e) = app.open_file(path) {
                    eprintln!("cannot open {path}: {e}");
                    app::App::default()
                } else {
                    app
                }
            }
            None => app::App::blank_start(),
        };
        // persisted preferences (vim/Regular) + recent files
        app.config = config::Config::load();
        // Restore rules (no file argument):
        // - a clean-exit record reopens its files from disk on ANY start
        //   (a purposeful close remembered them); paths that fail —
        //   moved, renamed, deleted — skip silently, and nothing
        //   restorable boots to the splash screen.
        // - a crash snapshot restores WITH its unsaved edits only after a
        //   GPU-loss restart; on a plain launch it is ignored (splash).
        if arg_file.is_none() {
            if let Some(s) = session::load() {
                if s.clean {
                    app.restore_open_files(s);
                } else if restarted {
                    app.restore_session(s);
                }
            }
        }
        let _ = &mut app;
        // the window title follows the active tab's file
        iced::application(|app: &app::App| app.window_title(), app::update, view::root)
            .theme(|_| theme::app())
            // the embedded faces: the mono family for the editor, and the
            // bold/semibold weights of both families so weight selects
            // render even where system fonts lack those weights
            .font(dbml_ui::MONO_TTF)
            .font(dbml_ui::MONO_TTF_BOLD)
            .font(dbml_ui::FIRA_SANS_BOLD)
            .font(dbml_ui::FIRA_SANS_SEMIBOLD)
            .subscription(app::subscription)
            // canvas strokes (dash flow, markers, glow) are triangle meshes;
            // without MSAA every moving stroke rasterizes hard-edged
            .antialiasing(true)
            .window(iced::window::Settings {
                size: iced::Size::new(1400.0, 900.0),
                // a floor under the layout: overlays, the diagnostics panel
                // and the diff cells are styled for sane dimensions — the
                // window cannot shrink past this
                min_size: Some(iced::Size::new(940.0, 600.0)),
                // the X button goes through the app's unsaved-changes
                // confirm instead of killing the process (the GPU-loss
                // restart path bypasses all of this and self-heals).
                // NOTE: this lives IN the settings — a later .window()
                // call replaces the whole struct and silently re-enables
                // the auto-close (the dialog vanished once that way)
                exit_on_close_request: false,
                // the favicon mark as the window/taskbar icon: the 128px
                // PNG decoded at boot
                icon: window_icon(),
                ..Default::default()
            })
            .run_with(|| {
                // iced 0.13 never focuses the main window itself, so keyboard
                // input was ignored until the first click focused it. The main
                // window is the only one: focus the oldest.
                let focus = iced::window::get_oldest().then(|id| {
                    id.map(iced::window::gain_focus)
                        .unwrap_or_else(iced::Task::none)
                });
                (app, focus)
            })
    };
    // wgpu 0.19 PANICS (no error path) when the GPU device dies under a
    // live surface — which happens when the screen blanks / the machine
    // suspends and the driver drops the device ("Parent device is lost" at
    // the next frame). The next process initializes wgpu cleanly, so this
    // specific panic restarts the app once instead of dying.
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(run)) {
        Ok(result) => result,
        Err(panic) => {
            let message = panic
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| panic.downcast_ref::<&'static str>().map(|s| s.to_string()))
                .unwrap_or_default();
            if is_gpu_device_loss(&message) {
                if std::env::var_os("DBML_UI_RESTARTED").is_none() {
                    eprintln!("GPU device lost (screen blank / suspend) — restarting (the session snapshot restores all tabs and edits)");
                    if let Ok(exe) = std::env::current_exe() {
                        let mut cmd = std::process::Command::new(exe);
                        cmd.args(std::env::args_os().skip(1));
                        cmd.env("DBML_UI_RESTARTED", "1");
                        if cmd.spawn().is_ok() {
                            // exit without unwinding through the dead iced
                            // loop; the child owns the session from here
                            return Ok(());
                        }
                    }
                    eprintln!("restart failed — re-raising the original panic");
                } else {
                    eprintln!("GPU device lost again after restart — giving up");
                }
            }
            std::panic::resume_unwind(panic)
        }
    }
}

/// The wgpu panic signatures that mean "the GPU device died under us"
/// (screen blank, suspend, driver reset): nothing else should restart.
fn is_gpu_device_loss(message: &str) -> bool {
    message.contains("Parent device is lost") || message.contains("Surface::get_current_texture")
}

#[cfg(test)]
mod tests {
    use super::is_gpu_device_loss;

    #[test]
    fn gpu_device_loss_matches_the_wgpu_panic_and_nothing_else() {
        let real = "Error in Surface::get_current_texture_view: Validation Error\n\nCaused by:\n  Parent device is lost";
        assert!(is_gpu_device_loss(real));
        assert!(is_gpu_device_loss("Error in Surface::get_current_texture_view: Timeout"));
        // unrelated panics must re-raise, not restart
        assert!(!is_gpu_device_loss("called `Option::unwrap()` on a `None` value"));
        assert!(!is_gpu_device_loss(""));
    }
}
