// A real Windows GUI program in release builds (no console window); debug builds keep the console for panics and logs.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

fn main() -> std::process::ExitCode {
    shaiya_ui::run::<shaiya_studio::StudioApp>()
}
