//! Flux Remote: Flux's Claude sessions from the phone, through the user's own Telegram bot.
//!
//! The service (`service = true`, one instance for the whole app) is the bot ([`bot::Bot`]): it
//! polls Telegram (`getUpdates` — outgoing HTTPS only, no server, no ports), watches every Claude
//! session of Flux (`claude-remote`) and keeps a topic of the bot's private chat per session — the
//! user's messages go to the session, Claude's answer comes as it is written, the permissions,
//! questions and the plan come as buttons; «Общее» is the console. A window's instance draws the
//! «Binding» block of the settings page ([`window`]).
//!
//! - [`telegram`] — the Bot API's values and calls (no host: tested natively);
//! - [`transport`] — the calls' order and pace, the polling;
//! - [`host`] — what the bot needs from Flux, as a trait (the SDK's, or [`testing`]'s fake);
//! - [`store`] — what is kept between runs: the owner, «Общее», the sessions' topics;
//! - [`keys`] — the buttons' data;
//! - [`bot`] — the state and the routing; the modules: [`binding`], [`general`], [`handoff`],
//!   [`awake`] (agent A), [`newchat`], [`conversation`], [`requests`], [`render`] (agent B);
//! - [`model`] — the messages between the service and the windows.

mod awake;
mod binding;
mod bot;
mod conversation;
mod general;
mod handoff;
mod host;
mod keys;
mod model;
mod newchat;
mod render;
mod requests;
mod store;
mod telegram;
#[cfg(test)]
mod testing;
mod transport;
mod window;

use flux_plugin_api::{CommandContext, Event, Plugin, register_plugin, service};

use crate::bot::{Bot, ClaudeEvent};
use crate::host::SdkHost;
use crate::window::Window;

enum Instance {
    Service(Box<Bot>),
    Window(Window),
}

struct FluxRemote {
    instance: Instance,
}

impl Plugin for FluxRemote {
    fn new() -> Self {
        let instance = if service::is_service() {
            Instance::Service(Box::new(Bot::new(&SdkHost)))
        } else {
            Instance::Window(Window::default())
        };
        FluxRemote { instance }
    }

    fn activate(&mut self) {
        match &mut self.instance {
            Instance::Service(bot) => bot.start(&mut SdkHost),
            Instance::Window(window) => window.start(),
        }
    }

    fn run_command(&mut self, _command: &str, _context: &CommandContext) {}

    fn on_event(&mut self, event: Event) {
        match &mut self.instance {
            Instance::Service(bot) => service_event(bot, &mut SdkHost, event),
            Instance::Window(window) => window.on_event(event),
        }
    }
}

fn service_event(bot: &mut Bot, host: &mut SdkHost, event: Event) {
    let claude = match event {
        Event::HttpResponse(head) => return bot.on_http_head(head.request, head.status),
        Event::HttpBody((request, chunk)) => return bot.on_http_body(request, &chunk),
        Event::HttpDone((request, result)) => {
            return bot.on_http_done(host, request, result.err().map(|err| err.message()));
        }
        Event::Timer(timer) => return bot.on_timer(host, timer),
        Event::SettingsChanged => return bot.settings_changed(host),
        Event::ServiceMessage((window, text)) => {
            if let Some(message) = service::parse(&text) {
                bot.on_window(host, window, message);
            }
            return;
        }
        Event::ClaudeSession(session) => ClaudeEvent::Session(session),
        Event::ClaudeSessionGone(session) => ClaudeEvent::Gone(session),
        Event::ClaudeText(update) => ClaudeEvent::Text(update),
        Event::ClaudeSteps((session, steps)) => ClaudeEvent::Steps(session, steps),
        Event::ClaudeRequest((session, request)) => ClaudeEvent::Request(session, request),
        Event::ClaudeResolved((session, request)) => ClaudeEvent::Resolved(session, request),
        Event::ClaudeTurnEnded((session, summary)) => ClaudeEvent::TurnEnded(session, summary),
        Event::ClaudeFailed((session, error)) => ClaudeEvent::Failed(session, error),
        Event::ClaudeLimits(limits) => ClaudeEvent::Limits(limits),
        Event::ClaudeRegion(region) => ClaudeEvent::Region(region),
        Event::SystemChanged(change) => ClaudeEvent::System(change),
        // (the session, the action's id), as the WIT has it.
        Event::ClaudeAction((session, action)) => ClaudeEvent::Action(action, session),
        _ => return,
    };
    bot.on_claude(host, claude);
}

register_plugin!(FluxRemote);
