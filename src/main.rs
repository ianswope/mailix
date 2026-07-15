use adw::prelude::*;
use gtk::glib;

mod composer;
mod config;
mod google;
mod mime;
mod omarchy;
mod render;
mod secrets;
mod store;
mod style;
mod util;
mod window;

const APP_ID: &str = "com.ianswope.Mailix";

fn main() -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_startup(|_| style::load());
    app.connect_activate(window::build);
    app.run()
}
