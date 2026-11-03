mod app;
mod components;
mod connection;
mod i18n;
mod runtime;
mod services;
mod settings;
mod state;
mod syntax;
mod views;

fn main() {
    env_logger::builder()
        .filter_level(log::LevelFilter::Info)
        .parse_default_env()
        .init();

    let settings = settings::Settings::load();
    app::run(settings);
}
