// SPDX-License-Identifier: MPL-2.0

mod app;
mod backend;
mod config;
mod model;

fn main() -> cosmic::iced::Result {
    cosmic::applet::run::<app::AppModel>(())
}
