//! A window's instance: the «Binding» block of the settings page (the manifest's `binding` tool
//! window). It shows what the service says of the connection: while the bot waits for its owner —
//! the link with the one-time code, its QR code for the phone's camera, «Copy» and «New Link»;
//! once bound — who it is bound to and «Unbind» (one calm question first); with the topics off —
//! how to turn them on in @BotFather and «Check Again»; a bad token, another program polling the
//! bot, no connection — what it is and what to do. The buttons go to the service ([`ToService`]).

use flux_plugin_api::dialog::Ask;
use flux_plugin_api::view::*;
use flux_plugin_api::{Event, UiEvent, service, system, tr};

use crate::model::{Connection, ToService, ToWindow};

/// The manifest's settings block.
pub const BLOCK: &str = "binding";

#[derive(Default)]
pub struct Window {
    connection: Connection,
    /// The link copied last: «Copied» on the button while it is the one shown.
    copied: Option<String>,
    /// «Unbind the bot?» waits for its answer.
    asked: Option<u64>,
}

impl Window {
    pub fn start(&mut self) {
        let _ = service::send_json(&ToService::Hello);
    }

    pub fn on_event(&mut self, event: Event) {
        match event {
            Event::ServiceStarted => self.start(),
            Event::ServiceMessage((None, text)) => {
                if let Some(ToWindow::Connection(connection)) = service::parse(&text) {
                    self.connection = connection;
                    self.render();
                }
            }
            Event::ToolWindowShown(window) if window == BLOCK => self.render(),
            Event::Ui(input) if input.window == BLOCK => {
                if let UiEvent::Clicked = input.event {
                    self.clicked(&input.element);
                }
            }
            Event::DialogAnswered((id, answer)) if Some(id) == self.asked => {
                self.asked = None;
                if answer == Some(0) {
                    let _ = service::send_json(&ToService::Unbind);
                }
            }
            _ => {}
        }
    }

    fn clicked(&mut self, element: &str) {
        let message = match element {
            "copy" => {
                if let Connection::Unbound { link, .. } = &self.connection {
                    system::copy_text(link);
                    self.copied = Some(link.clone());
                    self.render();
                }
                return;
            }
            "unbind" => {
                // One calm question: a short title and one sentence.
                self.asked = Some(
                    Ask::warning(&tr("Unbind the bot?"))
                        .message(&tr(
                            "The bot will stop answering in that chat; you can bind it again with a new link.",
                        ))
                        .danger(&tr("Unbind"))
                        .cancel(&tr("Cancel"))
                        .send(),
                );
                return;
            }
            "new-link" => ToService::NewCode,
            "check-again" => ToService::CheckAgain,
            _ => return,
        };
        let _ = service::send_json(&message);
    }

    fn render(&self) {
        let dim = |id: &str, words: &str| text(id, [span(words).tone(Tone::Dim)]);
        let line = |id: &str, words: &str, tone: Tone| text(id, [span(words).tone(tone)]);
        let check_again = || button("check-again", &tr("Check Again")).into();
        let children: Vec<Element> = match &self.connection {
            Connection::NoToken => Vec::new(),
            Connection::Checking => vec![dim("checking", &tr("Checking the bot…"))],
            Connection::Unbound { link, .. } if link.is_empty() => {
                vec![dim("checking", &tr("Checking the bot…"))]
            }
            Connection::Unbound { link, .. } => {
                let copied = self.copied.as_ref() == Some(link);
                vec![
                    label(
                        "hint",
                        &tr("Open the link on your phone or point its camera at the code"),
                    ),
                    qr("qr", link),
                    text("link", [span(link).code()]),
                    row(
                        "link-buttons",
                        [
                            button("copy", &tr(if copied { "Copied" } else { "Copy" })).into(),
                            button("new-link", &tr("New Link")).into(),
                        ],
                    ),
                ]
            }
            Connection::Ready { bot, owner } => vec![
                line(
                    "owner",
                    &tr("Bound to {0}").replace("{0}", owner),
                    Tone::Success,
                ),
                dim(
                    "about",
                    &tr("{0} answers only you; the sessions are topics of its chat.")
                        .replace("{0}", bot),
                ),
                row("owner-buttons", [button("unbind", &tr("Unbind")).into()]),
            ],
            Connection::NoTopics { .. } => vec![
                line(
                    "no-topics",
                    &tr(
                        "The bot's topics are off. Turn them on in @BotFather: the bot's settings → topics.",
                    ),
                    Tone::Warning,
                ),
                row("topics-buttons", [check_again()]),
            ],
            Connection::BadToken => vec![line(
                "bad-token",
                &tr("The token doesn't fit — check it in @BotFather"),
                Tone::Error,
            )],
            Connection::Conflict { .. } => vec![
                line(
                    "conflict",
                    &tr("Another program or another Flux polls this bot"),
                    Tone::Error,
                ),
                row("conflict-buttons", [check_again()]),
            ],
            Connection::Offline { error, .. } => vec![
                line(
                    "offline",
                    &tr("No connection to Telegram: {0}").replace("{0}", error),
                    Tone::Warning,
                ),
                dim("retrying", &tr("Flux tries again by itself.")),
            ],
        };
        set_view(BLOCK, column("root", children));
    }
}
