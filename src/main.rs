#[cfg(not(target_os = "windows"))]
mod app;
mod drm;
mod extensions;
#[cfg(not(target_os = "windows"))]
mod passwords;
mod memory_policy;
#[cfg(target_os = "windows")]
mod process_metrics;
mod state;
mod store_identity;
mod zoom;
#[cfg(target_os = "windows")]
mod windows_app;

fn main() {
    if let Err(error) = extensions::ensure_builtins() {
        eprintln!("ubar: could not provision built-in blocker: {error}");
    }
    eprintln!("ubar: DRM: {}", drm::detect().label());
    #[cfg(not(target_os = "windows"))]
    app::run();
    #[cfg(target_os = "windows")]
    windows_app::run();
}
