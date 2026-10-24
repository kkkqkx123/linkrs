mod app;
mod components;
mod connection;
mod runtime;
mod services;
mod settings;
mod state;
mod views;

fn main() {
    env_logger::builder()
        .filter_level(log::LevelFilter::Info)
        .parse_default_env()
        .init();

    let settings = settings::Settings::load();
    app::run(settings);
}
