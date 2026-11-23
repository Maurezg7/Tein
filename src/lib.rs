slint::include_modules!();

mod app;
mod download;
mod library;
mod net;
mod online;
mod paths;
mod platform;
mod player;
mod store;

pub fn run() {
    if let Err(e) = app::run() {
        log::error!("UI error: {e}");
    }
}

#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
fn android_main(app: slint::android::AndroidApp) {
    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(log::LevelFilter::Info)
            .with_tag("ritmo"),
    );
    paths::init_android(&app);
    slint::android::init(app).expect("Slint Android init");
    run();
}
