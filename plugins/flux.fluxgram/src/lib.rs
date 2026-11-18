//! FluxGram: Telegram chats in Flux — an unofficial client through the Telegram API.
//!
//! The service (`service = true`) starts the plugin's program (`fluxgram-helper`: TDLib behind
//! JSON lines, `helpers/fluxgram-helper/README.md`), keeps a TDLib client per account and the
//! accounts' list, and tells the windows' instances what changed; a window's instance draws the
//! FluxGram window on the left, the accounts in Settings and the Add Account dialog, and passes
//! what the user does to the service.
//!
//! - [`model`] — what needs no host: the messages, TDLib's states and errors, the masked numbers;
//! - [`service`] — the helper, the accounts, the sign-in;
//! - [`window`] — the three places of a window;
//! - [`config`] — FluxGram's app key with Telegram.

mod config;
mod model;
mod service;
mod window;

use flux_plugin_api::{CommandContext, Event, Plugin, register_plugin};

/// The plugin's instance: the service, or a window's.
enum FluxGram {
    Service(service::Service),
    Window(window::Window),
}

impl Plugin for FluxGram {
    fn new() -> Self {
        if flux_plugin_api::service::is_service() {
            FluxGram::Service(service::Service::new())
        } else {
            FluxGram::Window(window::Window::new())
        }
    }

    fn activate(&mut self) {
        match self {
            FluxGram::Service(service) => service.activate(),
            FluxGram::Window(window) => window.activate(),
        }
    }

    fn deactivate(&mut self) {
        if let FluxGram::Service(service) = self {
            service.deactivate();
        }
    }

    fn run_command(&mut self, command: &str, _context: &CommandContext) {
        if let FluxGram::Window(window) = self {
            window.run_command(command);
        }
    }

    fn on_event(&mut self, event: Event) {
        match self {
            FluxGram::Service(service) => service.on_event(event),
            FluxGram::Window(window) => window.on_event(event),
        }
    }
}

register_plugin!(FluxGram);
