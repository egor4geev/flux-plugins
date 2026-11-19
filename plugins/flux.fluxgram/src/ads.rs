//! Telegram's advertisements (part 5; Telegram's terms 3.3,
//! core.telegram.org/api/sponsored-messages): a client that shows channels and bots shows the
//! sponsored messages Telegram gives for them.
//!
//! - A channel's: asked for (`getChatSponsoredMessages`) when its feed reaches the last post, at
//!   most once per [`KEEP_FOR`] (Telegram's apps keep the answer five minutes), and shown after the
//!   last post as one message ([`crate::convert::sponsored`]): the sponsor over the text, "Ad" at
//!   its foot, its button under it.
//! - Seen: once it is on screen (the feed's `visible` reaches it), `viewMessages` with its id
//!   (TDLib 1.8.61 has no call of its own for it), once.
//! - Its button, or a click on its picture: `clickChatSponsoredMessage` (`is_media_click` for the
//!   picture) and the sponsor's link followed as a button's ([`crate::links`]): Telegram's opens
//!   in FluxGram, Telegram's own sites at once, any other page after a question.
//! - Its menu: About the Ad (the sponsor's info and the ad's `additional_info`), Report where it
//!   `can_be_reported` — `reportChatSponsoredMessage`, then the options it offers as a choice,
//!   asked again with the chosen one until it is done ([`ReportResult`]).
//! - A bot's: asked for when its chat opens, and shown as the bar over the chat ([`bar`], the
//!   pinned message's place — Telegram's apps show it "akin to an action bar"): "Ad · <title>" in
//!   the accent, its text; a click on it opens the page, its ⓘ is About the Ad. Seen once the chat
//!   is in sight.
//!
//! The window keeps what its clicks need ([`AdInfo`], `Note::Ad`). Needs no host: tested natively.

use std::time::Duration;

use flux_plugin_api::conversation::{self as conv, PinnedSpec};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::convert::ABOUT_AD;
use crate::model::{ChatKey, Words};

/// An answer of `getChatSponsoredMessages` is kept this long: Telegram's apps ask again only after
/// it.
pub const KEEP_FOR: Duration = Duration::from_secs(300);

/// What a window needs of a chat's advertisement: its message, the sponsor's page, the words of
/// About the Ad.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdInfo {
    /// The message's id in the feed.
    pub message: String,
    pub url: String,
    /// The ad's title: the sponsor's name.
    pub title: String,
    /// The sponsor's info and the ad's own; empty — none.
    pub sponsor_info: String,
    pub additional_info: String,
}

impl AdInfo {
    /// TDLib's `sponsoredMessage`.
    pub fn of(ad: &Value) -> Option<Self> {
        let text = |value: &Value| value.as_str().unwrap_or_default().trim().to_string();
        Some(AdInfo {
            message: ad["message_id"].as_i64()?.to_string(),
            url: text(&ad["sponsor"]["url"]),
            title: text(&ad["title"]),
            sponsor_info: text(&ad["sponsor"]["info"]),
            additional_info: text(&ad["additional_info"]),
        })
    }

    /// About the Ad's sentence: the sponsor's info and the ad's, a line each; `fallback` when
    /// Telegram gave neither.
    pub fn about(&self, fallback: &str) -> String {
        let lines: Vec<&str> = [self.sponsor_info.as_str(), self.additional_info.as_str()]
            .into_iter()
            .filter(|line| !line.is_empty())
            .collect();
        if lines.is_empty() {
            fallback.to_string()
        } else {
            lines.join("\n")
        }
    }
}

/// A bot's advertisement as the bar over its chat: "Ad · <title>" (or "Recommended · …") in the
/// accent, its text in one line, About the Ad as the bar's button; none — no ad.
pub fn bar(ad: &Value, words: &dyn Words) -> Option<PinnedSpec> {
    let id = ad["message_id"].as_i64()?;
    let label = if ad["is_recommended"] == true {
        words.tr("Recommended")
    } else {
        words.tr("Ad")
    };
    let title = ad["title"].as_str().unwrap_or_default();
    let content = &ad["content"];
    let text = content["text"]["text"]
        .as_str()
        .or_else(|| content["caption"]["text"].as_str())
        .unwrap_or_default();
    let line = text.lines().next().unwrap_or_default();
    let spec = conv::pinned_spec(&id.to_string(), &format!("{label} · {title}"), line, 0, 1)
        .action(ABOUT_AD, &words.tr("About the Ad"), "info");
    Some(spec)
}

