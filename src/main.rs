#[cfg(not(target_os = "windows"))]
mod app;
#[cfg(not(target_os = "windows"))]
mod extensions;
#[cfg(not(target_os = "windows"))]
mod passwords;
mod state;
#[cfg(target_os = "windows")]
mod windows_app;

fn main() {
    #[cfg(not(target_os = "windows"))]
    app::run();
    #[cfg(target_os = "windows")]
    windows_app::run();
}
