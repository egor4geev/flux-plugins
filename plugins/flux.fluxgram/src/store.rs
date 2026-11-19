//! What the service knows of an account's Telegram (part 3): its users, chats, groups and files,
//! kept up to date by TDLib's updates and the objects of its answers ([`Store::apply`] takes every
//! line of the account). The list (`chats.rs`) and the feed (`feed.rs`) read it; neither keeps a
//! copy. Messages aren't here: the feed keeps those of the open chats.
//!
//! Part 5: channels and bots are listed like people and groups; a forum's topics (names and
//! colors) are kept for the labels of its messages; the reactions each chat allows, and
//! Telegram's active ones for a chat that allows all ([`Store::reactions`]).
//!
//! Needs no host, so it is tested natively. Only the chats in a list or open pass the helper
//! (`fluxgram.chats`), so the store holds those — plus whatever the Add Chat popup's answers
//! brought (`getChat`), and every user TDLib mentions (`updateUser`: names of authors).

use std::collections::HashMap;

use flux_plugin_api::conversation::AvatarSpec;
use serde_json::Value;

use crate::model::{downloaded, initials};

/// The reactions of a chat that allows all of Telegram's, until `updateActiveEmojiReactions`
/// says which they are (part 5): the first of Telegram's own list.
pub const FALLBACK_REACTIONS: &[&str] = &[
    "👍", "❤", "🔥", "🥰", "👏", "😁", "🤔", "🤯", "😱", "😢", "🎉", "🙏",
];

/// Saved Messages' picture: a white bookmark on Telegram's blue, as its apps draw it.
const SAVED_PNG: &[u8] = include_bytes!("../icons/saved.png");
/// Where the service wrote it ([`write_saved_icon`]).
static SAVED_ICON: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// The service starts: Saved Messages' picture goes into the plugin's folder `dir`, for every
/// store made from now on (the list's rows, the header, the profile show it).
pub fn write_saved_icon(dir: &str) {
    let path = format!("{dir}/saved-messages.png");
    if std::fs::write(&path, SAVED_PNG).is_ok() {
        let _ = SAVED_ICON.set(path);
    }
}

/// Saved Messages' picture in a store: the path the service wrote it to; none — the initials.
#[derive(Debug, Clone, PartialEq)]
pub struct SavedIcon(pub Option<String>);

impl Default for SavedIcon {
    fn default() -> Self {
        SavedIcon(SAVED_ICON.get().cloned())
    }
}

/// A user, as messages and headers need them.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct User {
    pub id: i64,
    pub first_name: String,
    pub last_name: String,
    /// The first active username, without "@".
    pub username: Option<String>,
    pub color: u8,
    /// The small profile photo: TDLib's file id.
    pub photo: Option<i64>,
    pub kind: UserKind,
    pub status: Status,
    pub is_contact: bool,
    /// Telegram Premium (part 5): three reactions on a message, not one.
    pub premium: bool,
}

impl User {
    /// "Anna Smirnova"; empty for a deleted account (the caller says "Deleted Account").
    pub fn name(&self) -> String {
        format!("{} {}", self.first_name, self.last_name)
            .trim()
            .to_string()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UserKind {
    #[default]
    Regular,
    Bot,
    Deleted,
    Unknown,
}

/// When a user was online (`userStatus…`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Status {
    #[default]
    Empty,
    /// Online until `expires` (seconds since 1970).
    Online {
        expires: i64,
    },
    /// Last seen at `was` (seconds since 1970).
    Offline {
        was: i64,
    },
    Recently,
    LastWeek,
    LastMonth,
}

/// What kind of chat it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChatKind {
    /// A chat with a user (`chat id == user id`).
    Private {
        user: i64,
    },
    /// A secret chat with a user: FluxGram doesn't show them (they live on one device).
    Secret {
        user: i64,
    },
    BasicGroup {
        group: i64,
    },
    /// A supergroup, or a channel (`channel`).
    Supergroup {
        group: i64,
        channel: bool,
    },
    #[default]
    Unknown,
}

/// A chat, as the list and the feed need it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Chat {
    pub id: i64,
    pub kind: ChatKind,
    pub title: String,
    pub color: u8,
    /// The small chat photo: TDLib's file id.
    pub photo: Option<i64>,
    /// TDLib's last message, as it came: the list's line.
    pub last_message: Option<Value>,
    /// The order in Telegram's main list (0 — not in it), and pinned there.
    pub order: i64,
    pub pinned: bool,
    /// The order in the archive (0 — not archived).
    pub archive_order: i64,
    pub unread_count: u32,
    pub unread_mentions: u32,
    pub marked_unread: bool,
    pub last_read_inbox: i64,
    pub last_read_outbox: i64,
    /// Muted for this many seconds more (`notification_settings.mute_for`), unless the scope's
    /// default applies (`use_default_mute_for`).
    pub mute_for: i64,
    pub default_mute: bool,
    /// The user's unsent text.
    pub draft: Option<String>,
    /// The chat's permissions let members send text, and pin messages (part 5).
    pub can_send_basic: bool,
    pub can_pin_basic: bool,
    /// The message whose keyboard the field shows (part 5, `reply_markup_message_id`: TDLib
    /// applies Telegram's rules); 0 — none.
    pub reply_markup: i64,
    /// The user blocked the chat's person or bot (part 5, `block_list`: a stopped bot).
    pub blocked: bool,
    /// Its messages can't be forwarded or saved (part 5, `has_protected_content`).
    pub protected: bool,
    /// The reactions it allows (part 5, `available_reactions`): none — all of Telegram's active
    /// ones (`chatAvailableReactionsAll`); some — these emoji (empty: no reactions).
    pub reactions: Option<Vec<String>>,
    /// How many different reactions a message may carry (`max_reaction_count`); 0 — unknown.
    pub max_reactions: u32,
}