/// What a window asks of the service about an advertisement: `Request::Ad`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "ad", rename_all = "kebab-case")]
pub enum AdAsk {
    /// Its button or its picture (`media`) was clicked: Telegram is told (the window opens the
    /// page).
    Clicked {
        key: ChatKey,
        message: String,
        media: bool,
    },
    /// Report it: `option` — the id of the option chosen (base64, as TDLib's JSON has bytes);
    /// empty — the first ask. The options come back as `Note::AdReport` to this window.
    Report {
        key: ChatKey,
        message: String,
        option: String,
    },
}

/// What `reportChatSponsoredMessage` answered.
#[derive(Debug, Clone, PartialEq)]
pub enum ReportResult {
    /// Reported.
    Done,
    /// The user picks one of these (id, text) under this title, and it is asked again with it.
    Choose {
        title: String,
        options: Vec<(String, String)>,
    },
    /// The ads are hidden for the user in every chat.
    Hidden,
    /// Hiding them needs Telegram Premium.
    Premium,
    /// Too old, or gone; an error.
    Failed,
}

impl ReportResult {
    pub fn of(answer: &Value) -> Self {
        match answer["@type"].as_str().unwrap_or_default() {
            "reportSponsoredResultOk" => ReportResult::Done,
            "reportSponsoredResultOptionRequired" => ReportResult::Choose {
                title: answer["title"].as_str().unwrap_or_default().to_string(),
                options: answer["options"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|option| {
                        Some((
                            option["id"].as_str()?.to_string(),
                            option["text"].as_str()?.to_string(),
                        ))
                    })
                    .collect(),
            },
            "reportSponsoredResultAdsHidden" => ReportResult::Hidden,
            "reportSponsoredResultPremiumRequired" => ReportResult::Premium,
            _ => ReportResult::Failed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn about_says_the_sponsors_and_the_ads_info() {
        let ad = json!({"@type": "sponsoredMessage", "message_id": 42, "title": "Rocket VPN",
            "sponsor": {"@type": "advertisementSponsor", "url": "https://rocket.example",
                "info": "Rocket LLC, INN 7700000000"},
            "additional_info": "erid: 2VtzqwX"});
        let info = AdInfo::of(&ad).unwrap();
        assert_eq!(info.message, "42");
        assert_eq!(info.url, "https://rocket.example");
        assert_eq!(info.about("—"), "Rocket LLC, INN 7700000000\nerid: 2VtzqwX");
        let bare = AdInfo::of(&json!({"message_id": 1})).unwrap();
        assert_eq!(bare.about("Telegram shows it."), "Telegram shows it.");
    }

    #[test]
    fn a_bots_ad_is_a_bar_over_the_chat() {
        let ad = json!({"@type": "sponsoredMessage", "message_id": 7, "title": "Rocket VPN",
            "content": {"@type": "messageText", "text": {"text": "Fast.\nPrivate."}}});
        let spec = bar(&ad, &crate::model::English).unwrap();
        assert_eq!(spec.message, "7");
        assert_eq!(spec.title, "Ad · Rocket VPN");
        assert_eq!(spec.text, "Fast.");
        assert_eq!(spec.actions[0].id, ABOUT_AD);
        assert_eq!(bar(&json!({}), &crate::model::English), None);
    }

    #[test]
    fn reports_until_telegram_says_done() {
        let choose = ReportResult::of(&json!({"@type": "reportSponsoredResultOptionRequired",
            "title": "Why?", "options": [{"@type": "reportOption", "id": "AQ==", "text": "Spam"},
            {"@type": "reportOption", "id": "Ag==", "text": "Scam"}]}));
        assert_eq!(
            choose,
            ReportResult::Choose {
                title: "Why?".into(),
                options: vec![
                    ("AQ==".into(), "Spam".into()),
                    ("Ag==".into(), "Scam".into())
                ],
            }
        );
        assert_eq!(
            ReportResult::of(&json!({"@type": "reportSponsoredResultOk"})),
            ReportResult::Done
        );
        assert_eq!(
            ReportResult::of(&json!({"@type": "reportSponsoredResultAdsHidden"})),
            ReportResult::Hidden
        );
        assert_eq!(
            ReportResult::of(&json!({"@type": "error", "code": 400})),
            ReportResult::Failed
        );
    }
}
