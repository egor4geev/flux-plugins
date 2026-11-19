//! FluxGram: Telegram chats in Flux — an unofficial client through the Telegram API.
//!
//! The service (`service = true`) starts the plugin's program (`fluxgram-helper`: TDLib behind
//! JSON lines, `helpers/fluxgram-helper/README.md`), keeps a TDLib client per account and the
//! accounts' list, and tells the windows' instances what changed; a window's instance draws the
//! FluxGram window on the left, the accounts in Settings and the Add Account dialog, and passes
//! what the user does to the service.
//!
//! - [`model`] — what needs no host: the messages between the service and the windows, TDLib's
//!   states and errors, the masked numbers;
//! - [`store`] — what the service knows of an account's Telegram: users, chats, groups, files;
//! - [`service`] — the helper, the accounts, the sign-in; the chats' lines go to the store, the
//!   list and the feed;
//! - [`chats`] — the service's list: the chats the user added, the Add Chat popup, notifications;
//! - [`feed`] — the service's open chats: their messages, what the user sends, the media's
//!   downloads and the stickers' animations;
//! - [`panel`] — the field's panel: the account's stickers and GIFs, their search, sending them;
//! - [`pins`] — a chat's pinned messages and the bar over its feed (no host: tested natively);
//! - [`ads`] — Telegram's advertisements in channels: asked for, seen, clicked, reported (no host:
//!   tested natively);
//! - [`bots`] — the bots' buttons under their messages and keyboards over the field (no host:
//!   tested natively);
//! - [`suggest`] — the field's suggestions: the members for "@", the bots' commands for "/",
//!   the mentions of people without a username (no host: tested natively);
//! - [`links`] — every link's way: Telegram's open in FluxGram, the text's mentions, hashtags
//!   and commands do what they say, the web's are asked first as the settings say, a spoofed
//!   address always (no host: tested natively);
//! - [`hidden`] — the hidden chats' list, sealed with a key derived from the user's phrase;
//! - [`convert`] — TDLib's messages as Flux shows them (no host: tested natively);
//! - [`summary`] — a message in one line: the list's last line, a notification, a quote;
//! - [`profile`] / [`profile_view`] — the profile dialog: a person, a group and its members, the
//!   chat's media;
//! - [`window`] — a window's instance: the accounts in Settings, the Add Account dialog, and the
//!   routing to [`list`] (the window on the left) and [`chat`] (a chat's tab);
//! - [`config`] — FluxGram's app key with Telegram.

mod ads;
mod bots;
mod chat;
mod chats;
mod config;
mod convert;
mod feed;
mod hidden;
mod links;
mod list;
mod model;
mod panel;
mod pins;
mod profile;
mod profile_view;
mod service;
mod store;
mod suggest;
mod summary;
mod window;

use flux_plugin_api::{CommandContext, Event, Plugin, register_plugin};

/// The plugin's instance: the service, or a window's.
enum FluxGram {
    // Both are large (the service's stores, a window's lists and tabs) and made once.
    Service(Box<service::Service>),
    Window(Box<window::Window>),
}

impl Plugin for FluxGram {
    fn new() -> Self {
        if flux_plugin_api::service::is_service() {
            FluxGram::Service(Box::new(service::Service::new()))
        } else {
            FluxGram::Window(Box::new(window::Window::new()))
        }
    }

    fn activate(&mut self) {
        match self {
            FluxGram::Service(service) => {
                store::write_saved_icon(&flux_plugin_api::storage::dir());
                service.activate()
            }
            FluxGram::Window(window) => window.activate(),
        }
    }

    fn deactivate(&mut self) {
        if let FluxGram::Service(service) = self {
            service.deactivate();
        }
    }

    fn run_command(&mut self, command: &str, context: &CommandContext) {
        if let FluxGram::Window(window) = self {
            window.run_command(command, context.argument.as_deref());
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