/// A basic group or a supergroup: the members and the user's place among them.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Group {
    pub id: i64,
    pub member_count: u32,
    /// The user's status: `chatMemberStatusCreator`, `…Administrator`, `…Member`, `…Restricted`,
    /// `…Left`, `…Banned`.
    pub status: String,
    /// A restricted member's own permission to send text (`chatMemberStatusRestricted`).
    pub restricted_can_send: bool,
    /// The user's own right to pin messages (part 5): an administrator's (`can_pin_messages`, a
    /// channel's `can_edit_messages`), a restricted member's permission.
    pub own_can_pin: bool,
    pub is_forum: bool,
    pub username: Option<String>,
    /// A channel's posts carry their authors' signatures (part 5).
    pub sign_messages: bool,
    /// The user may post in the channel (part 5): an administrator's `can_post_messages`.
    pub own_can_post: bool,
}

/// A forum's topic (part 5): its label over the messages of a group whose topics show as one
/// feed.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Topic {
    pub name: String,
    /// The icon's color as one of the avatar colors (0–6).
    pub color: u8,
    pub general: bool,
    /// Closed (or the General topic hidden): only its creator and admins write there.
    pub closed: bool,
}

/// A file TDLib knows: where it is on disk once downloaded, and how far a download or an upload
/// is.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct File {
    pub id: i64,
    /// The size in bytes (the expected one while unknown); 0 — unknown.
    pub size: u64,
    /// Downloaded: the absolute path.
    pub path: Option<String>,
    pub downloading: bool,
    pub downloaded: u64,
    pub uploading: bool,
    pub uploaded: u64,
}

impl File {
    /// A download or an upload in progress: 0.0–1.0.
    pub fn progress(&self) -> Option<f32> {
        let done = if self.downloading {
            self.downloaded
        } else if self.uploading {
            self.uploaded
        } else {
            return None;
        };
        Some(if self.size == 0 {
            0.0
        } else {
            (done as f32 / self.size as f32).clamp(0.0, 1.0)
        })
    }
}

/// What a line changed in the store: the list and the feed look at what they show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Touched {
    Nothing,
    Chat(i64),
    User(i64),
    File(i64),
    /// The default notification settings: every chat's muting may have changed.
    Scope,
}

/// An account's Telegram, as far as FluxGram knows it.
#[derive(Debug, Default)]
pub struct Store {
    /// The signed-in user's id, once known.
    pub me: Option<i64>,
    pub users: HashMap<i64, User>,
    pub chats: HashMap<i64, Chat>,
    pub basic_groups: HashMap<i64, Group>,
    pub supergroups: HashMap<i64, Group>,
    pub files: HashMap<i64, File>,
    /// Private chats, groups and channels are muted by default (Settings → Notifications in
    /// Telegram).
    pub private_muted: bool,
    pub groups_muted: bool,
    pub channels_muted: bool,
    /// The forums' topics (part 5), by the chat and the topic's id.
    pub topics: HashMap<(i64, i64), Topic>,
    /// The favorite stickers' files (part 5): a sticker message offers Add to Favorites or
    /// Remove from Favorites by them.
    pub favorite_stickers: std::collections::HashSet<i64>,
    /// Saved Messages' bookmark, instead of the user's initials.
    pub saved_icon: SavedIcon,
    /// The emoji reactions Telegram has now (part 5, `updateActiveEmojiReactions`): those of a
    /// chat that allows all.
    pub active_reactions: Vec<String>,
}

