//! Terminal UI: app loop, canvas, status bar, CRT pass, widgets, screens.

pub mod app;
pub mod canvas;
pub mod crt;
pub mod keys;
pub mod screens;
#[cfg(test)]
pub(crate) mod shots;
pub mod status;
pub mod widgets;

pub use app::run;