impl Store {
    /// Takes a line of the account's client: an update, or an answer that is an object FluxGram
    /// keeps (`chat`, `user`, `basicGroup`, `supergroup`, `file`).
    pub fn apply(&mut self, value: &Value) -> Touched {
        let chat_id = value["chat_id"].as_i64().unwrap_or_default();
        match value["@type"].as_str().unwrap_or_default() {
            "user" => self.put_user(value),
            "updateUser" => self.put_user(&value["user"]),
            "updateUserStatus" => {
                let id = value["user_id"].as_i64().unwrap_or_default();
                let status = parse_status(&value["status"]);
                match self.users.get_mut(&id) {
                    Some(user) if user.status != status => {
                        user.status = status;
                        Touched::User(id)
                    }
                    _ => Touched::Nothing,
                }
            }
            "chat" => self.put_chat(value),
            "updateNewChat" => self.put_chat(&value["chat"]),
            "basicGroup" => self.put_group(value, false),
            "updateBasicGroup" => self.put_group(&value["basic_group"], false),
            "supergroup" => self.put_group(value, true),
            "updateSupergroup" => self.put_group(&value["supergroup"], true),
            "file" => self.put_file(value),
            "updateFile" => self.put_file(&value["file"]),
            "forumTopicInfo" => self.put_topic(value),
            "updateForumTopicInfo" => self.put_topic(&value["info"]),
            "forumTopics" => {
                let mut touched = Touched::Nothing;
                for topic in value["topics"].as_array().into_iter().flatten() {
                    if let Touched::Chat(chat) = self.put_topic(&topic["info"]) {
                        touched = Touched::Chat(chat);
                    }
                }
                touched
            }
            // Telegram's reactions (part 5): every chat that allows all of them changes.
            "updateActiveEmojiReactions" => {
                self.active_reactions = value["emojis"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect();
                Touched::Scope
            }
            "updateScopeNotificationSettings" => {
                let muted = value["notification_settings"]["mute_for"]
                    .as_i64()
                    .unwrap_or_default()
                    > 0;
                match value["scope"]["@type"].as_str().unwrap_or_default() {
                    "notificationSettingsScopePrivateChats" => self.private_muted = muted,
                    "notificationSettingsScopeGroupChats" => self.groups_muted = muted,
                    "notificationSettingsScopeChannelChats" => self.channels_muted = muted,
                    _ => return Touched::Nothing,
                }
                Touched::Scope
            }
            kind => {
                let Some(chat) = self.chats.get_mut(&chat_id) else {
                    return Touched::Nothing;
                };
                match kind {
                    "updateChatTitle" => chat.title = text(value, "title"),
                    "updateChatPhoto" => chat.photo = small_photo(&value["photo"]),
                    "updateChatAccentColors" => {
                        chat.color = color(&value["accent_color_id"], chat_id)
                    }
                    "updateChatHasProtectedContent" => {
                        chat.protected = value["has_protected_content"] == true
                    }
                    "updateChatPermissions" => {
                        chat.can_send_basic = can_send(&value["permissions"]);
                        chat.can_pin_basic = value["permissions"]["can_pin_messages"] == true;
                    }
                    "updateChatLastMessage" => {
                        chat.last_message = message_of(&value["last_message"]);
                        apply_positions(chat, &value["positions"]);
                    }
                    "updateChatPosition" => apply_position(chat, &value["position"]),
                    "updateChatReadInbox" => {
                        chat.last_read_inbox = value["last_read_inbox_message_id"]
                            .as_i64()
                            .unwrap_or(chat.last_read_inbox);
                        chat.unread_count = count(&value["unread_count"]);
                    }
                    "updateChatReadOutbox" => {
                        chat.last_read_outbox = value["last_read_outbox_message_id"]
                            .as_i64()
                            .unwrap_or(chat.last_read_outbox)
                    }
                    "updateChatUnreadMentionCount" => {
                        chat.unread_mentions = count(&value["unread_mention_count"])
                    }
                    "updateChatIsMarkedAsUnread" => {
                        chat.marked_unread = value["is_marked_as_unread"] == true
                    }
                    "updateChatNotificationSettings" => {
                        apply_notifications(chat, &value["notification_settings"])
                    }
                    "updateChatDraftMessage" => {
                        chat.draft = draft_of(&value["draft_message"]);
                        apply_positions(chat, &value["positions"]);
                    }
                    // A bot's keyboard over the field; a bot stopped or restarted (part 5).
                    "updateChatReplyMarkup" => {
                        chat.reply_markup = value["reply_markup_message_id"]
                            .as_i64()
                            .unwrap_or_default()
                    }
                    "updateChatBlockList" => {
                        chat.blocked = value["block_list"]["@type"] == "blockListMain"
                    }
                    "updateChatAvailableReactions" => {
                        apply_reactions(chat, &value["available_reactions"])
                    }
                    _ => return Touched::Nothing,
                }
                Touched::Chat(chat_id)
            }
        }
    }

    fn put_user(&mut self, user: &Value) -> Touched {
        let Some(id) = user["id"].as_i64() else {
            return Touched::Nothing;
        };
        let kind = match user["type"]["@type"].as_str().unwrap_or_default() {
            "userTypeBot" => UserKind::Bot,
            "userTypeDeleted" => UserKind::Deleted,
            "userTypeUnknown" => UserKind::Unknown,
            _ => UserKind::Regular,
        };
        let new = User {
            id,
            first_name: text(user, "first_name"),
            last_name: text(user, "last_name"),
            username: user["usernames"]["active_usernames"]
                .as_array()
                .and_then(|names| names.first())
                .and_then(Value::as_str)
                .map(str::to_string),
            color: color(&user["accent_color_id"], id),
            photo: small_photo(&user["profile_photo"]),
            kind,
            status: parse_status(&user["status"]),
            is_contact: user["is_contact"] == true,
            premium: user["is_premium"] == true,
        };
        if self.users.get(&id) == Some(&new) {
            return Touched::Nothing;
        }
        self.users.insert(id, new);
        Touched::User(id)
    }

    fn put_chat(&mut self, chat: &Value) -> Touched {
        let Some(id) = chat["id"].as_i64() else {
            return Touched::Nothing;
        };
        let kind = &chat["type"];
        let kind = match kind["@type"].as_str().unwrap_or_default() {
            "chatTypePrivate" => ChatKind::Private {
                user: kind["user_id"].as_i64().unwrap_or_default(),
            },
            "chatTypeSecret" => ChatKind::Secret {
                user: kind["user_id"].as_i64().unwrap_or_default(),
            },
            "chatTypeBasicGroup" => ChatKind::BasicGroup {
                group: kind["basic_group_id"].as_i64().unwrap_or_default(),
            },
            "chatTypeSupergroup" => ChatKind::Supergroup {
                group: kind["supergroup_id"].as_i64().unwrap_or_default(),
                channel: kind["is_channel"] == true,
            },
            _ => ChatKind::Unknown,
        };
        let mut new = Chat {
            id,
            kind,
            title: text(chat, "title"),
            color: color(&chat["accent_color_id"], id),
            photo: small_photo(&chat["photo"]),
            last_message: message_of(&chat["last_message"]),
            unread_count: count(&chat["unread_count"]),
            unread_mentions: count(&chat["unread_mention_count"]),
            marked_unread: chat["is_marked_as_unread"] == true,
            last_read_inbox: chat["last_read_inbox_message_id"]
                .as_i64()
                .unwrap_or_default(),
            last_read_outbox: chat["last_read_outbox_message_id"]
                .as_i64()
                .unwrap_or_default(),
            draft: draft_of(&chat["draft_message"]),
            can_send_basic: can_send(&chat["permissions"]),
            can_pin_basic: chat["permissions"]["can_pin_messages"] == true,
            reply_markup: chat["reply_markup_message_id"].as_i64().unwrap_or_default(),
            blocked: chat["block_list"]["@type"] == "blockListMain",
            protected: chat["has_protected_content"] == true,
            ..Chat::default()
        };
        apply_notifications(&mut new, &chat["notification_settings"]);
        apply_positions(&mut new, &chat["positions"]);
        apply_reactions(&mut new, &chat["available_reactions"]);
        if self.chats.get(&id) == Some(&new) {
            return Touched::Nothing;
        }
        self.chats.insert(id, new);
        Touched::Chat(id)
    }

    fn put_group(&mut self, group: &Value, supergroup: bool) -> Touched {
        let Some(id) = group["id"].as_i64() else {
            return Touched::Nothing;
        };
        let status = &group["status"];
        let new = Group {
            id,
            member_count: count(&group["member_count"]),
            status: status["@type"].as_str().unwrap_or_default().to_string(),
            restricted_can_send: status["@type"] == "chatMemberStatusRestricted"
                && can_send(&status["permissions"]),
            own_can_pin: status["rights"]["can_pin_messages"] == true
                || status["rights"]["can_edit_messages"] == true
                || status["permissions"]["can_pin_messages"] == true,
            is_forum: group["is_forum"] == true,
            username: group["usernames"]["active_usernames"]
                .as_array()
                .and_then(|names| names.first())
                .and_then(Value::as_str)
                .map(str::to_string),
            sign_messages: group["sign_messages"] == true,
            own_can_post: status["rights"]["can_post_messages"] == true,
        };
        let (groups, chat) = if supergroup {
            (&mut self.supergroups, -1_000_000_000_000 - id)
        } else {
            (&mut self.basic_groups, -id)
        };
        if groups.get(&id) == Some(&new) {
            return Touched::Nothing;
        }
        groups.insert(id, new);
        Touched::Chat(chat)
    }

    /// A forum topic's `forumTopicInfo`: its name and its icon's color.
    fn put_topic(&mut self, info: &Value) -> Touched {
        let (Some(chat), Some(id)) = (info["chat_id"].as_i64(), info["forum_topic_id"].as_i64())
        else {
            return Touched::Nothing;
        };
        let new = Topic {
            name: text(info, "name"),
            color: topic_color(info["icon"]["color"].as_i64().unwrap_or_default()),
            general: info["is_general"] == true,
            closed: info["is_closed"] == true || info["is_hidden"] == true,
        };
        if self.topics.get(&(chat, id)) == Some(&new) {
            return Touched::Nothing;
        }
        self.topics.insert((chat, id), new);
        Touched::Chat(chat)
    }

    fn put_file(&mut self, file: &Value) -> Touched {
        let Some(id) = file["id"].as_i64() else {
            return Touched::Nothing;
        };
        let size = file["size"]
            .as_u64()
            .filter(|size| *size > 0)
            .or_else(|| file["expected_size"].as_u64())
            .unwrap_or_default();
        let local = &file["local"];
        let remote = &file["remote"];
        let new = File {
            id,
            size,
            path: downloaded(file),
            downloading: local["is_downloading_active"] == true,
            downloaded: local["downloaded_size"].as_u64().unwrap_or_default(),
            uploading: remote["is_uploading_active"] == true,
            uploaded: remote["uploaded_size"].as_u64().unwrap_or_default(),
        };
        if self.files.get(&id) == Some(&new) {
            return Touched::Nothing;
        }
        self.files.insert(id, new);
        Touched::File(id)
    }

    // --- What the list and the feed read ---

    /// A chat FluxGram shows: one with a person or a bot, a group, a channel (part 5); no secret
    /// chats (they live on one device).
    pub fn listed(&self, chat: i64) -> bool {
        let Some(chat) = self.chats.get(&chat) else {
            return false;
        };
        match chat.kind {
            ChatKind::Private { .. }
            | ChatKind::BasicGroup { .. }
            | ChatKind::Supergroup { .. } => true,
            ChatKind::Secret { .. } | ChatKind::Unknown => false,
        }
    }

    /// A channel (part 5): posts without authors, subscribers rather than members.
    pub fn is_channel(&self, chat: i64) -> bool {
        self.chats
            .get(&chat)
            .is_some_and(|chat| matches!(chat.kind, ChatKind::Supergroup { channel: true, .. }))
    }

    /// A chat with a bot (part 5).
    pub fn is_bot(&self, chat: i64) -> bool {
        match self.chats.get(&chat).map(|chat| chat.kind) {
            Some(ChatKind::Private { user }) => self
                .users
                .get(&user)
                .is_some_and(|user| user.kind == UserKind::Bot),
            _ => false,
        }
    }

    /// The user is in the chat (part 5): not a group or a channel they left or were banned from
    /// (or never joined — a public channel opened by its name).
    pub fn joined(&self, chat: i64) -> bool {
        match self.chats.get(&chat).map(|chat| chat.kind) {
            Some(ChatKind::BasicGroup { .. } | ChatKind::Supergroup { .. }) => {
                self.group(chat).is_none_or(|group| {
                    !matches!(
                        group.status.as_str(),
                        "chatMemberStatusLeft" | "chatMemberStatusBanned"
                    )
                })
            }
            _ => true,
        }
    }

    /// A group whose topics show as one feed (part 5).
    pub fn is_forum(&self, chat: i64) -> bool {
        self.group(chat).is_some_and(|group| group.is_forum)
    }

    /// The chat's messages can't be forwarded (part 5): its content is protected.
    pub fn is_protected(&self, chat: i64) -> bool {
        self.chats.get(&chat).is_some_and(|chat| chat.protected)
    }

    /// The chat with the user themself: Saved Messages.
    pub fn is_saved_messages(&self, chat: i64) -> bool {
        self.chats.get(&chat).is_some_and(
            |chat| matches!(chat.kind, ChatKind::Private { user } if Some(user) == self.me),
        )
    }

    /// No notifications from the chat: muted itself, or by its kind's default.
    pub fn muted(&self, chat: i64) -> bool {
        let Some(chat) = self.chats.get(&chat) else {
            return false;
        };
        if !chat.default_mute {
            return chat.mute_for > 0;
        }
        match chat.kind {
            ChatKind::Private { .. } | ChatKind::Secret { .. } => self.private_muted,
            ChatKind::Supergroup { channel: true, .. } => self.channels_muted,
            _ => self.groups_muted,
        }
    }

    /// The group of a group chat.
    pub fn group(&self, chat: i64) -> Option<&Group> {
        match self.chats.get(&chat)?.kind {
            ChatKind::BasicGroup { group } => self.basic_groups.get(&group),
            ChatKind::Supergroup { group, .. } => self.supergroups.get(&group),
            _ => None,
        }
    }

    /// The user may write in the chat: a person's chat (unless the account is deleted), a group
    /// whose permissions or the user's own rights allow it.
    pub fn can_send(&self, chat: i64) -> bool {
        let Some(found) = self.chats.get(&chat) else {
            return false;
        };
        match found.kind {
            ChatKind::Private { user } => self
                .users
                .get(&user)
                .is_none_or(|user| user.kind != UserKind::Deleted),
            // A channel (part 5): its creator, and administrators who may post.
            ChatKind::Supergroup { channel: true, .. } => {
                match self.group(chat).map(|group| group.status.as_str()) {
                    Some("chatMemberStatusCreator") => true,
                    Some("chatMemberStatusAdministrator") => {
                        self.group(chat).is_some_and(|group| group.own_can_post)
                    }
                    _ => false,
                }
            }
            ChatKind::BasicGroup { .. } | ChatKind::Supergroup { .. } => {
                match self.group(chat).map(|group| group.status.as_str()) {
                    Some("chatMemberStatusCreator" | "chatMemberStatusAdministrator") => true,
                    Some("chatMemberStatusLeft" | "chatMemberStatusBanned") => false,
                    Some("chatMemberStatusRestricted") => self
                        .group(chat)
                        .is_some_and(|group| group.restricted_can_send),
                    _ => found.can_send_basic,
                }
            }
            _ => false,
        }
    }

    /// The user may pin and unpin the chat's messages (part 5): anyone in a person's chat; in a
    /// group its creator, an administrator with the right, a member when the chat's permissions
    /// let members (a restricted one by their own).
    pub fn can_pin(&self, chat: i64) -> bool {
        let Some(found) = self.chats.get(&chat) else {
            return false;
        };
        match found.kind {
            ChatKind::Private { .. } => true,
            ChatKind::BasicGroup { .. } | ChatKind::Supergroup { .. } => {
                let Some(group) = self.group(chat) else {
                    return false;
                };
                match group.status.as_str() {
                    "chatMemberStatusCreator" => true,
                    "chatMemberStatusAdministrator" | "chatMemberStatusRestricted" => {
                        group.own_can_pin
                    }
                    "chatMemberStatusMember" => found.can_pin_basic,
                    _ => false,
                }
            }
            _ => false,
        }
    }

    /// The reactions the user may put in the chat (part 5), in order: its own list, or
    /// Telegram's active ones when it allows all ([`FALLBACK_REACTIONS`] until they come). None
    /// in Saved Messages (Telegram's tags there are for Premium) or a channel the user isn't in.
    pub fn reactions(&self, chat: i64) -> Vec<String> {
        let Some(found) = self.chats.get(&chat) else {
            return Vec::new();
        };
        if self.is_saved_messages(chat) || (self.is_channel(chat) && !self.joined(chat)) {
            return Vec::new();
        }
        match &found.reactions {
            Some(some) => some.clone(),
            None if self.active_reactions.is_empty() => FALLBACK_REACTIONS
                .iter()
                .map(|emoji| emoji.to_string())
                .collect(),
            None => self.active_reactions.clone(),
        }
    }

    /// How many reactions of the user's own a message may carry (part 5): one, three with
    /// Telegram Premium (Telegram's defaults), never more than the chat lets a message carry.
    pub fn reaction_limit(&self, chat: i64) -> usize {
        let premium = self
            .me
            .and_then(|me| self.users.get(&me))
            .is_some_and(|me| me.premium);
        let own = if premium { 3 } else { 1 };
        match self.chats.get(&chat).map(|found| found.max_reactions) {
            Some(max) if max > 0 => own.min(max as usize),
            _ => own,
        }
    }

    /// A user's name; empty when the user is unknown or deleted.
    pub fn user_name(&self, user: i64) -> String {
        self.users.get(&user).map(User::name).unwrap_or_default()
    }

    /// A downloaded file's path.
    pub fn file_path(&self, file: i64) -> Option<&str> {
        self.files.get(&file)?.path.as_deref()
    }

    /// A chat's picture: its photo once downloaded, else its initials on its color; Saved
    /// Messages' — the bookmark.
    pub fn chat_avatar(&self, chat: i64) -> AvatarSpec {
        let Some(found) = self.chats.get(&chat) else {
            return AvatarSpec {
                image: None,
                initials: "?".into(),
                color: 0,
            };
        };
        if let Some(icon) = self
            .saved_icon
            .0
            .as_ref()
            .filter(|_| self.is_saved_messages(chat))
        {
            return AvatarSpec {
                image: Some(icon.clone()),
                initials: initials(&found.title),
                color: found.color,
            };
        }
        AvatarSpec {
            image: found
                .photo
                .and_then(|file| self.file_path(file))
                .map(str::to_string),
            initials: initials(&found.title),
            color: found.color,
        }
    }

    /// A user's picture: the photo once downloaded, else the initials on the user's color.
    pub fn user_avatar(&self, user: i64) -> AvatarSpec {
        let Some(found) = self.users.get(&user) else {
            return AvatarSpec {
                image: None,
                initials: "?".into(),
                color: user.rem_euclid(7) as u8,
            };
        };
        AvatarSpec {
            image: found
                .photo
                .and_then(|file| self.file_path(file))
                .map(str::to_string),
            initials: initials(&found.name()),
            color: found.color,
        }
    }

    /// The chat's small photo while it isn't downloaded: TDLib's file id, to download.
    pub fn chat_photo_missing(&self, chat: i64) -> Option<i64> {
        if self.saved_icon.0.is_some() && self.is_saved_messages(chat) {
            return None;
        }
        let file = self.chats.get(&chat)?.photo?;
        self.file_path(file).is_none().then_some(file)
    }
}

/// The chat id of a basic group (`-id`) or a supergroup (`-100…`), as TDLib makes them.
pub fn group_chat_id(group: i64, supergroup: bool) -> i64 {
    if supergroup {
        -1_000_000_000_000 - group
    } else {
        -group
    }
}

fn text(value: &Value, key: &str) -> String {
    value[key].as_str().unwrap_or_default().to_string()
}

fn count(value: &Value) -> u32 {
    value.as_u64().unwrap_or_default().min(u32::MAX as u64) as u32
}

/// Telegram's built-in name colors are 0–6, as the avatar colors; the rest are the Premium ones —
/// the classic rule (by id) then.
fn color(accent: &Value, id: i64) -> u8 {
    match accent.as_i64() {
        Some(accent) if (0..7).contains(&accent) => accent as u8,
        _ => id.rem_euclid(7) as u8,
    }
}

/// The small photo of a `profilePhoto` or a `chatPhotoInfo`.
fn small_photo(photo: &Value) -> Option<i64> {
    photo["small"]["id"].as_i64()
}

fn message_of(message: &Value) -> Option<Value> {
    (message.is_object()).then(|| message.clone())
}

fn parse_status(status: &Value) -> Status {
    match status["@type"].as_str().unwrap_or_default() {
        "userStatusOnline" => Status::Online {
            expires: status["expires"].as_i64().unwrap_or_default(),
        },
        "userStatusOffline" => Status::Offline {
            was: status["was_online"].as_i64().unwrap_or_default(),
        },
        "userStatusRecently" => Status::Recently,
        "userStatusLastWeek" => Status::LastWeek,
        "userStatusLastMonth" => Status::LastMonth,
        _ => Status::Empty,
    }
}

/// A forum topic's icon color (RGB; Telegram offers six) as the nearest avatar color: red 0,
/// orange 1, violet 2, green 3, cyan 4, blue 5, pink 6.
pub fn topic_color(rgb: i64) -> u8 {
    match rgb {
        0x6FB9F0 => 5,
        0xFFD67E => 1,
        0xCB86DB => 2,
        0x8EEE98 => 3,
        0xFF93B2 => 6,
        0xFB6F5F => 0,
        _ => {
            let (r, g, b) = ((rgb >> 16) & 0xFF, (rgb >> 8) & 0xFF, rgb & 0xFF);
            match (r, g, b) {
                _ if r > 200 && g < 140 && b < 140 => 0,
                _ if r > 200 && g > 150 && b < 150 => 1,
                _ if g > r && g > b => 3,
                _ if r > 150 && b > 150 && g < 150 => 2,
                _ if b > r && g > 150 => 4,
                _ => 5,
            }
        }
    }
}

/// `chatPermissions` let members send text.
fn can_send(permissions: &Value) -> bool {
    permissions["can_send_basic_messages"] == true
}

/// A chat's `available_reactions` (part 5): all of Telegram's, or some (custom emoji and the paid
/// one left out — they are only shown).
fn apply_reactions(chat: &mut Chat, available: &Value) {
    chat.max_reactions = available["max_reaction_count"].as_u64().unwrap_or_default() as u32;
    chat.reactions = match available["@type"].as_str().unwrap_or_default() {
        "chatAvailableReactionsSome" => Some(
            available["reactions"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|reaction| reaction["@type"] == "reactionTypeEmoji")
                .filter_map(|reaction| reaction["emoji"].as_str())
                .map(str::to_string)
                .collect(),
        ),
        _ => None,
    };
}

fn apply_notifications(chat: &mut Chat, settings: &Value) {
    if settings.is_null() {
        chat.default_mute = true;
        return;
    }
    chat.default_mute = settings["use_default_mute_for"] == true;
    chat.mute_for = settings["mute_for"].as_i64().unwrap_or_default();
}

/// The text of a `draftMessage` (an `inputMessageText` in it).
fn draft_of(draft: &Value) -> Option<String> {
    let text = draft["input_message_text"]["text"]["text"].as_str()?;
    (!text.is_empty()).then(|| text.to_string())
}

fn apply_positions(chat: &mut Chat, positions: &Value) {
    let Some(positions) = positions.as_array() else {
        return;
    };
    // The update lists every position the chat has now: the lists not named lost it.
    chat.order = 0;
    chat.pinned = false;
    chat.archive_order = 0;
    for position in positions {
        apply_position(chat, position);
    }
}

fn apply_position(chat: &mut Chat, position: &Value) {
    // `order` is an int64, as a string; 0 — the chat left the list.
    let order = match &position["order"] {
        Value::String(order) => order.parse().unwrap_or_default(),
        order => order.as_i64().unwrap_or_default(),
    };
    match position["list"]["@type"].as_str().unwrap_or_default() {
        "chatListMain" => {
            chat.order = order;
            chat.pinned = order != 0 && position["is_pinned"] == true;
        }
        "chatListArchive" => chat.archive_order = order,
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn file(id: i64, path: &str) -> Value {
        json!({"@type": "file", "id": id, "size": 1000, "expected_size": 1000,
            "local": {"path": path, "is_downloading_active": false,
                "is_downloading_completed": !path.is_empty(), "downloaded_size": 0},
            "remote": {"is_uploading_active": false, "uploaded_size": 0}})
    }

    fn group_chat() -> Value {
        json!({"@type": "chat", "id": -1000000001001i64, "title": "Backend · team", "accent_color_id": 5,
            "type": {"@type": "chatTypeSupergroup", "supergroup_id": 1001, "is_channel": false},
            "photo": {"small": file(7, "")},
            "positions": [{"@type": "chatPosition", "list": {"@type": "chatListMain"},
                "order": "9221294784512000005", "is_pinned": true}],
            "unread_count": 3, "unread_mention_count": 1,
            "last_read_inbox_message_id": 100, "last_read_outbox_message_id": 90,
            "notification_settings": {"use_default_mute_for": true, "mute_for": 0},
            "permissions": {"can_send_basic_messages": true},
            "last_message": {"@type": "message", "id": 103, "chat_id": -1000000001001i64}})
    }

    #[test]
    fn lists_the_reactions_a_chat_allows_and_the_users_limit() {
        let chat = -1000000001001i64;
        let mut store = Store {
            me: Some(777),
            ..Store::default()
        };
        store.apply(&group_chat());
        // All of Telegram's: the fallback until the active ones come, then those.
        assert_eq!(store.reactions(chat).len(), FALLBACK_REACTIONS.len());
        assert_eq!(
            store.apply(&json!({"@type": "updateActiveEmojiReactions", "emojis": ["👍", "🔥"]})),
            Touched::Scope
        );
        assert_eq!(store.reactions(chat), ["👍", "🔥"]);
        // Some: those (a custom one left out); none — empty.
        store.apply(
            &json!({"@type": "updateChatAvailableReactions", "chat_id": chat,
            "available_reactions": {"@type": "chatAvailableReactionsSome", "reactions": [
                {"@type": "reactionTypeEmoji", "emoji": "🎉"},
                {"@type": "reactionTypeCustomEmoji", "custom_emoji_id": "5"}],
                "max_reaction_count": 2}}),
        );
        assert_eq!(store.reactions(chat), ["🎉"]);
        store.apply(
            &json!({"@type": "updateChatAvailableReactions", "chat_id": chat,
            "available_reactions": {"@type": "chatAvailableReactionsSome", "reactions": [],
                "max_reaction_count": 11}}),
        );
        assert!(store.reactions(chat).is_empty());
        // One of the user's own; three with Premium — never more than the chat lets.
        assert_eq!(store.reaction_limit(chat), 1);
        store.apply(&json!({"@type": "user", "id": 777, "first_name": "Me", "is_premium": true}));
        assert_eq!(store.reaction_limit(chat), 3);
        store.chats.get_mut(&chat).unwrap().max_reactions = 2;
        assert_eq!(store.reaction_limit(chat), 2);
    }

    #[test]
    fn keeps_a_chat_and_its_updates() {
        let mut store = Store::default();
        assert_eq!(store.apply(&group_chat()), Touched::Chat(-1000000001001i64));
        // The same object again changes nothing.
        assert_eq!(store.apply(&group_chat()), Touched::Nothing);
        let chat = &store.chats[&-1000000001001i64];
        assert_eq!(chat.order, 9221294784512000005);
        assert!(chat.pinned);
        assert_eq!(chat.unread_count, 3);
        assert_eq!(chat.photo, Some(7));
        assert!(store.listed(-1000000001001i64));

        store.apply(
            &json!({"@type": "updateChatReadInbox", "chat_id": -1000000001001i64,
            "last_read_inbox_message_id": 103, "unread_count": 0}),
        );
        store.apply(
            &json!({"@type": "updateChatTitle", "chat_id": -1000000001001i64, "title": "Backend"}),
        );
        store.apply(
            &json!({"@type": "updateChatPosition", "chat_id": -1000000001001i64,
            "position": {"list": {"@type": "chatListMain"}, "order": "0", "is_pinned": false}}),
        );
        let chat = &store.chats[&-1000000001001i64];
        assert_eq!(chat.unread_count, 0);
        assert_eq!(chat.last_read_inbox, 103);
        assert_eq!(chat.title, "Backend");
        assert_eq!((chat.order, chat.pinned), (0, false));
        // An update of a chat the store doesn't know: nothing.
        assert_eq!(
            store.apply(&json!({"@type": "updateChatTitle", "chat_id": 5, "title": "x"})),
            Touched::Nothing
        );
    }

    #[test]
    fn lists_channels_and_bots_but_no_secret_chats() {
        let mut store = Store::default();
        store.apply(&json!({"@type": "chat", "id": -1002,
            "type": {"@type": "chatTypeSupergroup", "supergroup_id": 2, "is_channel": true}}));
        store.apply(&json!({"@type": "user", "id": 105, "first_name": "Weather",
            "type": {"@type": "userTypeBot"}}));
        store.apply(&json!({"@type": "chat", "id": 105,
            "type": {"@type": "chatTypePrivate", "user_id": 105}}));
        store.apply(&json!({"@type": "chat", "id": -2000000000001i64,
            "type": {"@type": "chatTypeSecret", "secret_chat_id": 1, "user_id": 101}}));
        store.apply(&json!({"@type": "chat", "id": 101,
            "type": {"@type": "chatTypePrivate", "user_id": 101}}));
        assert!(store.listed(-1002));
        assert!(store.listed(105));
        assert!(!store.listed(-2000000000001));
        assert!(store.listed(101));
        assert!(store.is_channel(-1002) && !store.is_channel(101));
        assert!(store.is_bot(105) && !store.is_bot(101));
    }

    #[test]
    fn keeps_a_forums_topics() {
        let mut store = Store::default();
        let touched = store.apply(
            &json!({"@type": "forumTopics", "total_count": 2, "topics": [
            {"@type": "forumTopic", "info": {"@type": "forumTopicInfo", "chat_id": -1006,
                "forum_topic_id": 1, "name": "General", "is_general": true,
                "icon": {"@type": "forumTopicIcon", "color": 0x6FB9F0}}},
            {"@type": "forumTopic", "info": {"@type": "forumTopicInfo", "chat_id": -1006,
                "forum_topic_id": 7, "name": "Design", "is_closed": true,
                "icon": {"@type": "forumTopicIcon", "color": 0xFF93B2}}}]}),
        );
        assert_eq!(touched, Touched::Chat(-1006));
        assert_eq!(store.topics[&(-1006, 1)].color, 5);
        assert!(store.topics[&(-1006, 1)].general);
        assert_eq!(store.topics[&(-1006, 7)].name, "Design");
        assert_eq!(store.topics[&(-1006, 7)].color, 6);
        assert!(store.topics[&(-1006, 7)].closed);
        // A renamed topic.
        store.apply(
            &json!({"@type": "updateForumTopicInfo", "info": {"@type": "forumTopicInfo",
            "chat_id": -1006, "forum_topic_id": 7, "name": "UI",
            "icon": {"@type": "forumTopicIcon", "color": 0x8EEE98}}}),
        );
        assert_eq!(store.topics[&(-1006, 7)].name, "UI");
        assert_eq!(topic_color(0x8EEE98), 3);
        assert_eq!(topic_color(0xF0F040), 1);
    }

    #[test]
    fn mutes_by_the_chat_or_its_scope() {
        let mut store = Store::default();
        store.apply(&group_chat());
        assert!(!store.muted(-1000000001001i64));
        assert_eq!(
            store.apply(&json!({"@type": "updateScopeNotificationSettings",
                "scope": {"@type": "notificationSettingsScopeGroupChats"},
                "notification_settings": {"mute_for": 3600}})),
            Touched::Scope
        );
        assert!(store.muted(-1000000001001i64));
        store.apply(
            &json!({"@type": "updateChatNotificationSettings", "chat_id": -1000000001001i64,
            "notification_settings": {"use_default_mute_for": false, "mute_for": 0}}),
        );
        assert!(!store.muted(-1000000001001i64));
    }

    #[test]
    fn knows_who_may_write() {
        let mut store = Store::default();
        store.apply(&group_chat());
        assert!(store.can_send(-1000000001001i64));
        let touched = store.apply(&json!({"@type": "updateSupergroup", "supergroup": {
            "id": 1001, "member_count": 5, "is_channel": false,
            "status": {"@type": "chatMemberStatusLeft"}}}));
        assert_eq!(touched, Touched::Chat(-1000000001001i64));
        assert_eq!(group_chat_id(1001, true), -1000000001001i64);
        assert_eq!(
            store.group(-1000000001001i64).map(|g| g.member_count),
            Some(5)
        );
        assert!(!store.can_send(-1000000001001i64));
    }

    #[test]
    fn follows_files_and_avatars() {
        let mut store = Store::default();
        store.apply(&group_chat());
        assert_eq!(store.chat_photo_missing(-1000000001001i64), Some(7));
        let avatar = store.chat_avatar(-1000000001001i64);
        assert_eq!((avatar.image, avatar.color), (None, 5));
        let mut downloading = file(7, "");
        downloading["local"]["is_downloading_active"] = json!(true);
        downloading["local"]["downloaded_size"] = json!(250);
        assert_eq!(
            store.apply(&json!({"@type": "updateFile", "file": downloading})),
            Touched::File(7)
        );
        assert_eq!(store.files[&7].progress(), Some(0.25));
        store.apply(&json!({"@type": "updateFile", "file": file(7, "/tmp/7.jpg")}));
        assert_eq!(store.chat_photo_missing(-1000000001001i64), None);
        assert_eq!(
            store.chat_avatar(-1000000001001i64).image.as_deref(),
            Some("/tmp/7.jpg")
        );
        // Saved Messages: the bookmark, whatever photo the user has.
        store.me = Some(777);
        store.apply(&json!({"@type": "chat", "id": 777, "title": "Egor",
            "type": {"@type": "chatTypePrivate", "user_id": 777}, "photo": {"small": file(8, "")}}));
        assert_eq!(store.chat_avatar(777).image, None, "no bookmark written");
        store.saved_icon = SavedIcon(Some("/data/saved-messages.png".into()));
        assert_eq!(
            store.chat_avatar(777).image.as_deref(),
            Some("/data/saved-messages.png")
        );
        assert_eq!(store.chat_photo_missing(777), None);
        assert!(store.chat_avatar(-1000000001001i64).image.is_some());
    }

    #[test]
    fn keeps_users_and_their_status() {
        let mut store = Store::default();
        store.apply(
            &json!({"@type": "updateUser", "user": {"@type": "user", "id": 101,
            "first_name": "Anna", "last_name": "Smirnova", "accent_color_id": 9,
            "usernames": {"active_usernames": ["anna"]},
            "status": {"@type": "userStatusOffline", "was_online": 1763380000},
            "type": {"@type": "userTypeRegular"}}}),
        );
        let anna = &store.users[&101];
        assert_eq!(anna.name(), "Anna Smirnova");
        assert_eq!(anna.username.as_deref(), Some("anna"));
        // A Premium color: by the id.
        assert_eq!(anna.color, (101 % 7) as u8);
        assert_eq!(
            store.apply(&json!({"@type": "updateUserStatus", "user_id": 101,
                "status": {"@type": "userStatusOnline", "expires": 1763380300}})),
            Touched::User(101)
        );
        assert_eq!(
            store.users[&101].status,
            Status::Online {
                expires: 1763380300
            }
        );
    }

    #[test]
    fn reads_drafts() {
        let mut store = Store::default();
        store.apply(&group_chat());
        store.apply(
            &json!({"@type": "updateChatDraftMessage", "chat_id": -1000000001001i64,
            "draft_message": {"date": 1, "input_message_text": {"@type": "inputMessageText",
                "text": {"text": "half a thought", "entities": []}}},
            "positions": []}),
        );
        assert_eq!(
            store.chats[&-1000000001001i64].draft.as_deref(),
            Some("half a thought")
        );
        store.apply(
            &json!({"@type": "updateChatDraftMessage", "chat_id": -1000000001001i64,
            "draft_message": null, "positions": []}),
        );
        assert_eq!(store.chats[&-1000000001001i64].draft, None);
    }
}
