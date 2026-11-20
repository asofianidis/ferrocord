//! Client state: guilds, channels, messages, read state and members.
//!
//! The store lives on the UI thread as a GPUI entity. Network work runs on a
//! Tokio runtime; results are funnelled back here and announced to views as
//! fine-grained [`StoreEvent`]s so that only the affected view re-renders.

use crate::discord::{
    self, Ban, Channel, Emoji, Gif, Guild, Id, Member, Message, Overwrite, Reaction, Role, Status, StickerItem, User, Webhook,
    automod::{self, Rule},
    events::ScheduledEvent,
    VoiceState, channel_kind,
    commands::{self, SlashCommand},
    gateway::{self, Command, Event},
    proto::Folder,
    permissions,
    rest::{HttpMethod, Rest, Upload},
};
use crate::markdown::{self, Doc};
use anyhow::{Result, anyhow};
use chrono::{DateTime, Local};
use gpui::{Context, EventEmitter};
use std::{
    collections::HashMap,
    future::Future,
    rc::Rc,
    time::{Duration, Instant},
};
use tokio::sync::mpsc;
use crate::voice::{ServerInfo, VoiceConnection, VoiceEvent};

mod demo;

const PAGE: u32 = 50;
const TYPING_TTL: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, PartialEq)]
pub enum Connection {
    LoggedOut,
    Connecting,
    Ready,
    Reconnecting,
    Failed(String),
}

#[derive(Clone, Debug)]
pub enum MessageChange {
    /// The whole list was replaced.
    Reset,
    Appended(usize),
    Prepended(usize),
    Updated(usize),
    Removed(usize),
}

#[derive(Clone, Debug)]
pub enum StoreEvent {
    Connection,
    Guilds,
    Channels,
    Selection,
    Unread,
    Messages { channel: Id, change: MessageChange },
    Typing(Id),
    Members(Id),
    Friends,
    /// Voice connection, voice members or speaking indicators changed.
    Voice,
    /// Someone is ringing us in a direct-message call.
    IncomingCall { channel: Id, title: String },
    /// The slash command list for a server or conversation arrived.
    Commands,
    /// An informational toast.
    Notice(String),
    /// Text to place on the clipboard, with a confirmation to show.
    Clipboard { text: String, notice: String },
    /// Someone mentioned or messaged the user in a channel that is not open.
    Mention { title: String, body: String },
    Error(String),
}

/// A message plus everything derived from it that is expensive to recompute
/// while rendering.
pub struct Msg {
    pub m: Message,
    pub doc: Doc,
    pub time: DateTime<Local>,
    pub pending: bool,
    pub failed: bool,
    pub mentions_me: bool,
}

#[derive(Default)]
pub struct ChannelMessages {
    pub list: Vec<Rc<Msg>>,
    pub loaded: bool,
    pub loading: bool,
    pub reached_start: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKind {
    Category,
    Channel,
    Thread,
    /// Someone connected to the voice channel above; the row id is a user id.
    VoiceUser,
}

#[derive(Clone, Copy, Debug)]
pub struct ChannelRow {
    pub id: Id,
    pub kind: RowKind,
}

pub struct GuildState {
    pub guild: Guild,
    pub own_roles: Vec<Id>,
    /// Channels the current user may see, in display order.
    pub rows: Vec<ChannelRow>,
}

#[derive(Clone, Debug, Default)]
pub struct MemberInfo {
    pub nick: Option<String>,
    pub color: Option<u32>,
    pub roles: Vec<Id>,
}

#[derive(Clone, Debug)]
pub enum MemberRow {
    Group { name: String, count: u32 },
    Member { user: User, nick: Option<String>, color: Option<u32>, status: Status },
    Placeholder,
}

#[derive(Clone, Debug, Default)]
pub struct VoiceMember {
    pub user: Id,
    pub self_mute: bool,
    pub self_deaf: bool,
    pub mute: bool,
    pub deaf: bool,
    /// On a stage: in the audience.
    pub suppress: bool,
    pub hand_raised: bool,
    pub video: bool,
    pub streaming: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum VoiceStatus {
    /// Waiting for Discord to assign a voice server.
    Joining,
    Connecting,
    Connected,
    Failed(String),
}

#[derive(Clone, Copy, Debug)]
pub enum VoiceModeration {
    Mute(bool),
    Deafen(bool),
    Disconnect,
}

pub struct VoiceSession {
    pub channel: Id,
    pub guild: Option<Id>,
    pub status: VoiceStatus,
    session_id: Option<String>,
    /// Token and endpoint from `VOICE_SERVER_UPDATE`.
    server: Option<(String, String)>,
}

#[derive(Default)]
pub struct ReadMarker {
    pub last_read: Option<Id>,
    pub mentions: u32,
}

pub struct TypingUser {
    pub user: Id,
    pub name: String,
    pub at: Instant,
}

pub struct Store {
    runtime: tokio::runtime::Handle,
    rest: Option<Rest>,
    commands: Option<mpsc::UnboundedSender<Command>>,
    session: u64,
    /// Offline sample data is loaded; nothing is sent anywhere.
    demo: bool,

    pub connection: Connection,
    pub me: Option<User>,
    pub guilds: Vec<GuildState>,
    pub channels: HashMap<Id, Channel>,
    pub dms: Vec<Id>,
    pub messages: HashMap<Id, ChannelMessages>,
    pub read: HashMap<Id, ReadMarker>,
    pub users: HashMap<Id, User>,
    pub presences: HashMap<Id, Status>,
    pub typing: HashMap<Id, Vec<TypingUser>>,
    pub member_info: HashMap<(Id, Id), MemberInfo>,
    pub member_lists: HashMap<Id, Vec<MemberRow>>,
    pub collapsed: std::collections::HashSet<Id>,
    pub show_members: bool,
    /// Who is connected to each voice channel (or direct-message call).
    pub voice: HashMap<Id, Vec<VoiceMember>>,
    pub voice_session: Option<VoiceSession>,
    voice_conn: Option<VoiceConnection>,
    voice_epoch: u64,
    pub self_mute: bool,
    pub self_deaf: bool,
    pub speaking: std::collections::HashSet<Id>,
    /// Direct-message calls in progress, with who is being rung.
    pub calls: HashMap<Id, Vec<Id>>,
    /// Live stages and their topics.
    pub stages: HashMap<Id, String>,
    pub my_status: Status,
    /// Scheduled events per server.
    pub events: HashMap<Id, Vec<ScheduledEvent>>,
    /// Events the user marked as interested.
    pub interested: std::collections::HashSet<Id>,
    /// Progress of an email-and-password sign-in, and its latest error.
    pub auth_step: AuthStep,
    pub auth_error: Option<String>,
    /// The session of the sign-in in progress; two-factor codes must use it.
    auth_session: Option<std::sync::Arc<discord::auth::Auth>>,
    /// About-me text and pronouns, fetched when settings open (user accounts).
    pub my_profile: Option<Profile>,
    gateway_session: String,
    /// Relationship type per user: 1 friend, 2 blocked, 3 incoming, 4 outgoing.
    pub relationships: Vec<(Id, u8)>,
    pub muted: std::collections::HashSet<Id>,
    /// The friends page is showing instead of a conversation.
    pub show_friends: bool,
    /// Slash commands, keyed by server id (or channel id for conversations).
    slash_commands: HashMap<Id, Rc<Vec<SlashCommand>>>,
    pub folders: Vec<Folder>,
    pub collapsed_folders: std::collections::HashSet<i64>,
    forums_loaded: std::collections::HashSet<Id>,

    /// `None` is the direct-messages view.
    pub selected_guild: Option<Id>,
    pub selected_channel: Option<Id>,
    last_channel: HashMap<Option<Id>, Id>,
    subscribed: Option<(Id, Id)>,
}

impl EventEmitter<StoreEvent> for Store {}

pub struct AuditLog {
    pub entries: Vec<discord::audit::AuditEntry>,
    /// Users the log mentions, for naming targets.
    pub users: HashMap<Id, User>,
}

/// Where an email-and-password sign-in has got to.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum AuthStep {
    #[default]
    Idle,
    /// Waiting for Discord.
    Working,
    /// Discord wants a second factor.
    Code {
        ticket: String,
        /// The masked phone number a code was texted to, when using SMS.
        texted_to: Option<String>,
        /// Whether an authenticator code is accepted (and SMS can be offered).
        totp: bool,
        sms: bool,
    },
}

#[derive(Clone, Default, Debug, PartialEq)]
pub struct Profile {
    pub bio: String,
    pub pronouns: String,
}

struct StoreResolver<'a> {
    store: &'a Store,
    guild: Option<Id>,
    mentions: &'a [User],
}

impl markdown::Resolver for StoreResolver<'_> {
    fn user(&self, id: Id) -> Option<String> {
        let user = self.mentions.iter().find(|u| u.id == id).or_else(|| self.store.users.get(&id))?;
        Some(self.store.display_name(self.guild, user))
    }

    fn channel(&self, id: Id) -> Option<String> {
        self.store.channels.get(&id).and_then(|c| c.name.clone())
    }

    fn role(&self, id: Id) -> Option<String> {
        let guild = self.store.guild(self.guild?)?;
        guild.guild.roles.iter().find(|r| r.id == id).map(|r| r.name.clone())
    }
}

impl Store {
    pub fn new(runtime: tokio::runtime::Handle) -> Self {
        Self {
            runtime,
            rest: None,
            commands: None,
            session: 0,
            demo: false,
            connection: Connection::LoggedOut,
            me: None,
            guilds: Vec::new(),
            channels: HashMap::new(),
            dms: Vec::new(),
            messages: HashMap::new(),
            read: HashMap::new(),
            users: HashMap::new(),
            presences: HashMap::new(),
            typing: HashMap::new(),
            member_info: HashMap::new(),
            member_lists: HashMap::new(),
            collapsed: Default::default(),
            show_members: true,
            voice: HashMap::new(),
            voice_session: None,
            voice_conn: None,
            voice_epoch: 0,
            self_mute: false,
            self_deaf: false,
            speaking: Default::default(),
            calls: HashMap::new(),
            stages: HashMap::new(),
            my_status: Status::Online,
            my_profile: None,
            auth_step: AuthStep::Idle,
            auth_error: None,
            auth_session: None,
            events: HashMap::new(),
            interested: Default::default(),
            gateway_session: String::new(),
            relationships: Vec::new(),
            muted: crate::settings::Settings::load().muted.into_iter().map(Id).collect(),
            show_friends: false,
            slash_commands: HashMap::new(),
            folders: Vec::new(),
            collapsed_folders: Default::default(),
            forums_loaded: Default::default(),
            selected_guild: None,
            selected_channel: None,
            last_channel: HashMap::new(),
            subscribed: None,
        }
    }

    // ---------------------------------------------------------------- session

    // ------------------------------------------------------- password sign-in

    fn auth_failed(&mut self, error: impl Into<String>, cx: &mut Context<Self>) {
        if !matches!(self.auth_step, AuthStep::Code { .. }) {
            self.auth_step = AuthStep::Idle;
        }
        self.auth_error = Some(error.into());
        cx.notify();
    }

    /// Finishes a sign-in: the token is kept, the password never was.
    fn auth_succeeded(&mut self, token: String, cx: &mut Context<Self>) {
        self.auth_step = AuthStep::Idle;
        self.auth_error = None;
        self.auth_session = None;
        crate::credentials::save(&token, false);
        self.login(token, false, cx);
    }

    /// Signs in with an email address or phone number and a password.
    pub fn password_login(&mut self, login: String, password: String, cx: &mut Context<Self>) {
        if login.is_empty() || password.is_empty() {
            return self.auth_failed("Enter your email or phone number and your password.", cx);
        }
        let auth = match discord::auth::Auth::new() {
            Ok(auth) => std::sync::Arc::new(auth),
            Err(e) => return self.auth_failed(e.to_string(), cx),
        };
        self.auth_session = Some(auth.clone());
        self.auth_step = AuthStep::Working;
        self.auth_error = None;
        cx.notify();
        self.run(
            cx,
            async move {
                let outcome = auth.login(&login, &password).await?;
                // With only SMS available, text the code straight away.
                let texted = match &outcome {
                    discord::auth::Outcome::Mfa(mfa) if mfa.sms && !mfa.totp => Some(auth.send_sms(&mfa.ticket).await?),
                    _ => None,
                };
                Ok((outcome, texted))
            },
            |store, result, cx| match result {
                Ok((discord::auth::Outcome::Token(token), _)) => store.auth_succeeded(token, cx),
                Ok((discord::auth::Outcome::Mfa(mfa), texted_to)) => {
                    if !mfa.totp && !mfa.sms {
                        let why = if mfa.webauthn { "a security key or passkey" } else { "a method Ferrocord does not support" };
                        return store.auth_failed(
                            format!("Your account's two-factor login uses {why}. Sign in with a token instead."),
                            cx,
                        );
                    }
                    store.auth_step =
                        AuthStep::Code { ticket: mfa.ticket, texted_to, totp: mfa.totp, sms: mfa.sms };
                    store.auth_error = None;
                    cx.notify();
                }
                Err(e) => store.auth_failed(e.to_string(), cx),
            },
        );
    }

    /// Sends the two-factor code: an authenticator or backup code, or the texted code.
    pub fn submit_auth_code(&mut self, code: String, cx: &mut Context<Self>) {
        let AuthStep::Code { ticket, texted_to, .. } = self.auth_step.clone() else { return };
        let code: String = code.chars().filter(|c| !c.is_whitespace() && *c != '-').collect();
        if code.is_empty() {
            return;
        }
        let Some(auth) = self.auth_session.clone() else { return };
        self.auth_error = None;
        cx.notify();
        let by_sms = texted_to.is_some();
        self.run(
            cx,
            async move { if by_sms { auth.sms(&ticket, &code).await } else { auth.totp(&ticket, &code).await } },
            |store, result, cx| match result {
                Ok(token) => store.auth_succeeded(token, cx),
                Err(e) => store.auth_failed(e.to_string(), cx),
            },
        );
    }

    /// Switches the second step to a texted code.
    pub fn text_auth_code(&mut self, cx: &mut Context<Self>) {
        let AuthStep::Code { ticket, totp, sms, .. } = self.auth_step.clone() else { return };
        let Some(auth) = self.auth_session.clone() else { return };
        self.run(cx, async move { auth.send_sms(&ticket).await }, move |store, result, cx| match result {
            Ok(phone) => {
                if let AuthStep::Code { ticket, .. } = store.auth_step.clone() {
                    store.auth_step = AuthStep::Code { ticket, texted_to: Some(phone), totp, sms };
                    store.auth_error = None;
                    cx.notify();
                }
            }
            Err(e) => store.auth_failed(e.to_string(), cx),
        });
    }

    pub fn cancel_auth(&mut self, cx: &mut Context<Self>) {
        self.auth_step = AuthStep::Idle;
        self.auth_error = None;
        self.auth_session = None;
        cx.notify();
    }

    pub fn login(&mut self, token: String, is_bot: bool, cx: &mut Context<Self>) {
        self.shutdown();
        let rest = match Rest::new(&token, is_bot) {
            Ok(rest) => rest,
            Err(e) => {
                self.connection = Connection::Failed(e.to_string());
                cx.emit(StoreEvent::Connection);
                return;
            }
        };
        self.rest = Some(rest);
        self.session += 1;
        let session = self.session;
        self.connection = Connection::Connecting;
        cx.emit(StoreEvent::Connection);

        let (event_tx, event_rx) = async_channel::unbounded();
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        self.commands = Some(command_tx);
        self.runtime.spawn(gateway::run(token, is_bot, event_tx, command_rx));

        cx.spawn(async move |this, cx| {
            while let Ok(event) = event_rx.recv().await {
                let alive = this.update(cx, |store, cx| {
                    if store.session != session {
                        return false;
                    }
                    store.apply(event, cx);
                    true
                });
                if !matches!(alive, Ok(true)) {
                    break;
                }
            }
        })
        .detach();

        // Expire typing indicators.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(2)).await;
                let alive = this.update(cx, |store, cx| {
                    if store.session != session {
                        return false;
                    }
                    store.prune_typing(cx);
                    true
                });
                if !matches!(alive, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    fn shutdown(&mut self) {
        if let Some(commands) = self.commands.take() {
            let _ = commands.send(Command::Shutdown);
        }
        self.rest = None;
    }

    pub fn logout(&mut self, cx: &mut Context<Self>) {
        self.leave_voice(cx);
        self.shutdown();
        // Keep the counter monotonic so work from the old session can never match a new one.
        let session = self.session + 1;
        let runtime = self.runtime.clone();
        *self = Store::new(runtime);
        self.session = session;
        cx.emit(StoreEvent::Connection);
        cx.emit(StoreEvent::Selection);
        cx.notify();
    }

    /// Runs a network future on the Tokio runtime and delivers its result back
    /// to the store on the UI thread.
    fn run<T: Send + 'static>(
        &self,
        cx: &mut Context<Self>,
        fut: impl Future<Output = Result<T>> + Send + 'static,
        then: impl FnOnce(&mut Self, Result<T>, &mut Context<Self>) + 'static,
    ) {
        let session = self.session;
        let join = self.runtime.spawn(fut);
        cx.spawn(async move |this, cx| {
            let result = match join.await {
                Ok(result) => result,
                Err(e) => Err(anyhow!("task failed: {e}")),
            };
            let _ = this.update(cx, |store, cx| {
                if store.session == session {
                    then(store, result, cx);
                }
            });
        })
        .detach();
    }

    fn report<T>(&self, result: Result<T>, what: &str, cx: &mut Context<Self>) -> Option<T> {
        match result {
            Ok(v) => Some(v),
            Err(e) => {
                log::warn!("{what}: {e}");
                cx.emit(StoreEvent::Error(format!("{what}: {e}")));
                None
            }
        }
    }

    // ---------------------------------------------------------------- lookups

    pub fn guild(&self, id: Id) -> Option<&GuildState> {
        self.guilds.iter().find(|g| g.guild.id == id)
    }

    fn guild_mut(&mut self, id: Id) -> Option<&mut GuildState> {
        self.guilds.iter_mut().find(|g| g.guild.id == id)
    }

    pub fn my_id(&self) -> Id {
        self.me.as_ref().map(|u| u.id).unwrap_or_default()
    }

    pub fn display_name(&self, guild: Option<Id>, user: &User) -> String {
        guild
            .and_then(|g| self.member_info.get(&(g, user.id)))
            .and_then(|m| m.nick.clone())
            .unwrap_or_else(|| user.display_name().to_string())
    }

    pub fn name_color(&self, guild: Option<Id>, user: Id) -> Option<u32> {
        self.member_info.get(&(guild?, user)).and_then(|m| m.color)
    }

    pub fn channel_title(&self, channel: &Channel) -> String {
        if let Some(name) = channel.name.as_ref().filter(|n| !n.is_empty()) {
            return name.clone();
        }
        let me = self.my_id();
        let names: Vec<&str> = channel
            .recipients
            .iter()
            .filter(|u| u.id != me)
            .map(|u| u.display_name())
            .collect();
        if names.is_empty() { "Unnamed".into() } else { names.join(", ") }
    }

    pub fn dm_peer<'a>(&self, channel: &'a Channel) -> Option<&'a User> {
        let me = self.my_id();
        (channel.kind == channel_kind::DM)
            .then(|| channel.recipients.iter().find(|u| u.id != me))
            .flatten()
    }

    pub fn status_of(&self, user: Id) -> Status {
        self.presences.get(&user).copied().unwrap_or_default()
    }

    pub fn can_send(&self, channel: Id) -> bool {
        let Some(channel) = self.channels.get(&channel) else { return false };
        let Some(guild) = channel.guild_id.and_then(|g| self.guild(g)) else { return true };
        // Threads inherit their parent's overwrites.
        let target = channel
            .is_thread()
            .then(|| channel.parent_id.and_then(|p| self.channels.get(&p)))
            .flatten()
            .unwrap_or(channel);
        let perms = permissions::in_channel(&guild.guild, target, self.my_id(), &guild.own_roles);
        perms & permissions::SEND_MESSAGES != 0
    }

    pub fn can_manage_messages(&self, channel: Id) -> bool {
        let Some(channel) = self.channels.get(&channel) else { return false };
        let Some(guild) = channel.guild_id.and_then(|g| self.guild(g)) else { return false };
        let perms = permissions::in_channel(&guild.guild, channel, self.my_id(), &guild.own_roles);
        perms & permissions::MANAGE_MESSAGES != 0
    }

    // ---------------------------------------------------------------- unread

    pub fn is_muted(&self, channel: Id) -> bool {
        self.muted.contains(&channel)
            || self.channels.get(&channel).and_then(|c| c.guild_id).is_some_and(|g| self.muted.contains(&g))
    }

    pub fn is_unread(&self, channel: Id) -> bool {
        if self.is_muted(channel) {
            return false;
        }
        let Some(last) = self.channels.get(&channel).and_then(|c| c.last_message_id) else { return false };
        match self.read.get(&channel) {
            Some(marker) => marker.last_read.is_none_or(|read| read < last),
            // Bots have no read state; treat everything as read at startup.
            None => false,
        }
    }

    pub fn mentions(&self, channel: Id) -> u32 {
        self.read.get(&channel).map_or(0, |m| m.mentions)
    }

    pub fn guild_unread(&self, guild: Id) -> (bool, u32) {
        let Some(guild) = self.guild(guild) else { return (false, 0) };
        let mut unread = false;
        let mut mentions = 0;
        for row in &guild.rows {
            if row.kind == RowKind::Category {
                continue;
            }
            unread |= self.is_unread(row.id);
            mentions += self.mentions(row.id);
        }
        (unread, mentions)
    }

    pub fn dm_unread(&self) -> u32 {
        self.dms.iter().filter(|id| self.is_unread(**id)).count() as u32
    }

    pub fn mark_guild_read(&mut self, guild: Id, cx: &mut Context<Self>) {
        let channels: Vec<Id> = self
            .guild(guild)
            .map(|g| g.rows.iter().filter(|r| r.kind != RowKind::Category).map(|r| r.id).collect())
            .unwrap_or_default();
        for channel in channels {
            self.mark_read(channel, cx);
        }
    }

    pub fn set_status(&mut self, status: Status, cx: &mut Context<Self>) {
        self.my_status = status;
        let me = self.my_id();
        self.presences.insert(me, status);
        if let Some(commands) = &self.commands {
            let _ = commands.send(Command::SetStatus(status));
        }
        cx.emit(StoreEvent::Channels);
    }

    /// Members of a channel's guild (or DM recipients) whose name matches `query`.
    pub fn mention_candidates(&self, channel: Id, query: &str, limit: usize) -> Vec<(User, String)> {
        let Some(info) = self.channels.get(&channel) else { return Vec::new() };
        let query = query.to_lowercase();
        let mut out: Vec<(u8, User, String)> = Vec::new();
        let mut consider = |user: &User, name: String| {
            let (a, b) = (name.to_lowercase(), user.username.to_lowercase());
            let rank = if a.starts_with(&query) || b.starts_with(&query) {
                0
            } else if a.contains(&query) || b.contains(&query) {
                1
            } else {
                return;
            };
            out.push((rank, user.clone(), name));
        };
        match info.guild_id {
            Some(guild) => {
                for ((g, user_id), member) in &self.member_info {
                    if *g != guild {
                        continue;
                    }
                    if let Some(user) = self.users.get(user_id) {
                        consider(user, member.nick.clone().unwrap_or_else(|| user.display_name().to_string()));
                    }
                }
            }
            None => {
                for user in info.recipients.iter().chain(self.me.as_ref()) {
                    consider(user, user.display_name().to_string());
                }
            }
        }
        out.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.2.to_lowercase().cmp(&b.2.to_lowercase())));
        out.truncate(limit);
        out.into_iter().map(|(_, user, name)| (user, name)).collect()
    }

    /// Text channels of the same guild whose name matches `query`.
    pub fn channel_candidates(&self, channel: Id, query: &str, limit: usize) -> Vec<(Id, String)> {
        let Some(guild) = self.channels.get(&channel).and_then(|c| c.guild_id).and_then(|g| self.guild(g)) else {
            return Vec::new();
        };
        let query = query.to_lowercase();
        guild
            .rows
            .iter()
            .filter(|r| r.kind != RowKind::Category)
            .filter_map(|r| {
                let name = self.channels.get(&r.id)?.name.clone()?;
                name.to_lowercase().contains(&query).then_some((r.id, name))
            })
            .take(limit)
            .collect()
    }

    pub fn mark_read(&mut self, channel: Id, cx: &mut Context<Self>) {
        let Some(last) = self.channels.get(&channel).and_then(|c| c.last_message_id) else { return };
        let marker = self.read.entry(channel).or_default();
        if marker.last_read.is_some_and(|read| read >= last) && marker.mentions == 0 {
            return;
        }
        marker.last_read = Some(last);
        marker.mentions = 0;
        cx.emit(StoreEvent::Unread);
        if let Some(rest) = self.rest.clone() {
            self.run(cx, async move { rest.ack(channel, last).await }, |_, result, _| {
                if let Err(e) = result {
                    log::debug!("ack failed: {e}");
                }
            });
        }
    }

    // ---------------------------------------------------------------- selection

    pub fn open_friends(&mut self, cx: &mut Context<Self>) {
        self.selected_guild = None;
        self.selected_channel = None;
        self.show_friends = true;
        cx.emit(StoreEvent::Selection);
    }

    pub fn select_guild(&mut self, guild: Option<Id>, cx: &mut Context<Self>) {
        if self.selected_guild == guild && (self.selected_channel.is_some() || self.show_friends) {
            return;
        }
        self.show_friends = false;
        self.selected_guild = guild;
        let channel = self.last_channel.get(&guild).copied().filter(|c| self.channels.contains_key(c)).or_else(|| {
            match guild {
                Some(id) => self.guild(id).and_then(|g| {
                    g.rows
                        .iter()
                        .find(|r| {
                            r.kind == RowKind::Channel
                                && self.channels.get(&r.id).is_some_and(|c| {
                                    matches!(c.kind, channel_kind::TEXT | channel_kind::ANNOUNCEMENT)
                                })
                        })
                        .map(|r| r.id)
                }),
                None => self.dms.first().copied(),
            }
        });
        self.selected_channel = None;
        match channel {
            Some(channel) => self.select_channel(channel, cx),
            None => cx.emit(StoreEvent::Selection),
        }
    }

    pub fn select_channel(&mut self, channel: Id, cx: &mut Context<Self>) {
        let Some(info) = self.channels.get(&channel) else { return };
        let is_forum = info.is_forum();
        if !info.is_text_like() && !is_forum {
            return;
        }
        let guild = info.guild_id;
        self.show_friends = false;
        self.selected_guild = guild;
        self.selected_channel = Some(channel);
        self.last_channel.insert(guild, channel);
        cx.emit(StoreEvent::Selection);

        if is_forum {
            self.load_forum(channel, cx);
        } else if !self.messages.get(&channel).is_some_and(|m| m.loaded) {
            self.load_history(channel, cx);
        }
        self.mark_read(channel, cx);

        if let Some(guild) = guild {
            if self.subscribed != Some((guild, channel)) {
                self.subscribed = Some((guild, channel));
                if let Some(commands) = &self.commands {
                    let _ = commands.send(Command::SubscribeMembers { guild_id: guild, channel_id: channel });
                }
            }
        }
    }

    pub fn toggle_members(&mut self, cx: &mut Context<Self>) {
        self.show_members = !self.show_members;
        cx.emit(StoreEvent::Selection);
    }

    pub fn toggle_category(&mut self, category: Id, cx: &mut Context<Self>) {
        if !self.collapsed.remove(&category) {
            self.collapsed.insert(category);
        }
        cx.emit(StoreEvent::Channels);
    }

    /// Opens (creating if needed) a direct message with `user`.
    pub fn open_dm(&mut self, user: Id, cx: &mut Context<Self>) {
        if user == self.my_id() {
            return;
        }
        let existing = self.dms.iter().copied().find(|id| {
            self.channels
                .get(id)
                .is_some_and(|c| c.kind == channel_kind::DM && c.recipients.iter().any(|u| u.id == user))
        });
        if let Some(channel) = existing {
            self.select_channel(channel, cx);
            return;
        }
        let Some(rest) = self.rest.clone() else { return };
        self.run(cx, async move { rest.open_dm(user).await }, |store, result, cx| {
            if let Some(channel) = store.report(result, "Could not open direct message", cx) {
                let id = channel.id;
                store.upsert_channel(channel, cx);
                store.select_channel(id, cx);
            }
        });
    }

    // ---------------------------------------------------------------- friends, mutes and membership

    pub fn relationship(&self, user: Id) -> Option<u8> {
        self.relationships.iter().find(|(id, _)| *id == user).map(|(_, kind)| *kind)
    }

    fn set_relationship(&mut self, user: Id, kind: Option<u8>, cx: &mut Context<Self>) {
        self.relationships.retain(|(id, _)| *id != user);
        if let Some(kind) = kind {
            self.relationships.push((user, kind));
        }
        cx.emit(StoreEvent::Friends);
    }

    /// Runs a relationship request; in demo mode applies `local` instead.
    fn relationship_call<F>(
        &mut self,
        user: Option<Id>,
        local: Option<u8>,
        what: &'static str,
        cx: &mut Context<Self>,
        call: impl FnOnce(Rest) -> F + Send + 'static,
    ) where
        F: Future<Output = Result<()>> + Send + 'static,
    {
        if self.demo {
            if let Some(user) = user {
                self.set_relationship(user, local, cx);
            }
            return;
        }
        let Some(rest) = self.rest.clone() else { return };
        if rest.is_bot() {
            cx.emit(StoreEvent::Error("Friends are only available to user accounts.".into()));
            return;
        }
        self.run(cx, async move { call(rest).await }, move |store, result, cx| {
            store.report(result, what, cx);
        });
    }

    pub fn add_friend(&mut self, username: String, cx: &mut Context<Self>) {
        if self.demo {
            cx.emit(StoreEvent::Error("Friend requests are not sent in demo mode.".into()));
            return;
        }
        self.relationship_call(None, None, "Could not send friend request", cx, move |rest| async move {
            rest.add_friend(&username).await
        });
    }

    pub fn accept_friend(&mut self, user: Id, cx: &mut Context<Self>) {
        self.relationship_call(Some(user), Some(1), "Could not accept request", cx, move |rest| async move {
            rest.accept_friend(user).await
        });
    }

    pub fn block_user(&mut self, user: Id, cx: &mut Context<Self>) {
        self.relationship_call(Some(user), Some(2), "Could not block user", cx, move |rest| async move {
            rest.block_user(user).await
        });
    }

    pub fn remove_relationship(&mut self, user: Id, cx: &mut Context<Self>) {
        self.relationship_call(Some(user), None, "Could not update relationship", cx, move |rest| async move {
            rest.remove_relationship(user).await
        });
    }

    pub fn create_group_dm(&mut self, users: Vec<Id>, cx: &mut Context<Self>) {
        if self.demo {
            let recipients = users.iter().filter_map(|u| self.users.get(u).cloned()).collect();
            let id = Id::now();
            let channel = Channel { id, kind: channel_kind::GROUP_DM, recipients, ..Default::default() };
            self.messages.insert(id, ChannelMessages { loaded: true, reached_start: true, ..Default::default() });
            self.upsert_channel(channel, cx);
            self.select_channel(id, cx);
            return;
        }
        let Some(rest) = self.rest.clone() else { return };
        self.run(cx, async move { rest.create_group_dm(&users).await }, |store, result, cx| {
            if let Some(channel) = store.report(result, "Could not create group", cx) {
                let id = channel.id;
                store.upsert_channel(channel, cx);
                store.select_channel(id, cx);
            }
        });
    }

    pub fn close_dm(&mut self, channel: Id, cx: &mut Context<Self>) {
        if self.demo {
            self.apply(Event::ChannelDelete(channel), cx);
            return;
        }
        let Some(rest) = self.rest.clone() else { return };
        self.run(cx, async move { rest.close_dm(channel).await }, move |store, result, cx| {
            if store.report(result, "Could not close conversation", cx).is_some() {
                store.apply(Event::ChannelDelete(channel), cx);
            }
        });
    }

    pub fn leave_guild(&mut self, guild: Id, cx: &mut Context<Self>) {
        if self.demo {
            self.apply(Event::GuildDelete(guild), cx);
            return;
        }
        let Some(rest) = self.rest.clone() else { return };
        self.run(cx, async move { rest.leave_guild(guild).await }, move |store, result, cx| {
            if store.report(result, "Could not leave server", cx).is_some() {
                store.apply(Event::GuildDelete(guild), cx);
            }
        });
    }

    pub fn copy_invite(&mut self, channel: Id, cx: &mut Context<Self>) {
        let copied = |code: &str| StoreEvent::Clipboard {
            text: format!("https://discord.gg/{code}"),
            notice: "Invite link copied. It expires in one day.".into(),
        };
        if self.demo {
            cx.emit(copied("demo-invite"));
            return;
        }
        let Some(rest) = self.rest.clone() else { return };
        self.run(cx, async move { rest.create_invite(channel).await }, move |store, result, cx| {
            if let Some(code) = store.report(result, "Could not create invite", cx) {
                cx.emit(copied(&code));
            }
        });
    }

    /// Mutes or unmutes a server (pass its id) or a single channel.
    pub fn toggle_mute(&mut self, target: Id, cx: &mut Context<Self>) {
        let muted = !self.muted.contains(&target);
        if muted {
            self.muted.insert(target);
        } else {
            self.muted.remove(&target);
        }
        let all: Vec<u64> = self.muted.iter().map(|id| id.0).collect();
        crate::settings::Settings::update(|s| s.muted = all);
        cx.emit(StoreEvent::Unread);
        cx.emit(StoreEvent::Channels);

        // Keep Discord's own setting in step for user accounts.
        let Some(rest) = self.rest.clone().filter(|r| !r.is_bot() && !self.demo) else { return };
        let (guild, channel) = match self.channels.get(&target) {
            Some(info) => (info.guild_id, Some(target)),
            None => (Some(target), None),
        };
        self.run(cx, async move { rest.set_muted(guild, channel, muted).await }, |_, result, _| {
            if let Err(e) = result {
                log::warn!("could not sync mute setting: {e}");
            }
        });
    }

    // ---------------------------------------------------------------- administration

    /// Our permissions across a whole server.
    pub fn guild_permissions(&self, guild: Id) -> u64 {
        let Some(state) = self.guild(guild) else { return 0 };
        permissions::base(&state.guild, self.my_id(), &state.own_roles)
    }

    pub fn can(&self, guild: Id, permission: u64) -> bool {
        self.guild_permissions(guild) & permission == permission
    }

    /// The position of our highest role; roles at or above it cannot be managed.
    pub fn top_role_position(&self, guild: Id) -> i32 {
        let Some(state) = self.guild(guild) else { return 0 };
        if state.guild.owner_id == Some(self.my_id()) {
            return i32::MAX;
        }
        state.guild.roles.iter().filter(|r| state.own_roles.contains(&r.id)).map(|r| r.position).max().unwrap_or(0)
    }

    /// Runs an administrative request and applies its result locally, so the
    /// change shows immediately (the gateway may confirm it again later).
    fn admin<T: Send + 'static, F>(
        &mut self,
        what: &'static str,
        cx: &mut Context<Self>,
        call: impl FnOnce(Rest) -> F + Send + 'static,
        apply: impl FnOnce(&mut Self, T, &mut Context<Self>) + 'static,
    ) where
        F: Future<Output = Result<T>> + Send + 'static,
    {
        let Some(rest) = self.rest.clone() else { return };
        self.run(cx, async move { call(rest).await }, move |store, result, cx| {
            if let Some(value) = store.report(result, what, cx) {
                apply(store, value, cx);
            }
        });
    }

    // ---------------------------------------------------------------- account

    /// Whether this is a bot account (several profile fields are user-only).
    pub fn is_bot(&self) -> bool {
        self.rest.as_ref().is_some_and(Rest::is_bot)
    }

    fn set_me(&mut self, user: User, cx: &mut Context<Self>) {
        self.users.insert(user.id, user.clone());
        self.me = Some(user);
        cx.emit(StoreEvent::Channels);
        cx.notify();
    }

    /// Fetches the about-me text and pronouns, if not already known.
    pub fn load_my_profile(&mut self, cx: &mut Context<Self>) {
        if self.my_profile.is_some() {
            return;
        }
        if self.demo || self.is_bot() {
            self.my_profile = Some(Profile::default());
            return;
        }
        let (Some(rest), Some(me)) = (self.rest.clone(), self.me.as_ref().map(|u| u.id)) else { return };
        self.run(cx, async move { rest.my_profile(me).await }, |store, result, cx| {
            let Ok(value) = result else { return };
            let text = |v: &serde_json::Value| v.as_str().unwrap_or_default().to_string();
            let p = &value["user_profile"];
            store.my_profile = Some(Profile { bio: text(&p["bio"]), pronouns: text(&p["pronouns"]) });
            cx.notify();
        });
    }

    /// Saves the display name, and for user accounts the about-me and pronouns.
    /// Bot accounts have no display name or profile text, so only users get here.
    pub fn edit_profile(&mut self, display_name: String, profile: Profile, cx: &mut Context<Self>) {
        let global_name = (!display_name.is_empty()).then_some(display_name);
        if self.demo {
            if let Some(mut me) = self.me.clone() {
                me.global_name = global_name;
                self.set_me(me, cx);
            }
            self.my_profile = Some(profile);
            cx.emit(StoreEvent::Notice("Profile saved.".into()));
            return;
        }
        let changed_name = self.me.as_ref().map(|m| m.global_name.clone()) != Some(global_name.clone());
        let changed_profile = self.my_profile.as_ref() != Some(&profile);
        let saved = profile.clone();
        self.admin(
            "Could not save your profile",
            cx,
            move |rest| async move {
                let user = if changed_name {
                    Some(rest.modify_me(serde_json::json!({ "global_name": global_name })).await?)
                } else {
                    None
                };
                if changed_profile {
                    rest.modify_profile(serde_json::json!({ "bio": profile.bio, "pronouns": profile.pronouns })).await?;
                }
                Ok(user)
            },
            move |store, user, cx| {
                if let Some(user) = user {
                    store.set_me(user, cx);
                }
                store.my_profile = Some(saved);
                cx.emit(StoreEvent::Notice("Profile saved.".into()));
            },
        );
    }

    /// Changes the username. User accounts must confirm with their password.
    pub fn change_username(&mut self, username: String, password: String, cx: &mut Context<Self>) {
        if self.demo {
            if let Some(mut me) = self.me.clone() {
                me.username = username;
                self.set_me(me, cx);
            }
            return;
        }
        let body = if self.is_bot() {
            serde_json::json!({ "username": username })
        } else {
            serde_json::json!({ "username": username, "password": password })
        };
        self.admin("Could not change your username", cx, move |rest| async move { rest.modify_me(body).await }, |store, user, cx| {
            store.set_me(user, cx);
            cx.emit(StoreEvent::Notice("Username changed.".into()));
        });
    }

    /// Sets the avatar from image bytes, or removes it with `None`.
    pub fn set_avatar(&mut self, image: Option<(String, Vec<u8>)>, cx: &mut Context<Self>) {
        if self.demo {
            cx.emit(StoreEvent::Notice("Avatars are not uploaded in demo mode.".into()));
            return;
        }
        let avatar = image.map(|(mime, bytes)| data_uri(&mime, &bytes));
        let body = serde_json::json!({ "avatar": avatar });
        self.admin("Could not change your avatar", cx, move |rest| async move { rest.modify_me(body).await }, |store, user, cx| {
            store.set_me(user, cx);
            cx.emit(StoreEvent::Notice("Avatar updated.".into()));
        });
    }

    pub fn rename_guild(&mut self, guild: Id, name: String, cx: &mut Context<Self>) {
        let new_name = name.clone();
        let apply = move |store: &mut Self, _: (), cx: &mut Context<Self>| {
            if let Some(state) = store.guild_mut(guild) {
                state.guild.name = new_name;
            }
            cx.emit(StoreEvent::Guilds);
            cx.emit(StoreEvent::Channels);
        };
        if self.demo {
            return apply(self, (), cx);
        }
        let body = serde_json::json!({ "name": name });
        self.admin("Could not rename the server", cx, move |rest| async move { rest.modify_guild(guild, body).await }, apply);
    }

    /// Sets the server icon from image bytes, or removes it with `None`.
    pub fn set_guild_icon(&mut self, guild: Id, image: Option<(String, Vec<u8>)>, cx: &mut Context<Self>) {
        if self.demo {
            cx.emit(StoreEvent::Notice("Icons are not uploaded in demo mode.".into()));
            return;
        }
        let icon = image.map(|(mime, bytes)| data_uri(&mime, &bytes));
        let body = serde_json::json!({ "icon": icon });
        self.admin("Could not change the icon", cx, move |rest| async move { rest.modify_guild(guild, body).await }, |_, _, cx| {
            cx.emit(StoreEvent::Notice("Server icon updated.".into()));
        });
    }

    pub fn create_channel(&mut self, guild: Id, name: String, kind: u8, parent: Option<Id>, cx: &mut Context<Self>) {
        let apply = move |store: &mut Self, mut channel: Channel, cx: &mut Context<Self>| {
            channel.guild_id = Some(guild);
            let id = channel.id;
            let selectable = channel.is_text_like() || channel.is_forum();
            store.upsert_channel(channel, cx);
            if selectable {
                store.messages.entry(id).or_insert_with(|| ChannelMessages {
                    loaded: store.demo,
                    reached_start: store.demo,
                    ..Default::default()
                });
                store.select_channel(id, cx);
            }
        };
        if self.demo {
            let position = self.channels.values().filter(|c| c.guild_id == Some(guild)).map(|c| c.position).max();
            let channel = Channel {
                id: Id::now(),
                kind,
                guild_id: Some(guild),
                name: Some(name),
                parent_id: parent,
                position: position.unwrap_or(0) + 1,
                ..Default::default()
            };
            return apply(self, channel, cx);
        }
        let mut body = serde_json::json!({ "name": name, "type": kind });
        if let Some(parent) = parent {
            body["parent_id"] = parent.to_string().into();
        }
        self.admin("Could not create the channel", cx, move |rest| async move { rest.create_channel(guild, body).await }, apply);
    }

    pub fn edit_channel(&mut self, channel: Id, name: String, topic: Option<String>, cx: &mut Context<Self>) {
        let apply = |store: &mut Self, updated: Channel, cx: &mut Context<Self>| store.upsert_channel(updated, cx);
        if self.demo {
            let Some(mut updated) = self.channels.get(&channel).cloned() else { return };
            updated.name = Some(name);
            if topic.is_some() {
                updated.topic = topic;
            }
            return apply(self, updated, cx);
        }
        let mut body = serde_json::json!({ "name": name });
        if let Some(topic) = topic {
            body["topic"] = topic.into();
        }
        self.admin("Could not save the channel", cx, move |rest| async move { rest.modify_channel(channel, body).await }, apply);
    }

    pub fn delete_channel(&mut self, channel: Id, cx: &mut Context<Self>) {
        let apply = move |store: &mut Self, _: (), cx: &mut Context<Self>| store.apply(Event::ChannelDelete(channel), cx);
        if self.demo {
            return apply(self, (), cx);
        }
        self.admin("Could not delete the channel", cx, move |rest| async move { rest.delete_channel(channel).await }, apply);
    }

    fn upsert_role(&mut self, guild: Id, role: Role, cx: &mut Context<Self>) {
        if let Some(state) = self.guild_mut(guild) {
            match state.guild.roles.iter_mut().find(|r| r.id == role.id) {
                Some(existing) => *existing = role,
                None => state.guild.roles.push(role),
            }
        }
        self.recolor_members(guild);
        self.rebuild_rows(guild);
        cx.emit(StoreEvent::Guilds);
        cx.emit(StoreEvent::Channels);
        cx.emit(StoreEvent::Members(guild));
    }

    /// Recomputes everyone's name colour after a role change.
    fn recolor_members(&mut self, guild: Id) {
        let Some(state) = self.guild(guild) else { return };
        let roles = state.guild.roles.clone();
        let color_of = |member_roles: &[Id]| {
            roles
                .iter()
                .filter(|r| r.color != 0 && member_roles.contains(&r.id))
                .max_by_key(|r| r.position)
                .map(|r| r.color)
        };
        for ((g, _), info) in self.member_info.iter_mut() {
            if *g == guild {
                info.color = color_of(&info.roles);
            }
        }
        let infos = &self.member_info;
        if let Some(rows) = self.member_lists.get_mut(&guild) {
            for row in rows.iter_mut() {
                if let MemberRow::Member { user, color, nick, .. } = row {
                    if let Some(info) = infos.get(&(guild, user.id)) {
                        *color = info.color;
                        *nick = info.nick.clone();
                    }
                }
            }
        }
    }

    /// Creates a role and hands its id to `then` (to open it for editing).
    pub fn create_role(&mut self, guild: Id, cx: &mut Context<Self>, then: impl FnOnce(Id, &mut gpui::App) + 'static) {
        let apply = move |store: &mut Self, role: Role, cx: &mut Context<Self>| {
            let id = role.id;
            store.upsert_role(guild, role, cx);
            cx.defer(move |cx| then(id, cx));
        };
        if self.demo {
            let role = Role { id: Id::now(), name: "new role".into(), position: 1, ..Default::default() };
            return apply(self, role, cx);
        }
        let body = serde_json::json!({ "name": "new role" });
        self.admin("Could not create the role", cx, move |rest| async move { rest.create_role(guild, body).await }, apply);
    }

    pub fn save_role(&mut self, guild: Id, role: Id, name: String, color: u32, permissions: u64, hoist: bool, cx: &mut Context<Self>) {
        let apply = move |store: &mut Self, updated: Role, cx: &mut Context<Self>| {
            store.upsert_role(guild, updated, cx);
            cx.emit(StoreEvent::Notice("Role saved.".into()));
        };
        if self.demo {
            let Some(mut updated) = self.guild(guild).and_then(|g| g.guild.roles.iter().find(|r| r.id == role)).cloned()
            else {
                return;
            };
            updated.name = name;
            updated.color = color;
            updated.permissions = permissions;
            updated.hoist = hoist;
            return apply(self, updated, cx);
        }
        let body = serde_json::json!({
            "name": name, "color": color, "permissions": permissions.to_string(), "hoist": hoist,
        });
        self.admin("Could not save the role", cx, move |rest| async move { rest.modify_role(guild, role, body).await }, apply);
    }

    /// Roles from highest to lowest, without @everyone, in Discord's order
    /// (ties on position go to the older role).
    pub fn ordered_roles(&self, guild: Id) -> Vec<Role> {
        let Some(state) = self.guild(guild) else { return Vec::new() };
        let mut roles: Vec<Role> = state.guild.roles.iter().filter(|r| r.id != guild).cloned().collect();
        roles.sort_by(|a, b| b.position.cmp(&a.position).then(a.id.cmp(&b.id)));
        roles
    }

    /// Swaps a role with the one above (`up`) or below it.
    pub fn move_role(&mut self, guild: Id, role: Id, up: bool, cx: &mut Context<Self>) {
        let top = self.top_role_position(guild);
        let mut order = self.ordered_roles(guild);
        let Some(ix) = order.iter().position(|r| r.id == role) else { return };
        let other = if up { ix.checked_sub(1) } else { Some(ix + 1).filter(|&o| o < order.len()) };
        let Some(other) = other else { return };
        if order[ix].position >= top || order[other].position >= top {
            cx.emit(StoreEvent::Error("You can only move roles below your highest role.".into()));
            return;
        }
        order.swap(ix, other);
        // Renumber from the bottom (@everyone is 0), sending only what moved.
        let count = order.len() as i32;
        let moved: Vec<Role> = order
            .into_iter()
            .enumerate()
            .filter_map(|(i, mut r)| {
                let position = count - i as i32;
                (r.position != position).then(|| {
                    r.position = position;
                    r
                })
            })
            .collect();
        for r in &moved {
            self.upsert_role(guild, r.clone(), cx);
        }
        if self.demo {
            return;
        }
        let body: Vec<_> =
            moved.iter().map(|r| serde_json::json!({ "id": r.id.to_string(), "position": r.position })).collect();
        let body = serde_json::Value::Array(body);
        self.admin("Could not move the role", cx, move |rest| async move { rest.reorder_roles(guild, body).await }, move |store, roles, cx| {
            for r in roles {
                store.upsert_role(guild, r, cx);
            }
        });
    }

    fn set_overwrite_locally(&mut self, channel: Id, target: Id, overwrite: Option<Overwrite>, cx: &mut Context<Self>) {
        if let Some(c) = self.channels.get_mut(&channel) {
            c.permission_overwrites.retain(|o| o.id != target);
            c.permission_overwrites.extend(overwrite);
        }
        cx.emit(StoreEvent::Channels);
        cx.notify();
    }

    /// Creates or replaces a role's or member's permission override on a channel.
    pub fn save_overwrite(&mut self, channel: Id, overwrite: Overwrite, cx: &mut Context<Self>) {
        let target = overwrite.id;
        let saved = overwrite.clone();
        let apply = move |store: &mut Self, _: (), cx: &mut Context<Self>| {
            store.set_overwrite_locally(channel, target, Some(saved), cx);
            cx.emit(StoreEvent::Notice("Permissions saved.".into()));
        };
        if self.demo {
            return apply(self, (), cx);
        }
        let body = serde_json::json!({
            "type": overwrite.kind,
            "allow": overwrite.allow.to_string(),
            "deny": overwrite.deny.to_string(),
        });
        self.admin("Could not save the permissions", cx, move |rest| async move { rest.set_overwrite(channel, target, body).await }, apply);
    }

    pub fn delete_overwrite(&mut self, channel: Id, target: Id, cx: &mut Context<Self>) {
        let apply = move |store: &mut Self, _: (), cx: &mut Context<Self>| store.set_overwrite_locally(channel, target, None, cx);
        if self.demo {
            return apply(self, (), cx);
        }
        self.admin("Could not remove the override", cx, move |rest| async move { rest.delete_overwrite(channel, target).await }, apply);
    }

    // ------------------------------------------------------------ expressions

    fn change_expressions(&mut self, guild: Id, cx: &mut Context<Self>, change: impl FnOnce(&mut Guild)) {
        if let Some(state) = self.guild_mut(guild) {
            change(&mut state.guild);
        }
        cx.emit(StoreEvent::Guilds);
        cx.notify();
    }

    /// Adds a custom emoji from image bytes (PNG, JPEG, GIF or WebP, at most 256 KB).
    pub fn create_emoji(&mut self, guild: Id, name: String, mime: String, bytes: Vec<u8>, cx: &mut Context<Self>) {
        if bytes.len() > 256 * 1024 {
            cx.emit(StoreEvent::Error("Emoji images must be 256 KB or smaller.".into()));
            return;
        }
        let apply = move |store: &mut Self, emoji: Emoji, cx: &mut Context<Self>| {
            store.change_expressions(guild, cx, |g| g.emojis.push(emoji));
        };
        if self.demo {
            let emoji = Emoji { id: Some(Id::now()), name: Some(name), animated: mime == "image/gif" };
            return apply(self, emoji, cx);
        }
        let uri = data_uri(&mime, &bytes);
        self.admin("Could not add the emoji", cx, move |rest| async move { rest.create_emoji(guild, &name, &uri).await }, apply);
    }

    pub fn rename_emoji(&mut self, guild: Id, emoji: Id, name: String, cx: &mut Context<Self>) {
        let apply = move |store: &mut Self, updated: Emoji, cx: &mut Context<Self>| {
            store.change_expressions(guild, cx, |g| {
                if let Some(e) = g.emojis.iter_mut().find(|e| e.id == Some(emoji)) {
                    e.name = updated.name;
                }
            });
        };
        if self.demo {
            return apply(self, Emoji { id: Some(emoji), name: Some(name), animated: false }, cx);
        }
        self.admin("Could not rename the emoji", cx, move |rest| async move { rest.rename_emoji(guild, emoji, &name).await }, apply);
    }

    pub fn delete_emoji(&mut self, guild: Id, emoji: Id, cx: &mut Context<Self>) {
        let apply = move |store: &mut Self, _: (), cx: &mut Context<Self>| {
            store.change_expressions(guild, cx, |g| g.emojis.retain(|e| e.id != Some(emoji)));
        };
        if self.demo {
            return apply(self, (), cx);
        }
        self.admin("Could not delete the emoji", cx, move |rest| async move { rest.delete_emoji(guild, emoji).await }, apply);
    }

    /// Uploads a sticker (PNG, APNG or GIF, at most 512 KB). `tags` is the
    /// emoji Discord suggests it for.
    pub fn create_sticker(&mut self, guild: Id, name: String, tags: String, file: Upload, cx: &mut Context<Self>) {
        if file.bytes.len() > 512 * 1024 {
            cx.emit(StoreEvent::Error("Stickers must be 512 KB or smaller.".into()));
            return;
        }
        let apply = move |store: &mut Self, sticker: StickerItem, cx: &mut Context<Self>| {
            store.change_expressions(guild, cx, |g| g.stickers.push(sticker));
        };
        if self.demo {
            return apply(self, StickerItem { id: Id::now(), name, format_type: 1 }, cx);
        }
        self.admin("Could not add the sticker", cx, move |rest| async move { rest.create_sticker(guild, &name, &tags, file).await }, apply);
    }

    pub fn rename_sticker(&mut self, guild: Id, sticker: Id, name: String, cx: &mut Context<Self>) {
        let apply = move |store: &mut Self, updated: StickerItem, cx: &mut Context<Self>| {
            store.change_expressions(guild, cx, |g| {
                if let Some(s) = g.stickers.iter_mut().find(|s| s.id == sticker) {
                    s.name = updated.name;
                }
            });
        };
        if self.demo {
            return apply(self, StickerItem { id: sticker, name, format_type: 1 }, cx);
        }
        self.admin("Could not rename the sticker", cx, move |rest| async move { rest.rename_sticker(guild, sticker, &name).await }, apply);
    }

    pub fn delete_sticker(&mut self, guild: Id, sticker: Id, cx: &mut Context<Self>) {
        let apply = move |store: &mut Self, _: (), cx: &mut Context<Self>| {
            store.change_expressions(guild, cx, |g| g.stickers.retain(|s| s.id != sticker));
        };
        if self.demo {
            return apply(self, (), cx);
        }
        self.admin("Could not delete the sticker", cx, move |rest| async move { rest.delete_sticker(guild, sticker).await }, apply);
    }

    // ----------------------------------------------------------------- events

    fn upsert_event(&mut self, guild: Id, event: ScheduledEvent, cx: &mut Context<Self>) {
        let list = self.events.entry(guild).or_default();
        match list.iter_mut().find(|e| e.id == event.id) {
            Some(existing) => {
                // Updates from the gateway carry no interested count.
                let count = existing.user_count;
                *existing = event;
                if existing.user_count.is_none() {
                    existing.user_count = count;
                }
            }
            None => list.push(event),
        }
        list.sort_by_key(|e| e.start());
        cx.emit(StoreEvent::Channels);
        cx.notify();
    }

    /// Upcoming and running events of a server, soonest first.
    pub fn upcoming_events(&self, guild: Id) -> Vec<ScheduledEvent> {
        self.events.get(&guild).map(|l| l.iter().filter(|e| e.is_upcoming()).cloned().collect()).unwrap_or_default()
    }

    /// Refreshes a server's events with their interested counts.
    pub fn refresh_events(&mut self, guild: Id, cx: &mut Context<Self>) {
        if self.demo {
            return;
        }
        let path = format!("/guilds/{guild}/scheduled-events?with_user_count=true");
        self.admin("Could not load events", cx, move |rest| async move { rest.call(HttpMethod::GET, path, None).await }, move |store, list, cx| {
            let events: Vec<ScheduledEvent> = serde_json::from_value(list).unwrap_or_default();
            store.events.insert(guild, Vec::new());
            for e in events {
                store.upsert_event(guild, e, cx);
            }
        });
    }

    pub fn create_event(&mut self, guild: Id, body: serde_json::Value, cx: &mut Context<Self>) {
        if self.demo {
            let mut body = body;
            body["id"] = serde_json::json!(Id::now().to_string());
            let Ok(mut event) = serde_json::from_value::<ScheduledEvent>(body) else { return };
            event.guild_id = Some(guild);
            event.status = 1;
            event.user_count = Some(1);
            self.interested.insert(event.id);
            return self.upsert_event(guild, event, cx);
        }
        let path = format!("/guilds/{guild}/scheduled-events");
        self.admin("Could not create the event", cx, move |rest| async move { rest.call(HttpMethod::POST, path, Some(body)).await }, move |store, value, cx| {
            if let Ok(event) = serde_json::from_value::<ScheduledEvent>(value) {
                store.upsert_event(guild, event, cx);
            }
        });
    }

    /// Starts (2), ends (3) or cancels (4) an event.
    pub fn set_event_status(&mut self, guild: Id, event: Id, status: u8, cx: &mut Context<Self>) {
        let apply = move |store: &mut Self, cx: &mut Context<Self>| {
            if let Some(e) = store.events.get_mut(&guild).and_then(|l| l.iter_mut().find(|e| e.id == event)) {
                e.status = status;
            }
            cx.emit(StoreEvent::Channels);
            cx.notify();
        };
        if self.demo {
            return apply(self, cx);
        }
        let path = format!("/guilds/{guild}/scheduled-events/{event}");
        let body = serde_json::json!({ "status": status });
        self.admin("Could not change the event", cx, move |rest| async move { rest.call(HttpMethod::PATCH, path, Some(body)).await }, move |store, _, cx| apply(store, cx));
    }

    pub fn delete_event(&mut self, guild: Id, event: Id, cx: &mut Context<Self>) {
        let apply = move |store: &mut Self, cx: &mut Context<Self>| store.apply(Event::ScheduledEventDelete { guild_id: guild, id: event }, cx);
        if self.demo {
            return apply(self, cx);
        }
        let path = format!("/guilds/{guild}/scheduled-events/{event}");
        self.admin("Could not delete the event", cx, move |rest| async move { rest.call(HttpMethod::DELETE, path, None).await }, move |store, _, cx| apply(store, cx));
    }

    /// Marks or unmarks the user as interested in an event.
    pub fn toggle_interest(&mut self, guild: Id, event: Id, cx: &mut Context<Self>) {
        let on = !self.interested.contains(&event);
        let apply = move |store: &mut Self, cx: &mut Context<Self>| {
            if on {
                store.interested.insert(event);
            } else {
                store.interested.remove(&event);
            }
            if let Some(e) = store.events.get_mut(&guild).and_then(|l| l.iter_mut().find(|e| e.id == event)) {
                let count = e.user_count.unwrap_or(0);
                e.user_count = Some(if on { count + 1 } else { count.saturating_sub(1) });
            }
            cx.emit(StoreEvent::Channels);
            cx.notify();
        };
        if self.demo {
            return apply(self, cx);
        }
        if self.is_bot() {
            cx.emit(StoreEvent::Error("Bot accounts cannot mark interest in events.".into()));
            return;
        }
        let path = format!("/guilds/{guild}/scheduled-events/{event}/users/@me");
        let method = if on { HttpMethod::PUT } else { HttpMethod::DELETE };
        self.admin("Could not update your interest", cx, move |rest| async move { rest.call(method, path, None).await }, move |store, _, cx| apply(store, cx));
    }

    // ---------------------------------------------------------------- AutoMod

    pub fn fetch_automod(&mut self, guild: Id, cx: &mut Context<Self>, then: impl FnOnce(Vec<Rule>, &mut gpui::App) + 'static) {
        if self.demo {
            let sample = automod::parse_list(&serde_json::json!([
                { "id": "7001", "name": "No leaked builds", "trigger_type": 1, "enabled": true,
                  "trigger_metadata": { "keyword_filter": ["leaked build", "*.torrent"], "allow_list": [] },
                  "actions": [{ "type": 1 }] },
                { "id": "7002", "name": "Block mention spam", "trigger_type": 5, "enabled": true,
                  "trigger_metadata": { "mention_total_limit": 10 }, "actions": [{ "type": 1 }] },
            ]));
            cx.defer(move |cx| then(sample, cx));
            return;
        }
        let path = format!("/guilds/{guild}/auto-moderation/rules");
        self.admin("Could not load AutoMod rules", cx, move |rest| async move { rest.call(HttpMethod::GET, path, None).await }, move |_, list, cx| {
            let rules = automod::parse_list(&list);
            cx.defer(move |cx| then(rules, cx));
        });
    }

    /// Creates a rule (`rule` is `None`) or changes one, handing back the result.
    pub fn save_automod_rule(
        &mut self,
        guild: Id,
        rule: Option<Id>,
        body: serde_json::Value,
        cx: &mut Context<Self>,
        then: impl FnOnce(Rule, &mut gpui::App) + 'static,
    ) {
        if self.demo {
            // A partial change (such as switching a rule off) is already shown by the caller.
            if body.get("name").is_none() {
                return;
            }
            let mut value = body;
            value["id"] = serde_json::json!(rule.unwrap_or_else(Id::now).to_string());
            value["trigger_type"] = value.get("trigger_type").cloned().unwrap_or(serde_json::json!(1));
            value["enabled"] = value.get("enabled").cloned().unwrap_or(serde_json::json!(true));
            if let Some(rule) = automod::parse(&value) {
                cx.defer(move |cx| then(rule, cx));
            }
            return;
        }
        let (method, path) = match rule {
            Some(id) => (HttpMethod::PATCH, format!("/guilds/{guild}/auto-moderation/rules/{id}")),
            None => (HttpMethod::POST, format!("/guilds/{guild}/auto-moderation/rules")),
        };
        self.admin("Could not save the AutoMod rule", cx, move |rest| async move { rest.call(method, path, Some(body)).await }, move |_, value, cx| {
            if let Some(rule) = automod::parse(&value) {
                cx.defer(move |cx| then(rule, cx));
            }
        });
    }

    pub fn delete_automod_rule(&mut self, guild: Id, rule: Id, cx: &mut Context<Self>, then: impl FnOnce(&mut gpui::App) + 'static) {
        if self.demo {
            cx.defer(then);
            return;
        }
        let path = format!("/guilds/{guild}/auto-moderation/rules/{rule}");
        self.admin("Could not delete the rule", cx, move |rest| async move { rest.call(HttpMethod::DELETE, path, None).await }, move |_, _, cx| {
            cx.defer(then);
        });
    }

    // ------------------------------------------------------------- onboarding

    pub fn fetch_onboarding(&mut self, guild: Id, cx: &mut Context<Self>, then: impl FnOnce(serde_json::Value, &mut gpui::App) + 'static) {
        if self.demo {
            let sample = serde_json::json!({ "guild_id": guild.to_string(), "enabled": false, "mode": 0,
                "default_channel_ids": [], "prompts": [] });
            cx.defer(move |cx| then(sample, cx));
            return;
        }
        let path = format!("/guilds/{guild}/onboarding");
        self.admin("Could not load onboarding", cx, move |rest| async move { rest.call(HttpMethod::GET, path, None).await }, move |_, value, cx| {
            cx.defer(move |cx| then(value, cx));
        });
    }

    pub fn save_onboarding(&mut self, guild: Id, body: serde_json::Value, cx: &mut Context<Self>, then: impl FnOnce(serde_json::Value, &mut gpui::App) + 'static) {
        if self.demo {
            cx.emit(StoreEvent::Notice("Onboarding saved.".into()));
            cx.defer(move |cx| then(body, cx));
            return;
        }
        let path = format!("/guilds/{guild}/onboarding");
        self.admin("Could not save onboarding", cx, move |rest| async move { rest.call(HttpMethod::PUT, path, Some(body)).await }, move |_, value, cx| {
            cx.emit(StoreEvent::Notice("Onboarding saved.".into()));
            cx.defer(move |cx| then(value, cx));
        });
    }

    // --------------------------------------------------------------- webhooks

    pub fn fetch_webhooks(&mut self, guild: Id, cx: &mut Context<Self>, then: impl FnOnce(Vec<Webhook>, &mut gpui::App) + 'static) {
        if self.demo {
            let channel = self.channels.values().filter(|c| c.guild_id == Some(guild) && c.kind == 0).map(|c| c.id).min();
            let sample = vec![Webhook {
                id: Id(4242),
                name: Some("CI notifications".into()),
                channel_id: channel,
                token: Some("demo-token".into()),
                avatar: None,
            }];
            cx.defer(move |cx| then(sample, cx));
            return;
        }
        self.admin("Could not load webhooks", cx, move |rest| async move { rest.webhooks(guild).await }, move |_, hooks, cx| {
            cx.defer(move |cx| then(hooks, cx));
        });
    }

    pub fn create_webhook(&mut self, channel: Id, name: String, cx: &mut Context<Self>, then: impl FnOnce(Webhook, &mut gpui::App) + 'static) {
        if self.demo {
            let hook = Webhook { id: Id::now(), name: Some(name), channel_id: Some(channel), token: Some("demo-token".into()), avatar: None };
            cx.defer(move |cx| then(hook, cx));
            return;
        }
        self.admin("Could not create the webhook", cx, move |rest| async move { rest.create_webhook(channel, &name).await }, move |_, hook, cx| {
            cx.defer(move |cx| then(hook, cx));
        });
    }

    /// Renames a webhook and/or moves it to another channel.
    pub fn edit_webhook(
        &mut self,
        webhook: Id,
        name: String,
        channel: Id,
        cx: &mut Context<Self>,
        then: impl FnOnce(Webhook, &mut gpui::App) + 'static,
    ) {
        if self.demo {
            let hook = Webhook { id: webhook, name: Some(name), channel_id: Some(channel), token: Some("demo-token".into()), avatar: None };
            cx.defer(move |cx| then(hook, cx));
            return;
        }
        let body = serde_json::json!({ "name": name, "channel_id": channel.to_string() });
        self.admin("Could not change the webhook", cx, move |rest| async move { rest.modify_webhook(webhook, body).await }, move |_, hook, cx| {
            cx.defer(move |cx| then(hook, cx));
        });
    }

    pub fn delete_webhook(&mut self, webhook: Id, cx: &mut Context<Self>, then: impl FnOnce(&mut gpui::App) + 'static) {
        if self.demo {
            cx.defer(then);
            return;
        }
        self.admin("Could not delete the webhook", cx, move |rest| async move { rest.delete_webhook(webhook).await }, move |_, _, cx| {
            cx.defer(then);
        });
    }

    /// Loads the most recent audit log entries.
    pub fn fetch_audit_log(&mut self, guild: Id, cx: &mut Context<Self>, then: impl FnOnce(AuditLog, &mut gpui::App) + 'static) {
        let finish = move |log: serde_json::Value, cx: &mut Context<Self>| {
            let log = AuditLog {
                entries: discord::audit::parse(&log),
                users: discord::audit::users(&log).into_iter().map(|u| (u.id, u)).collect(),
            };
            cx.defer(move |cx| then(log, cx));
        };
        if self.demo {
            let me = self.me.as_ref().map(|u| serde_json::json!({ "id": u.id.to_string(), "username": u.username }));
            let first_channel = self.channels.values().find(|c| c.guild_id == Some(guild)).map(|c| c.id.to_string());
            let sample = serde_json::json!({
                "users": [me, { "id": "77", "username": "spammer" }],
                "audit_log_entries": [
                    { "id": Id::now().to_string(), "user_id": "1", "target_id": "77", "action_type": 22, "reason": "Spam" },
                    { "id": Id(Id::now().0 - (3_600_000 << 22)).to_string(), "user_id": "1", "target_id": first_channel,
                      "action_type": 11, "changes": [{ "key": "topic" }] },
                ],
            });
            return finish(sample, cx);
        }
        self.admin("Could not load the audit log", cx, move |rest| async move { rest.audit_log(guild).await }, move |_, log, cx| finish(log, cx));
    }

    pub fn delete_role(&mut self, guild: Id, role: Id, cx: &mut Context<Self>) {
        let apply = move |store: &mut Self, _: (), cx: &mut Context<Self>| store.apply(Event::RoleDelete { guild_id: guild, role_id: role }, cx);
        if self.demo {
            return apply(self, (), cx);
        }
        self.admin("Could not delete the role", cx, move |rest| async move { rest.delete_role(guild, role).await }, apply);
    }

    pub fn member_roles(&self, guild: Id, user: Id) -> Vec<Id> {
        if user == self.my_id() {
            return self.guild(guild).map(|g| g.own_roles.clone()).unwrap_or_default();
        }
        self.member_info.get(&(guild, user)).map(|m| m.roles.clone()).unwrap_or_default()
    }

    fn update_member_locally(&mut self, guild: Id, user: Id, cx: &mut Context<Self>, change: impl FnOnce(&mut MemberInfo)) {
        let info = self.member_info.entry((guild, user)).or_default();
        change(info);
        let roles = info.roles.clone();
        if user == self.my_id() {
            if let Some(state) = self.guild_mut(guild) {
                state.own_roles = roles;
            }
            self.rebuild_rows(guild);
        }
        self.recolor_members(guild);
        cx.emit(StoreEvent::Members(guild));
        cx.emit(StoreEvent::Channels);
    }

    pub fn set_member_role(&mut self, guild: Id, user: Id, role: Id, on: bool, cx: &mut Context<Self>) {
        let apply = move |store: &mut Self, _: (), cx: &mut Context<Self>| {
            let current = store.member_roles(guild, user);
            store.update_member_locally(guild, user, cx, |info| {
                info.roles = current;
                info.roles.retain(|r| *r != role);
                if on {
                    info.roles.push(role);
                }
            });
        };
        if self.demo {
            return apply(self, (), cx);
        }
        self.admin("Could not change roles", cx, move |rest| async move { rest.set_member_role(guild, user, role, on).await }, apply);
    }

    pub fn set_nickname(&mut self, guild: Id, user: Id, nick: Option<String>, cx: &mut Context<Self>) {
        let me = self.my_id();
        let nick_for_apply = nick.clone();
        let apply = move |store: &mut Self, _: (), cx: &mut Context<Self>| {
            store.update_member_locally(guild, user, cx, |info| info.nick = nick_for_apply);
        };
        if self.demo {
            return apply(self, (), cx);
        }
        let target = (user != me).then_some(user);
        self.admin("Could not change the nickname", cx, move |rest| async move {
            rest.set_nickname(guild, target, nick.as_deref()).await
        }, apply);
    }

    /// Times a member out for `minutes`, or lifts a timeout with `None`.
    pub fn timeout_member(&mut self, guild: Id, user: Id, minutes: Option<i64>, cx: &mut Context<Self>) {
        let until = minutes.map(|m| (chrono::Utc::now() + chrono::Duration::minutes(m)).to_rfc3339());
        let notice = match minutes {
            Some(m) if m >= 1440 => format!("Timed out for {} day(s).", m / 1440),
            Some(m) => format!("Timed out for {} hour(s).", (m / 60).max(1)),
            None => "Timeout removed.".to_string(),
        };
        if self.demo {
            cx.emit(StoreEvent::Notice(notice));
            return;
        }
        let body = serde_json::json!({ "communication_disabled_until": until });
        self.admin("Could not time out the member", cx, move |rest| async move { rest.modify_member(guild, user, body).await }, move |_, _, cx| {
            cx.emit(StoreEvent::Notice(notice));
        });
    }

    pub fn kick_member(&mut self, guild: Id, user: Id, cx: &mut Context<Self>) {
        let apply = move |store: &mut Self, _: (), cx: &mut Context<Self>| {
            store.apply(Event::MemberRemove { guild_id: guild, user_id: user }, cx);
            cx.emit(StoreEvent::Notice("Member kicked.".into()));
        };
        if self.demo {
            return apply(self, (), cx);
        }
        self.admin("Could not kick the member", cx, move |rest| async move { rest.kick(guild, user).await }, apply);
    }

    pub fn ban_member(&mut self, guild: Id, user: Id, delete_message_seconds: u32, cx: &mut Context<Self>) {
        let apply = move |store: &mut Self, _: (), cx: &mut Context<Self>| {
            store.apply(Event::MemberRemove { guild_id: guild, user_id: user }, cx);
            cx.emit(StoreEvent::Notice("Member banned.".into()));
        };
        if self.demo {
            return apply(self, (), cx);
        }
        self.admin("Could not ban the member", cx, move |rest| async move {
            rest.ban(guild, user, delete_message_seconds).await
        }, apply);
    }

    pub fn fetch_bans(&mut self, guild: Id, cx: &mut Context<Self>, then: impl FnOnce(Vec<Ban>, &mut gpui::App) + 'static) {
        if self.demo {
            let sample = vec![Ban { user: User { id: Id(77), username: "spammer".into(), ..Default::default() }, reason: Some("Spam".into()) }];
            cx.defer(move |cx| then(sample, cx));
            return;
        }
        self.admin("Could not load bans", cx, move |rest| async move { rest.bans(guild).await }, move |_, bans, cx| {
            cx.defer(move |cx| then(bans, cx));
        });
    }

    pub fn unban(&mut self, guild: Id, user: Id, cx: &mut Context<Self>, then: impl FnOnce(&mut gpui::App) + 'static) {
        if self.demo {
            cx.defer(then);
            return;
        }
        self.admin("Could not unban", cx, move |rest| async move { rest.unban(guild, user).await }, move |_, _, cx| {
            cx.defer(then);
        });
    }

    // ---------------------------------------------------------------- voice

    pub fn voice_members(&self, channel: Id) -> &[VoiceMember] {
        self.voice.get(&channel).map_or(&[], Vec::as_slice)
    }

    pub fn voice_member(&self, user: Id) -> Option<&VoiceMember> {
        self.voice.values().flatten().find(|m| m.user == user)
    }

    fn channel_permissions(&self, channel: Id) -> u64 {
        let Some(info) = self.channels.get(&channel) else { return 0 };
        let Some(guild) = info.guild_id.and_then(|g| self.guild(g)) else { return permissions::ALL };
        permissions::in_channel(&guild.guild, info, self.my_id(), &guild.own_roles)
    }

    pub fn has_permission(&self, channel: Id, permission: u64) -> bool {
        self.channel_permissions(channel) & permission == permission
    }

    /// May move people on and off this stage.
    pub fn is_stage_moderator(&self, channel: Id) -> bool {
        let perms = self.channel_permissions(channel);
        perms & (permissions::MANAGE_CHANNELS | permissions::MUTE_MEMBERS | permissions::MOVE_MEMBERS) != 0
    }

    /// Records someone's voice state, moving them out of any channel they left.
    fn track_voice_state(&mut self, state: &VoiceState) {
        let user = state.user_id;
        let guild = state.guild_id;
        let channels = &self.channels;
        for (channel, members) in self.voice.iter_mut() {
            // Only clear the user from channels in the same server (or calls, for DMs).
            let same_scope = channels.get(channel).map(|c| c.guild_id) == Some(guild) || guild.is_none();
            if same_scope {
                members.retain(|m| m.user != user);
            }
        }
        self.voice.retain(|_, members| !members.is_empty());
        if let Some(channel) = state.channel_id {
            let member = VoiceMember {
                user,
                self_mute: state.self_mute,
                self_deaf: state.self_deaf,
                mute: state.mute,
                deaf: state.deaf,
                suppress: state.suppress,
                hand_raised: state.request_to_speak_timestamp.is_some(),
                video: state.self_video,
                streaming: state.self_stream,
            };
            self.voice.entry(channel).or_default().push(member);
        }
        if let (Some(guild), Some(member)) = (guild, &state.member) {
            if let Some(user) = &member.user {
                self.learn_member(guild, &user.clone(), member);
            }
        }
    }

    fn apply_voice_state(&mut self, state: VoiceState, cx: &mut Context<Self>) {
        let mut state = state;
        if state.guild_id.is_none() {
            state.guild_id = state.channel_id.and_then(|c| self.channels.get(&c)).and_then(|c| c.guild_id);
        }
        self.track_voice_state(&state);
        if state.user_id == self.my_id() {
            if let Some(session) = self.voice_session.as_mut() {
                match state.channel_id {
                    Some(channel) => {
                        // A moderator may have moved us; follow along.
                        session.channel = channel;
                        session.session_id = Some(state.session_id.clone());
                        self.try_connect_voice(cx);
                    }
                    None if session.status != VoiceStatus::Joining => {
                        self.end_voice(cx);
                        cx.emit(StoreEvent::Notice("You were disconnected from voice.".into()));
                    }
                    None => {}
                }
            }
        }
        cx.emit(StoreEvent::Voice);
    }

    fn try_connect_voice(&mut self, cx: &mut Context<Self>) {
        let me = self.my_id();
        let Some(session) = self.voice_session.as_mut() else { return };
        let (Some(session_id), Some((token, endpoint))) = (session.session_id.clone(), session.server.clone()) else {
            return;
        };
        let Some(conn) = &self.voice_conn else { return };
        conn.connect(ServerInfo {
            channel: session.channel,
            guild_or_channel: session.guild.unwrap_or(session.channel),
            user: me,
            session_id,
            token,
            endpoint,
        });
        session.status = VoiceStatus::Connecting;
        cx.emit(StoreEvent::Voice);
    }

    fn send_voice_state(&self, channel: Option<Id>, guild: Option<Id>) {
        if let Some(commands) = &self.commands {
            let _ = commands.send(Command::UpdateVoiceState {
                guild_id: guild,
                channel_id: channel,
                self_mute: self.self_mute,
                self_deaf: self.self_deaf,
            });
        }
    }

    fn start_voice_connection(&mut self, cx: &mut Context<Self>) {
        let (tx, rx) = async_channel::unbounded();
        let options = crate::settings::Settings::load().voice_options();
        // Drop the old connection first so the audio devices are free.
        self.voice_conn = None;
        self.voice_conn =
            Some(VoiceConnection::start(self.runtime.clone(), &options, self.my_id(), self.self_mute, self.self_deaf, tx));
        self.voice_epoch += 1;
        let epoch = self.voice_epoch;
        cx.spawn(async move |this, cx| {
            while let Ok(event) = rx.recv().await {
                let alive = this.update(cx, |store, cx| {
                    if store.voice_epoch != epoch {
                        return false;
                    }
                    store.apply_voice_event(event, cx);
                    true
                });
                if !matches!(alive, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    fn apply_voice_event(&mut self, event: VoiceEvent, cx: &mut Context<Self>) {
        match event {
            VoiceEvent::Connected => {
                if let Some(session) = self.voice_session.as_mut() {
                    session.status = VoiceStatus::Connected;
                }
            }
            VoiceEvent::Disconnected(reason) => {
                if let Some(session) = self.voice_session.as_mut() {
                    session.status = VoiceStatus::Failed(reason);
                }
                self.speaking.clear();
            }
            VoiceEvent::Speaking(mut users) => {
                if self.demo {
                    // Keep the sample call lively.
                    users.extend([Id(2), Id(4)]);
                }
                self.speaking = users;
            }
            VoiceEvent::DeviceProblem(problem) => cx.emit(StoreEvent::Error(problem)),
        }
        cx.emit(StoreEvent::Voice);
    }

    pub fn join_voice(&mut self, channel: Id, cx: &mut Context<Self>) {
        let Some(info) = self.channels.get(&channel) else { return };
        let guild = info.guild_id;
        let is_stage = info.kind == channel_kind::STAGE;
        if self.voice_session.as_ref().is_some_and(|s| s.channel == channel) {
            return;
        }
        if guild.is_none() && self.rest.as_ref().is_some_and(|r| r.is_bot()) {
            cx.emit(StoreEvent::Error("Bots cannot join direct-message calls.".into()));
            return;
        }
        if guild.is_some() && !self.has_permission(channel, permissions::CONNECT) {
            cx.emit(StoreEvent::Error("You do not have permission to join this channel.".into()));
            return;
        }
        self.speaking.clear();
        self.voice_session =
            Some(VoiceSession { channel, guild, status: VoiceStatus::Joining, session_id: None, server: None });
        self.start_voice_connection(cx);

        if self.demo {
            let state = VoiceState {
                user_id: self.my_id(),
                channel_id: Some(channel),
                guild_id: guild,
                self_mute: self.self_mute,
                self_deaf: self.self_deaf,
                suppress: is_stage,
                ..Default::default()
            };
            self.track_voice_state(&state);
            if let Some(session) = self.voice_session.as_mut() {
                session.status = VoiceStatus::Connected;
            }
            // Sample conversation: someone is already talking.
            self.speaking.insert(if is_stage { Id(4) } else { Id(2) });
        } else {
            self.send_voice_state(Some(channel), guild);
        }
        cx.emit(StoreEvent::Voice);
    }

    /// Starts a call in a direct message and rings the other people.
    pub fn start_call(&mut self, channel: Id, cx: &mut Context<Self>) {
        let already_running = self.calls.contains_key(&channel) || !self.voice_members(channel).is_empty();
        self.join_voice(channel, cx);
        if already_running || self.demo {
            return;
        }
        let Some(rest) = self.rest.clone() else { return };
        self.run(cx, async move { rest.ring(channel).await }, |_, result, _| {
            if let Err(e) = result {
                log::warn!("could not ring: {e}");
            }
        });
    }

    fn end_voice(&mut self, cx: &mut Context<Self>) {
        self.voice_conn = None;
        self.voice_epoch += 1;
        self.speaking.clear();
        if let Some(session) = self.voice_session.take() {
            if self.demo {
                let me = self.my_id();
                if let Some(members) = self.voice.get_mut(&session.channel) {
                    members.retain(|m| m.user != me);
                }
                self.voice.retain(|_, m| !m.is_empty());
            }
        }
        cx.emit(StoreEvent::Voice);
    }

    pub fn leave_voice(&mut self, cx: &mut Context<Self>) {
        let Some(session) = self.voice_session.as_ref() else { return };
        if !self.demo {
            self.send_voice_state(None, session.guild);
        }
        self.end_voice(cx);
    }

    /// Applies our mute and deafen flags and shows them to other people.
    fn announce_self_state(&mut self, cx: &mut Context<Self>) {
        if let Some(conn) = &self.voice_conn {
            conn.set_muted(self.self_mute);
            conn.set_deafened(self.self_deaf);
        }
        if let Some(session) = &self.voice_session {
            let (channel, guild) = (session.channel, session.guild);
            if self.demo {
                let me = self.my_id();
                let (mute, deaf) = (self.self_mute, self.self_deaf);
                if let Some(member) = self.voice.get_mut(&channel).and_then(|m| m.iter_mut().find(|m| m.user == me)) {
                    member.self_mute = mute;
                    member.self_deaf = deaf;
                }
            } else {
                self.send_voice_state(Some(channel), guild);
            }
        }
        cx.emit(StoreEvent::Voice);
    }

    pub fn toggle_self_mute(&mut self, cx: &mut Context<Self>) {
        self.self_mute = !self.self_mute;
        // Unmuting while deafened also undeafens, as in Discord.
        if !self.self_mute {
            self.self_deaf = false;
        }
        self.announce_self_state(cx);
    }

    pub fn toggle_self_deaf(&mut self, cx: &mut Context<Self>) {
        self.self_deaf = !self.self_deaf;
        self.self_mute = self.self_deaf;
        self.announce_self_state(cx);
    }

    pub fn set_input_sensitivity(&mut self, db: f32) {
        if let Some(conn) = &self.voice_conn {
            conn.set_sensitivity(db);
        }
    }

    /// Reopens the audio devices, e.g. after choosing different ones.
    pub fn restart_voice_audio(&mut self, cx: &mut Context<Self>) {
        if self.voice_session.is_none() {
            return;
        }
        self.start_voice_connection(cx);
        if !self.demo {
            self.try_connect_voice(cx);
        }
    }

    /// Runs a stage or voice moderation request.
    fn voice_call<F>(&mut self, what: &'static str, cx: &mut Context<Self>, call: impl FnOnce(Rest) -> F + Send + 'static)
    where
        F: Future<Output = Result<()>> + Send + 'static,
    {
        let Some(rest) = self.rest.clone() else { return };
        self.run(cx, async move { call(rest).await }, move |store, result, cx| {
            store.report(result, what, cx);
        });
    }

    /// Applies a voice change locally, for demo mode.
    fn demo_voice_member(&mut self, user: Id, cx: &mut Context<Self>, change: impl FnOnce(&mut VoiceMember)) {
        if let Some(member) = self.voice.values_mut().flatten().find(|m| m.user == user) {
            change(member);
        }
        cx.emit(StoreEvent::Voice);
    }

    pub fn set_hand_raised(&mut self, channel: Id, raised: bool, cx: &mut Context<Self>) {
        let Some(guild) = self.channels.get(&channel).and_then(|c| c.guild_id) else { return };
        if self.demo {
            let me = self.my_id();
            return self.demo_voice_member(me, cx, |m| m.hand_raised = raised);
        }
        let stamp = raised.then(|| chrono::Utc::now().to_rfc3339());
        let body = serde_json::json!({ "channel_id": channel.to_string(), "request_to_speak_timestamp": stamp });
        self.voice_call("Could not update your request", cx, move |rest| async move {
            rest.update_voice_state(guild, None, body).await
        });
    }

    /// Moves someone (or ourselves, with `None`) on or off the stage.
    pub fn set_on_stage(&mut self, channel: Id, user: Option<Id>, on_stage: bool, cx: &mut Context<Self>) {
        let Some(guild) = self.channels.get(&channel).and_then(|c| c.guild_id) else { return };
        if self.demo {
            let target = user.unwrap_or(self.my_id());
            return self.demo_voice_member(target, cx, |m| {
                m.suppress = !on_stage;
                m.hand_raised = false;
            });
        }
        let mut body = serde_json::json!({ "channel_id": channel.to_string(), "suppress": !on_stage });
        if on_stage && user.is_none() {
            body["request_to_speak_timestamp"] = serde_json::Value::Null;
        }
        self.voice_call("Could not change who is on stage", cx, move |rest| async move {
            rest.update_voice_state(guild, user, body).await
        });
    }

    pub fn start_stage(&mut self, channel: Id, topic: String, cx: &mut Context<Self>) {
        if self.demo {
            self.stages.insert(channel, topic);
            cx.emit(StoreEvent::Voice);
            return;
        }
        self.voice_call("Could not start the stage", cx, move |rest| async move {
            rest.start_stage(channel, &topic).await
        });
    }

    pub fn end_stage(&mut self, channel: Id, cx: &mut Context<Self>) {
        if self.demo {
            self.stages.remove(&channel);
            cx.emit(StoreEvent::Voice);
            return;
        }
        self.voice_call("Could not end the stage", cx, move |rest| async move { rest.end_stage(channel).await });
    }

    /// Server mute, server deafen or disconnect someone in voice.
    pub fn moderate_voice(&mut self, guild: Id, user: Id, change: VoiceModeration, cx: &mut Context<Self>) {
        if self.demo {
            return self.demo_voice_member(user, cx, |m| match change {
                VoiceModeration::Mute(on) => m.mute = on,
                VoiceModeration::Deafen(on) => m.deaf = on,
                VoiceModeration::Disconnect => {}
            });
        }
        let body = match change {
            VoiceModeration::Mute(on) => serde_json::json!({ "mute": on }),
            VoiceModeration::Deafen(on) => serde_json::json!({ "deaf": on }),
            VoiceModeration::Disconnect => serde_json::json!({ "channel_id": null }),
        };
        self.voice_call("Could not update that member", cx, move |rest| async move {
            rest.modify_member(guild, user, body).await
        });
    }

    // ---------------------------------------------------------------- threads and forums

    fn load_forum(&mut self, forum: Id, cx: &mut Context<Self>) {
        if !self.forums_loaded.insert(forum) {
            return;
        }
        let Some(rest) = self.rest.clone() else { return };
        let guild = self.channels.get(&forum).and_then(|c| c.guild_id);
        self.run(cx, async move { rest.forum_threads(forum).await }, move |store, result, cx| {
            // Archived posts are a bonus; a failure here should not nag the user.
            for mut thread in result.unwrap_or_default() {
                thread.guild_id = thread.guild_id.or(guild);
                thread.parent_id = thread.parent_id.or(Some(forum));
                if !store.channels.contains_key(&thread.id) {
                    store.upsert_channel(thread, cx);
                }
            }
        });
    }

    /// Posts of a forum, most recently active first.
    pub fn forum_posts(&self, forum: Id) -> Vec<&Channel> {
        let mut posts: Vec<&Channel> =
            self.channels.values().filter(|c| c.parent_id == Some(forum) && c.is_thread()).collect();
        posts.sort_by_key(|c| std::cmp::Reverse(c.last_message_id.unwrap_or(c.id)));
        posts
    }

    fn adopt_thread(&mut self, mut thread: Channel, parent: Id, cx: &mut Context<Self>) {
        let id = thread.id;
        thread.guild_id = thread.guild_id.or_else(|| self.channels.get(&parent).and_then(|c| c.guild_id));
        thread.parent_id = thread.parent_id.or(Some(parent));
        self.upsert_channel(thread, cx);
        self.select_channel(id, cx);
    }

    fn demo_thread(&mut self, parent: Id, name: String, cx: &mut Context<Self>) -> Id {
        let id = Id::now();
        let thread = Channel {
            id,
            kind: channel_kind::PUBLIC_THREAD,
            name: Some(name),
            owner_id: Some(self.my_id()),
            ..Default::default()
        };
        self.messages.insert(id, ChannelMessages { loaded: true, reached_start: true, ..Default::default() });
        self.adopt_thread(thread, parent, cx);
        id
    }

    pub fn create_thread(&mut self, channel: Id, message: Id, name: String, cx: &mut Context<Self>) {
        if self.demo {
            self.demo_thread(channel, name, cx);
            return;
        }
        let Some(rest) = self.rest.clone() else { return };
        self.run(cx, async move { rest.create_thread(channel, message, &name).await }, move |store, result, cx| {
            if let Some(thread) = store.report(result, "Could not create thread", cx) {
                store.adopt_thread(thread, channel, cx);
            }
        });
    }

    pub fn create_post(&mut self, forum: Id, title: String, content: String, cx: &mut Context<Self>) {
        if self.demo {
            let thread = self.demo_thread(forum, title, cx);
            self.send_message(thread, content, None, Vec::new(), cx);
            return;
        }
        let Some(rest) = self.rest.clone() else { return };
        self.run(cx, async move { rest.create_post(forum, &title, &content).await }, move |store, result, cx| {
            if let Some(thread) = store.report(result, "Could not create post", cx) {
                store.adopt_thread(thread, forum, cx);
            }
        });
    }

    // ---------------------------------------------------------------- slash commands and forwarding

    fn command_scope(&self, channel: Id) -> Id {
        self.channels.get(&channel).and_then(|c| c.guild_id).unwrap_or(channel)
    }

    pub fn commands_for(&self, channel: Id) -> Option<Rc<Vec<SlashCommand>>> {
        self.slash_commands.get(&self.command_scope(channel)).cloned()
    }

    /// Fetches the slash commands usable in `channel`, once per server.
    pub fn load_commands(&mut self, channel: Id, cx: &mut Context<Self>) {
        let scope = self.command_scope(channel);
        if self.slash_commands.contains_key(&scope) {
            return;
        }
        // Reserve the slot so that typing does not start several fetches.
        self.slash_commands.insert(scope, Rc::new(Vec::new()));
        if self.demo {
            let sample = serde_json::json!({
                "applications": [{ "id": "5", "name": "BuildBot" }],
                "application_commands": [
                    { "id": "9001", "application_id": "5", "version": "1", "type": 1, "name": "deploy",
                      "description": "Deploy a branch", "options": [
                        { "type": 3, "name": "branch", "description": "", "required": true },
                        { "type": 5, "name": "force", "description": "" },
                    ]},
                    { "id": "9002", "application_id": "5", "version": "1", "type": 1, "name": "status",
                      "description": "Show build status", "options": [] },
                    { "id": "9003", "application_id": "5", "version": "1", "type": 1, "name": "artifact",
                      "description": "Attach a build artifact", "options": [
                        { "type": 11, "name": "file", "description": "", "required": true },
                        { "type": 3, "name": "note", "description": "" },
                    ]},
                ],
            });
            self.slash_commands.insert(scope, Rc::new(commands::parse_index(&sample)));
            cx.emit(StoreEvent::Commands);
            return;
        }
        let Some(rest) = self.rest.clone().filter(|r| !r.is_bot()) else { return };
        let guild = self.channels.get(&channel).and_then(|c| c.guild_id);
        self.run(cx, async move { rest.command_index(guild, channel).await }, move |store, result, cx| {
            match result {
                Ok(index) => {
                    store.slash_commands.insert(scope, Rc::new(commands::parse_index(&index)));
                    cx.emit(StoreEvent::Commands);
                }
                Err(e) => {
                    // Allow a later retry.
                    store.slash_commands.remove(&scope);
                    log::warn!("could not load slash commands: {e}");
                }
            }
        });
    }

    /// Runs a slash command, uploading `files` for its file options. Returns
    /// false if the arguments were rejected, so the caller can leave the text
    /// in place for the user to fix.
    pub fn run_command(
        &mut self,
        channel: Id,
        command: &SlashCommand,
        args: &str,
        files: Vec<std::path::PathBuf>,
        cx: &mut Context<Self>,
    ) -> bool {
        let mut files = files;
        files.truncate(command.file_options());
        let names: Vec<String> = files
            .iter()
            .map(|p| p.file_name().map_or_else(|| "file".into(), |n| n.to_string_lossy().into_owned()))
            .collect();
        let data = match commands::interaction_data(command, args, &names) {
            Ok(data) => data,
            Err(reason) => {
                cx.emit(StoreEvent::Error(reason));
                return false;
            }
        };
        if self.demo {
            let with = match names.len() {
                0 => String::new(),
                1 => format!(" with {}", names[0]),
                n => format!(" with {n} files"),
            };
            cx.emit(StoreEvent::Notice(format!(
                "/{}{with} was accepted. Demo mode does not run commands.",
                command.full_name()
            )));
            return true;
        }
        let Some(rest) = self.rest.clone() else { return false };
        let guild = self.channels.get(&channel).and_then(|c| c.guild_id);
        let payload = serde_json::json!({
            "type": 2,
            "application_id": command.application_id.to_string(),
            "guild_id": guild.map(|g| g.to_string()),
            "channel_id": channel.to_string(),
            "session_id": self.gateway_session,
            "nonce": Id::now().to_string(),
            "data": data,
        });
        let read = move || -> Result<Vec<Upload>> {
            files
                .into_iter()
                .zip(names)
                .map(|(path, filename)| Ok(Upload { filename, bytes: std::fs::read(path)? }))
                .collect()
        };
        self.run(
            cx,
            async move {
                let uploads = tokio::task::spawn_blocking(read).await??;
                rest.interact(payload, uploads).await
            },
            |store, result, cx| {
                store.report(result, "The command failed", cx);
            },
        );
        true
    }

    pub fn forward_message(&mut self, target: Id, source: Id, message: Id, cx: &mut Context<Self>) {
        let original = self.messages.get(&source).and_then(|e| e.list.iter().find(|m| m.m.id == message)).cloned();
        let notice = self.channels.get(&target).map(|c| format!("Forwarded to {}.", self.channel_title(c)));
        if self.demo {
            let (Some(original), Some(me)) = (original, self.me.clone()) else { return };
            let snapshot = discord::Snapshot {
                message: discord::SnapshotMessage {
                    content: original.m.content.clone(),
                    attachments: original.m.attachments.clone(),
                    embeds: original.m.embeds.clone(),
                },
            };
            let local = Message {
                id: Id::now(),
                channel_id: target,
                author: me,
                message_snapshots: vec![snapshot],
                ..Default::default()
            };
            let msg = self.make_msg(local);
            self.messages.entry(target).or_default().list.push(msg);
            cx.emit(StoreEvent::Messages { channel: target, change: MessageChange::Appended(1) });
            cx.emit(StoreEvent::Notice(notice.unwrap_or_default()));
            return;
        }
        let Some(rest) = self.rest.clone() else { return };
        let guild = self.channels.get(&source).and_then(|c| c.guild_id);
        let nonce = Id::now();
        self.run(
            cx,
            async move { rest.forward(target, source, message, guild, nonce).await },
            move |store, result, cx| {
                if let Some(sent) = store.report(result, "Could not forward message", cx) {
                    store.insert_message(sent, Some(nonce), cx);
                    cx.emit(StoreEvent::Notice(notice.unwrap_or_default()));
                }
            },
        );
    }

    pub fn toggle_folder(&mut self, folder: i64, cx: &mut Context<Self>) {
        if !self.collapsed_folders.remove(&folder) {
            self.collapsed_folders.insert(folder);
        }
        cx.emit(StoreEvent::Guilds);
    }

    // ---------------------------------------------------------------- GIFs and stickers

    pub fn search_gifs(
        &mut self,
        query: String,
        cx: &mut Context<Self>,
        then: impl FnOnce(Result<Vec<Gif>, String>, &mut gpui::App) + 'static,
    ) {
        let unavailable = if self.demo {
            Some("GIF search needs a signed-in account.")
        } else if self.rest.as_ref().is_none_or(|r| r.is_bot()) {
            Some("GIF search is not available to bot accounts.")
        } else {
            None
        };
        if let Some(reason) = unavailable {
            cx.defer(move |cx| then(Err(reason.to_string()), cx));
            return;
        }
        let Some(rest) = self.rest.clone() else { return };
        self.run(cx, async move { rest.gifs(&query).await }, move |_, result, cx| {
            then(result.map_err(|e| e.to_string()), cx);
        });
    }

    /// Stickers of the channel's server.
    pub fn stickers(&self, channel: Id) -> Vec<StickerItem> {
        self.channels
            .get(&channel)
            .and_then(|c| c.guild_id)
            .and_then(|g| self.guild(g))
            .map(|g| g.guild.stickers.clone())
            .unwrap_or_default()
    }

    pub fn send_sticker(&mut self, channel: Id, sticker: StickerItem, cx: &mut Context<Self>) {
        if self.demo {
            let Some(me) = self.me.clone() else { return };
            let local = Message {
                id: Id::now(),
                channel_id: channel,
                author: me,
                sticker_items: vec![sticker],
                ..Default::default()
            };
            let msg = self.make_msg(local);
            self.messages.entry(channel).or_default().list.push(msg);
            cx.emit(StoreEvent::Messages { channel, change: MessageChange::Appended(1) });
            return;
        }
        let Some(rest) = self.rest.clone() else { return };
        let nonce = Id::now();
        let id = sticker.id;
        self.run(cx, async move { rest.send_sticker(channel, id, nonce).await }, move |store, result, cx| {
            if let Some(sent) = store.report(result, "Could not send sticker", cx) {
                store.insert_message(sent, Some(nonce), cx);
            }
        });
    }

    // ---------------------------------------------------------------- polls and interactions

    pub fn vote_poll(&mut self, channel: Id, message: Id, answer: u32, cx: &mut Context<Self>) {
        let Some(poll) = self
            .messages
            .get(&channel)
            .and_then(|e| e.list.iter().find(|m| m.m.id == message))
            .and_then(|m| m.m.poll.clone())
        else {
            return;
        };
        let (_, voted) = poll.count(answer);
        if self.demo {
            self.modify_message(channel, message, cx, |m| {
                if let Some(poll) = m.poll.as_mut() {
                    if !poll.allow_multiselect && !voted {
                        for other in poll.answers.clone() {
                            if poll.count(other.answer_id).1 {
                                poll.apply_vote(other.answer_id, false, true);
                            }
                        }
                    }
                    poll.apply_vote(answer, !voted, true);
                }
            });
            return;
        }
        let Some(rest) = self.rest.clone() else { return };
        if rest.is_bot() {
            cx.emit(StoreEvent::Error("Bots cannot vote in polls.".into()));
            return;
        }
        let mut mine: Vec<u32> =
            poll.answers.iter().map(|a| a.answer_id).filter(|a| poll.count(*a).1 && *a != answer).collect();
        if !poll.allow_multiselect {
            mine.clear();
        }
        if !voted {
            mine.push(answer);
        }
        self.run(cx, async move { rest.vote_poll(channel, message, &mine).await }, |store, result, cx| {
            store.report(result, "Could not vote", cx);
        });
    }

    /// Presses a button or picks a select-menu option on a bot's message.
    pub fn interact(
        &mut self,
        channel: Id,
        message: Id,
        custom_id: String,
        component_type: u8,
        value: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(rest) = self.rest.clone().filter(|_| !self.demo) else {
            cx.emit(StoreEvent::Error("Interactions are not available in demo mode.".into()));
            return;
        };
        if rest.is_bot() {
            cx.emit(StoreEvent::Error("Bots cannot use other bots' buttons.".into()));
            return;
        }
        let Some(msg) = self.messages.get(&channel).and_then(|e| e.list.iter().find(|m| m.m.id == message)) else {
            return;
        };
        let guild = self.channels.get(&channel).and_then(|c| c.guild_id);
        let mut data = serde_json::json!({ "component_type": component_type, "custom_id": custom_id });
        if let Some(value) = value {
            data["type"] = component_type.into();
            data["values"] = serde_json::json!([value]);
        }
        let payload = serde_json::json!({
            "type": 3,
            "nonce": Id::now().to_string(),
            "guild_id": guild.map(|g| g.to_string()),
            "channel_id": channel.to_string(),
            "message_flags": msg.m.flags,
            "message_id": message.to_string(),
            "application_id": msg.m.application_id.unwrap_or(msg.m.author.id).to_string(),
            "session_id": self.gateway_session,
            "data": data,
        });
        self.run(cx, async move { rest.interact(payload, Vec::new()).await }, |store, result, cx| {
            store.report(result, "The interaction failed", cx);
        });
    }

    // ---------------------------------------------------------------- messages

    fn mentions_me(&self, m: &Message) -> bool {
        let guild = m.guild_id.or_else(|| self.channels.get(&m.channel_id).and_then(|c| c.guild_id));
        m.mentions_user(self.my_id())
            || guild
                .and_then(|g| self.guild(g))
                .is_some_and(|g| m.mention_roles.iter().any(|r| g.own_roles.contains(r)))
    }

    fn make_msg(&self, m: Message) -> Rc<Msg> {
        let guild = m.guild_id.or_else(|| self.channels.get(&m.channel_id).and_then(|c| c.guild_id));
        let resolver = StoreResolver { store: self, guild, mentions: &m.mentions };
        let doc = markdown::parse(&m.content, &resolver);
        let mentions_me = self.mentions_me(&m);
        Rc::new(Msg { time: m.time(), doc, m, pending: false, failed: false, mentions_me })
    }

    /// Remembers member details (nickname, role colour) seen on a message.
    fn learn_member(&mut self, guild: Id, user: &User, member: &Member) {
        let color = self.guild(guild).and_then(|g| {
            g.guild
                .roles
                .iter()
                .filter(|r| r.color != 0 && member.roles.contains(&r.id))
                .max_by_key(|r| r.position)
                .map(|r| r.color)
        });
        self.member_info.insert((guild, user.id), MemberInfo { nick: member.nick.clone(), color, roles: member.roles.clone() });
        self.users.entry(user.id).or_insert_with(|| user.clone());
    }

    fn learn_from(&mut self, m: &Message) {
        let guild = m.guild_id.or_else(|| self.channels.get(&m.channel_id).and_then(|c| c.guild_id));
        if let (Some(guild), Some(member)) = (guild, &m.member) {
            if !self.member_info.contains_key(&(guild, m.author.id)) {
                self.learn_member(guild, &m.author.clone(), &member.clone());
            }
        }
        self.users.entry(m.author.id).or_insert_with(|| m.author.clone());
    }

    pub fn load_history(&mut self, channel: Id, cx: &mut Context<Self>) {
        let Some(rest) = self.rest.clone() else { return };
        let entry = self.messages.entry(channel).or_default();
        if entry.loading || entry.reached_start {
            return;
        }
        entry.loading = true;
        let before = entry.list.iter().find(|m| !m.pending).map(|m| m.m.id);
        let initial = !entry.loaded;
        self.run(cx, async move { rest.messages(channel, before, PAGE).await }, move |store, result, cx| {
            let loaded = store.report(result, "Could not load messages", cx);
            let fetched = loaded.is_some();
            let mut batch = loaded.unwrap_or_default();
            batch.reverse(); // oldest first
            for m in &batch {
                store.learn_from(m);
            }
            let msgs: Vec<Rc<Msg>> = batch.into_iter().map(|m| store.make_msg(m)).collect();
            let count = msgs.len();
            let entry = store.messages.entry(channel).or_default();
            entry.loading = false;
            if fetched {
                entry.loaded = true;
                if count < PAGE as usize {
                    entry.reached_start = true;
                }
            }
            if initial {
                // Keep anything that arrived over the gateway while loading.
                let newest = msgs.last().map(|m| m.m.id);
                let live: Vec<_> =
                    entry.list.drain(..).filter(|m| newest.is_none_or(|n| m.m.id > n)).collect();
                entry.list = msgs;
                entry.list.extend(live);
                cx.emit(StoreEvent::Messages { channel, change: MessageChange::Reset });
            } else {
                entry.list.splice(0..0, msgs);
                cx.emit(StoreEvent::Messages { channel, change: MessageChange::Prepended(count) });
            }
        });
    }

    pub fn send_message(
        &mut self,
        channel: Id,
        content: String,
        reply_to: Option<Id>,
        uploads: Vec<Upload>,
        cx: &mut Context<Self>,
    ) {
        if self.demo {
            let Some(me) = self.me.clone() else { return };
            let local = Message { id: Id::now(), channel_id: channel, author: me, content, ..Default::default() };
            let msg = self.make_msg(local);
            self.messages.entry(channel).or_default().list.push(msg);
            cx.emit(StoreEvent::Messages { channel, change: MessageChange::Appended(1) });
            return;
        }
        let (Some(rest), Some(me)) = (self.rest.clone(), self.me.clone()) else { return };
        let nonce = Id::now();

        // Show the message immediately; the gateway echo replaces it.
        let referenced = reply_to.and_then(|id| {
            self.messages.get(&channel)?.list.iter().find(|m| m.m.id == id).map(|m| Box::new(m.m.clone()))
        });
        let placeholder = Message {
            id: nonce,
            channel_id: channel,
            author: me,
            content: if content.is_empty() && !uploads.is_empty() {
                format!("Uploading {} file(s)…", uploads.len())
            } else {
                content.clone()
            },
            kind: if reply_to.is_some() { discord::message_kind::REPLY } else { 0 },
            referenced_message: referenced,
            ..Default::default()
        };
        let mut msg = self.make_msg(placeholder);
        Rc::get_mut(&mut msg).unwrap().pending = true;
        self.messages.entry(channel).or_default().list.push(msg);
        cx.emit(StoreEvent::Messages { channel, change: MessageChange::Appended(1) });

        self.run(
            cx,
            async move { rest.send_message(channel, &content, reply_to, nonce, uploads).await },
            move |store, result, cx| match result {
                Ok(sent) => store.insert_message(sent, Some(nonce), cx),
                Err(e) => {
                    if let Some(entry) = store.messages.get_mut(&channel) {
                        if let Some(ix) = entry.list.iter().position(|m| m.pending && m.m.id == nonce) {
                            let old = &entry.list[ix];
                            entry.list[ix] = Rc::new(Msg {
                                m: old.m.clone(),
                                doc: old.doc.clone(),
                                time: old.time,
                                pending: true,
                                failed: true,
                                mentions_me: false,
                            });
                            cx.emit(StoreEvent::Messages { channel, change: MessageChange::Updated(ix) });
                        }
                    }
                    cx.emit(StoreEvent::Error(format!("Message not sent: {e}")));
                }
            },
        );
    }

    /// Removes a message that failed to send.
    pub fn discard_failed(&mut self, channel: Id, id: Id, cx: &mut Context<Self>) {
        if let Some(entry) = self.messages.get_mut(&channel) {
            if let Some(ix) = entry.list.iter().position(|m| m.failed && m.m.id == id) {
                entry.list.remove(ix);
                cx.emit(StoreEvent::Messages { channel, change: MessageChange::Removed(ix) });
            }
        }
    }

    fn nonce_of(m: &Message) -> Option<Id> {
        match m.nonce.as_ref()? {
            serde_json::Value::String(s) => s.parse().ok().map(Id),
            serde_json::Value::Number(n) => n.as_u64().map(Id),
            _ => None,
        }
    }

    /// Inserts a new message, replacing its optimistic placeholder if present.
    fn insert_message(&mut self, m: Message, nonce: Option<Id>, cx: &mut Context<Self>) {
        let channel = m.channel_id;
        self.learn_from(&m);
        let nonce = nonce.or_else(|| Self::nonce_of(&m));
        let msg = self.make_msg(m);
        let id = msg.m.id;
        let entry = self.messages.entry(channel).or_default();

        if let Some(ix) = entry.list.iter().position(|x| x.m.id == id) {
            entry.list[ix] = msg;
            // The REST response and the gateway echo can both land; drop a leftover placeholder.
            if let Some(n) = nonce {
                if let Some(pix) = entry.list.iter().position(|x| x.pending && x.m.id == n) {
                    entry.list.remove(pix);
                    cx.emit(StoreEvent::Messages { channel, change: MessageChange::Removed(pix) });
                    return;
                }
            }
            cx.emit(StoreEvent::Messages { channel, change: MessageChange::Updated(ix) });
            return;
        }
        if let Some(ix) = nonce.and_then(|n| entry.list.iter().position(|x| x.pending && x.m.id == n)) {
            entry.list[ix] = msg;
            cx.emit(StoreEvent::Messages { channel, change: MessageChange::Updated(ix) });
            return;
        }
        if !entry.loaded && !entry.loading {
            // Not viewing this channel; history will be fetched on demand.
            return;
        }
        entry.list.push(msg);
        cx.emit(StoreEvent::Messages { channel, change: MessageChange::Appended(1) });
    }

    pub fn edit_message(&mut self, channel: Id, message: Id, content: String, cx: &mut Context<Self>) {
        let Some(rest) = self.rest.clone() else { return };
        self.run(cx, async move { rest.edit_message(channel, message, &content).await }, |store, result, cx| {
            store.report(result, "Could not edit message", cx);
        });
    }

    pub fn delete_message(&mut self, channel: Id, message: Id, cx: &mut Context<Self>) {
        let Some(rest) = self.rest.clone() else { return };
        self.run(cx, async move { rest.delete_message(channel, message).await }, |store, result, cx| {
            store.report(result, "Could not delete message", cx);
        });
    }

    pub fn set_pinned(&mut self, channel: Id, message: Id, pinned: bool, cx: &mut Context<Self>) {
        if self.demo {
            self.modify_message(channel, message, cx, |m| m.pinned = pinned);
            return;
        }
        let Some(rest) = self.rest.clone() else { return };
        self.run(cx, async move { rest.set_pinned(channel, message, pinned).await }, |store, result, cx| {
            store.report(result, "Could not change pin", cx);
        });
    }

    pub fn toggle_reaction(&mut self, channel: Id, message: Id, emoji: Emoji, cx: &mut Context<Self>) {
        if self.demo {
            self.modify_message(channel, message, cx, |m| {
                match m.reactions.iter_mut().find(|r| r.emoji.same_as(&emoji)) {
                    Some(r) if r.me => {
                        r.me = false;
                        r.count -= 1;
                    }
                    Some(r) => {
                        r.me = true;
                        r.count += 1;
                    }
                    None => m.reactions.push(Reaction { count: 1, me: true, emoji }),
                }
                m.reactions.retain(|r| r.count > 0);
            });
            return;
        }
        let Some(rest) = self.rest.clone() else { return };
        let mine = self
            .messages
            .get(&channel)
            .and_then(|e| e.list.iter().find(|m| m.m.id == message))
            .is_some_and(|m| m.m.reactions.iter().any(|r| r.me && r.emoji.same_as(&emoji)));
        let name = emoji.api_name();
        self.run(cx, async move { rest.set_reaction(channel, message, &name, !mine).await }, |store, result, cx| {
            store.report(result, "Could not update reaction", cx);
        });
    }

    pub fn send_typing(&mut self, channel: Id, cx: &mut Context<Self>) {
        let Some(rest) = self.rest.clone() else { return };
        self.run(cx, async move { rest.typing(channel).await }, |_, _, _| {});
    }

    pub fn fetch_pins(
        &mut self,
        channel: Id,
        cx: &mut Context<Self>,
        then: impl FnOnce(Vec<Rc<Msg>>, &mut gpui::App) + 'static,
    ) {
        if self.demo {
            let pins = self.local_messages(channel, |m| m.pinned);
            cx.defer(move |cx| then(pins, cx));
            return;
        }
        let Some(rest) = self.rest.clone() else { return };
        self.run(cx, async move { rest.pins(channel).await }, move |store, result, cx| {
            if let Some(pins) = store.report(result, "Could not load pins", cx) {
                let msgs = pins.into_iter().map(|m| store.make_msg(m)).collect();
                then(msgs, cx);
            }
        });
    }

    fn local_messages(&self, channel: Id, keep: impl Fn(&Message) -> bool) -> Vec<Rc<Msg>> {
        self.messages
            .get(&channel)
            .map(|e| e.list.iter().rev().filter(|m| keep(&m.m)).cloned().collect())
            .unwrap_or_default()
    }

    /// Searches the channel's guild (or the DM) for `query`.
    pub fn search(
        &mut self,
        channel: Id,
        query: String,
        cx: &mut Context<Self>,
        then: impl FnOnce(Vec<Rc<Msg>>, &mut gpui::App) + 'static,
    ) {
        if self.demo {
            let needle = query.to_lowercase();
            let hits = self.local_messages(channel, |m| m.content.to_lowercase().contains(&needle));
            cx.defer(move |cx| then(hits, cx));
            return;
        }
        let Some(rest) = self.rest.clone() else { return };
        if rest.is_bot() {
            cx.emit(StoreEvent::Error("Search is not available to bot accounts.".into()));
            cx.defer(move |cx| then(Vec::new(), cx));
            return;
        }
        let guild = self.channels.get(&channel).and_then(|c| c.guild_id);
        self.run(cx, async move { rest.search(guild, channel, &query).await }, move |store, result, cx| {
            let hits = store.report(result, "Search failed", cx).unwrap_or_default();
            let msgs = hits.into_iter().map(|m| store.make_msg(m)).collect();
            then(msgs, cx);
        });
    }

    /// Rebuilds one message in place with `change` applied.
    fn modify_message(
        &mut self,
        channel: Id,
        message: Id,
        cx: &mut Context<Self>,
        change: impl FnOnce(&mut Message),
    ) {
        let Some(ix) = self.messages.get(&channel).and_then(|e| e.list.iter().position(|m| m.m.id == message))
        else {
            return;
        };
        let mut m = self.messages[&channel].list[ix].m.clone();
        change(&mut m);
        let msg = self.make_msg(m);
        self.messages.get_mut(&channel).unwrap().list[ix] = msg;
        cx.emit(StoreEvent::Messages { channel, change: MessageChange::Updated(ix) });
    }

    // ---------------------------------------------------------------- typing

    fn prune_typing(&mut self, cx: &mut Context<Self>) {
        let now = Instant::now();
        let mut changed = Vec::new();
        for (channel, list) in self.typing.iter_mut() {
            let before = list.len();
            list.retain(|t| now.duration_since(t.at) < TYPING_TTL);
            if list.len() != before {
                changed.push(*channel);
            }
        }
        self.typing.retain(|_, list| !list.is_empty());
        for channel in changed {
            cx.emit(StoreEvent::Typing(channel));
        }
    }

    fn stop_typing(&mut self, channel: Id, user: Id, cx: &mut Context<Self>) {
        if let Some(list) = self.typing.get_mut(&channel) {
            let before = list.len();
            list.retain(|t| t.user != user);
            if list.len() != before {
                cx.emit(StoreEvent::Typing(channel));
            }
        }
    }

    // ---------------------------------------------------------------- channels

    fn upsert_channel(&mut self, mut channel: Channel, cx: &mut Context<Self>) {
        let id = channel.id;
        if let Some(old) = self.channels.get(&id) {
            // Updates may omit fields we already know.
            if channel.last_message_id.is_none() {
                channel.last_message_id = old.last_message_id;
            }
            if channel.recipients.is_empty() {
                channel.recipients = old.recipients.clone();
            }
            if channel.guild_id.is_none() {
                channel.guild_id = old.guild_id;
            }
        }
        for user in &channel.recipients {
            self.users.entry(user.id).or_insert_with(|| user.clone());
        }
        let guild = channel.guild_id;
        let is_dm = channel.is_dm();
        self.channels.insert(id, channel);
        if is_dm {
            if !self.dms.contains(&id) {
                self.dms.push(id);
            }
            self.sort_dms();
        } else if let Some(guild) = guild {
            self.rebuild_rows(guild);
        }
        cx.emit(StoreEvent::Channels);
    }

    fn sort_dms(&mut self) {
        let channels = &self.channels;
        self.dms.sort_by_key(|id| {
            let last = channels.get(id).and_then(|c| c.last_message_id).unwrap_or(*id);
            std::cmp::Reverse(last)
        });
    }

    /// Recomputes the visible, ordered channel list of a guild.
    fn rebuild_rows(&mut self, guild_id: Id) {
        let me = self.my_id();
        let Some(guild) = self.guild(guild_id) else { return };
        let visible = |c: &Channel| {
            permissions::in_channel(&guild.guild, c, me, &guild.own_roles) & permissions::VIEW_CHANNEL != 0
        };
        let mut all: Vec<&Channel> = self.channels.values().filter(|c| c.guild_id == Some(guild_id)).collect();
        // Voice-like channels sort after text channels within a category.
        let voice_rank = |c: &Channel| matches!(c.kind, channel_kind::VOICE | channel_kind::STAGE) as u8;
        all.sort_by_key(|c| (voice_rank(c), c.position, c.id));

        let mut threads: HashMap<Id, Vec<Id>> = HashMap::new();
        for c in all.iter().filter(|c| c.is_thread()) {
            if let Some(parent) = c.parent_id {
                // Forum posts are listed in the forum's own view, not the sidebar.
                if self.channels.get(&parent).is_some_and(|p| p.is_forum()) {
                    continue;
                }
                threads.entry(parent).or_default().push(c.id);
            }
        }

        let mut rows = Vec::new();
        let push_children = |rows: &mut Vec<ChannelRow>, parent: Option<Id>| {
            for c in all.iter().filter(|c| {
                c.parent_id == parent && c.kind != channel_kind::CATEGORY && !c.is_thread() && visible(c)
            }) {
                rows.push(ChannelRow { id: c.id, kind: RowKind::Channel });
                for thread in threads.get(&c.id).into_iter().flatten() {
                    rows.push(ChannelRow { id: *thread, kind: RowKind::Thread });
                }
            }
        };
        push_children(&mut rows, None);
        let mut categories: Vec<&&Channel> = all.iter().filter(|c| c.kind == channel_kind::CATEGORY).collect();
        categories.sort_by_key(|c| (c.position, c.id));
        for category in categories {
            let start = rows.len();
            rows.push(ChannelRow { id: category.id, kind: RowKind::Category });
            push_children(&mut rows, Some(category.id));
            if rows.len() == start + 1 {
                rows.pop(); // hide empty or fully hidden categories
            }
        }
        if let Some(guild) = self.guild_mut(guild_id) {
            guild.rows = rows;
        }
    }

    fn add_guild(&mut self, mut guild: Guild, own_roles: Option<Vec<Id>>) {
        let id = guild.id;
        for channel in guild.channels.drain(..).chain(guild.threads.drain(..)) {
            let mut channel = channel;
            channel.guild_id = Some(id);
            self.channels.insert(channel.id, channel);
        }
        for mut state in std::mem::take(&mut guild.voice_states) {
            state.guild_id = Some(id);
            self.track_voice_state(&state);
        }
        for stage in std::mem::take(&mut guild.stage_instances) {
            self.stages.insert(stage.channel_id, stage.topic);
        }
        let members = std::mem::take(&mut guild.members);
        // Updates carry no events; keep what we have.
        let events = std::mem::take(&mut guild.guild_scheduled_events);
        if !events.is_empty() || !self.events.contains_key(&id) {
            self.events.insert(id, events);
        }
        match self.guild_mut(id) {
            Some(existing) => {
                // GUILD_UPDATE carries no channels; keep what we have.
                if guild.roles.is_empty() {
                    guild.roles = std::mem::take(&mut existing.guild.roles);
                }
                existing.guild = guild;
                if let Some(roles) = own_roles {
                    existing.own_roles = roles;
                }
            }
            None => self.guilds.push(GuildState { guild, own_roles: own_roles.unwrap_or_default(), rows: Vec::new() }),
        }
        for member in members {
            if let Some(user) = member.user.clone() {
                self.learn_member(id, &user, &member);
            }
        }
        self.rebuild_rows(id);
    }

    // ---------------------------------------------------------------- gateway

    fn apply(&mut self, event: Event, cx: &mut Context<Self>) {
        match event {
            Event::Connecting => {
                if self.connection != Connection::Ready && self.connection != Connection::Reconnecting {
                    self.connection = Connection::Connecting;
                    cx.emit(StoreEvent::Connection);
                }
            }
            Event::Reconnecting => {
                if self.connection == Connection::Ready {
                    self.connection = Connection::Reconnecting;
                    cx.emit(StoreEvent::Connection);
                }
            }
            Event::Fatal(reason) => {
                self.shutdown();
                self.connection = Connection::Failed(reason);
                cx.emit(StoreEvent::Connection);
            }
            Event::Resumed => {
                self.connection = Connection::Ready;
                cx.emit(StoreEvent::Connection);
            }
            Event::Ready(ready) => self.apply_ready(*ready, cx),
            Event::GuildCreate(guild) => {
                self.add_guild(*guild, None);
                cx.emit(StoreEvent::Guilds);
                cx.emit(StoreEvent::Channels);
            }
            Event::OwnMemberUpdate { guild_id, roles } => {
                if let Some(guild) = self.guild_mut(guild_id) {
                    guild.own_roles = roles;
                }
                self.rebuild_rows(guild_id);
                cx.emit(StoreEvent::Channels);
            }
            Event::GuildDelete(id) => {
                self.guilds.retain(|g| g.guild.id != id);
                self.channels.retain(|_, c| c.guild_id != Some(id));
                if self.selected_guild == Some(id) {
                    self.selected_channel = None;
                    self.select_guild(None, cx);
                }
                cx.emit(StoreEvent::Guilds);
            }
            Event::ChannelUpsert(channel) => self.upsert_channel(*channel, cx),
            Event::ChannelDelete(id) => {
                if let Some(channel) = self.channels.remove(&id) {
                    self.dms.retain(|d| *d != id);
                    self.messages.remove(&id);
                    if let Some(guild) = channel.guild_id {
                        self.rebuild_rows(guild);
                    }
                    if self.selected_channel == Some(id) {
                        self.selected_channel = None;
                        self.last_channel.remove(&channel.guild_id);
                        let guild = self.selected_guild;
                        self.select_guild(guild, cx);
                    }
                    cx.emit(StoreEvent::Channels);
                }
            }
            Event::MessageCreate(m) => {
                let m = *m;
                let (channel, id, author) = (m.channel_id, m.id, m.author.id);
                let me = self.my_id();
                let mentioned = author != me && self.mentions_me(&m);
                let is_dm = match self.channels.get_mut(&channel) {
                    Some(c) => {
                        c.last_message_id = Some(id);
                        c.is_dm()
                    }
                    None => false,
                };
                if is_dm {
                    self.sort_dms();
                    cx.emit(StoreEvent::Channels);
                }
                let muted = self.is_muted(channel);
                // Prepare the toast before the message moves into the list.
                let notice = (mentioned || is_dm).then(|| {
                    let info = self.channels.get(&channel);
                    let who = self.display_name(info.and_then(|c| c.guild_id), &m.author);
                    let place = match info {
                        Some(c) if c.is_dm() => String::new(),
                        Some(c) => format!(" in #{}", self.channel_title(c)),
                        None => String::new(),
                    };
                    let mut body: String = m.content.chars().take(140).collect();
                    if body.is_empty() {
                        body = "Sent an attachment".into();
                    }
                    StoreEvent::Mention { title: format!("{who}{place}"), body }
                });
                self.stop_typing(channel, author, cx);
                self.insert_message(m, None, cx);

                if self.selected_channel == Some(channel) || author == me {
                    self.mark_read(channel, cx);
                } else {
                    let marker = self.read.entry(channel).or_default();
                    if mentioned || is_dm {
                        marker.mentions += 1;
                        if let Some(notice) = notice.filter(|_| !muted) {
                            cx.emit(notice);
                        }
                    }
                    cx.emit(StoreEvent::Unread);
                }
            }
            Event::MessageUpdate(v) => {
                let (Some(channel), Some(id)) = (json_id(&v["channel_id"]), json_id(&v["id"])) else { return };
                self.modify_message(channel, id, cx, |m| {
                    if let Some(content) = v["content"].as_str() {
                        m.content = content.to_string();
                    }
                    if let Some(edited) = v["edited_timestamp"].as_str() {
                        m.edited_timestamp = Some(edited.to_string());
                    }
                    if let Some(pinned) = v["pinned"].as_bool() {
                        m.pinned = pinned;
                    }
                    for (key, apply) in [
                        ("embeds", &(|m: &mut Message, v| m.embeds = serde_json::from_value(v).unwrap_or_default())
                            as &dyn Fn(&mut Message, serde_json::Value)),
                        ("attachments", &|m, v| m.attachments = serde_json::from_value(v).unwrap_or_default()),
                        ("mentions", &|m, v| m.mentions = serde_json::from_value(v).unwrap_or_default()),
                    ] {
                        if v[key].is_array() {
                            apply(m, v[key].clone());
                        }
                    }
                });
            }
            Event::MessageDelete { channel_id, ids } => {
                let Some(entry) = self.messages.get_mut(&channel_id) else { return };
                for id in ids {
                    if let Some(ix) = entry.list.iter().position(|m| m.m.id == id) {
                        entry.list.remove(ix);
                        cx.emit(StoreEvent::Messages { channel: channel_id, change: MessageChange::Removed(ix) });
                    }
                }
            }
            Event::ReactionAdd { channel_id, message_id, user_id, emoji } => {
                let me = self.my_id() == user_id;
                self.modify_message(channel_id, message_id, cx, |m| {
                    match m.reactions.iter_mut().find(|r| r.emoji.same_as(&emoji)) {
                        Some(r) => {
                            r.count += 1;
                            r.me |= me;
                        }
                        None => m.reactions.push(Reaction { count: 1, me, emoji }),
                    }
                });
            }
            Event::ReactionRemove { channel_id, message_id, user_id, emoji } => {
                let me = self.my_id() == user_id;
                self.modify_message(channel_id, message_id, cx, |m| {
                    if let Some(r) = m.reactions.iter_mut().find(|r| r.emoji.same_as(&emoji)) {
                        r.count = r.count.saturating_sub(1);
                        if me {
                            r.me = false;
                        }
                    }
                    m.reactions.retain(|r| r.count > 0);
                });
            }
            Event::ReactionClear { channel_id, message_id } => {
                self.modify_message(channel_id, message_id, cx, |m| m.reactions.clear());
            }
            Event::Typing { channel_id, user_id, member } => {
                if user_id == self.my_id() {
                    return;
                }
                let guild = self.channels.get(&channel_id).and_then(|c| c.guild_id);
                if let (Some(guild), Some(member)) = (guild, &member) {
                    if let Some(user) = &member.user {
                        if !self.member_info.contains_key(&(guild, user.id)) {
                            self.learn_member(guild, &user.clone(), member);
                        }
                    }
                }
                let name = match self.users.get(&user_id) {
                    Some(user) => self.display_name(guild, user),
                    None => "Someone".to_string(),
                };
                let list = self.typing.entry(channel_id).or_default();
                list.retain(|t| t.user != user_id);
                list.push(TypingUser { user: user_id, name, at: Instant::now() });
                cx.emit(StoreEvent::Typing(channel_id));
            }
            Event::VoiceState(state) => self.apply_voice_state(*state, cx),
            Event::VoiceServer { guild_id, channel_id, token, endpoint } => {
                let Some(session) = self.voice_session.as_mut() else { return };
                let ours = match session.guild {
                    Some(guild) => guild_id == Some(guild),
                    None => channel_id == Some(session.channel),
                };
                if !ours {
                    return;
                }
                if let Some(endpoint) = endpoint {
                    session.server = Some((token, endpoint));
                    self.try_connect_voice(cx);
                }
            }
            Event::Call { channel_id, ringing, ended } => {
                if ended {
                    self.calls.remove(&channel_id);
                } else {
                    let me = self.my_id();
                    let in_call = self.voice_session.as_ref().is_some_and(|s| s.channel == channel_id);
                    let was_ringing = self.calls.get(&channel_id).is_some_and(|r| r.contains(&me));
                    if ringing.contains(&me) && !was_ringing && !in_call {
                        let title = self
                            .channels
                            .get(&channel_id)
                            .map(|c| self.channel_title(c))
                            .unwrap_or_else(|| "Someone".into());
                        cx.emit(StoreEvent::IncomingCall { channel: channel_id, title });
                    }
                    self.calls.insert(channel_id, ringing);
                }
                cx.emit(StoreEvent::Voice);
            }
            Event::Stage { channel_id, topic } => {
                match topic {
                    Some(topic) => self.stages.insert(channel_id, topic),
                    None => self.stages.remove(&channel_id),
                };
                cx.emit(StoreEvent::Voice);
            }
            Event::Relationship { id, user, kind } => {
                if let Some(user) = user {
                    self.users.insert(user.id, user);
                }
                self.set_relationship(id, kind, cx);
            }
            Event::Mutes(changes) => {
                for (id, muted) in changes {
                    if muted {
                        self.muted.insert(id);
                    } else {
                        self.muted.remove(&id);
                    }
                }
                cx.emit(StoreEvent::Unread);
                cx.emit(StoreEvent::Channels);
            }
            Event::PollVote { channel_id, message_id, user_id, answer_id, added } => {
                let mine = user_id == self.my_id();
                self.modify_message(channel_id, message_id, cx, |m| {
                    if let Some(poll) = m.poll.as_mut() {
                        poll.apply_vote(answer_id, added, mine);
                    }
                });
            }
            Event::RoleUpsert { guild_id, role } => self.upsert_role(guild_id, role, cx),
            Event::RoleDelete { guild_id, role_id } => {
                if let Some(state) = self.guild_mut(guild_id) {
                    state.guild.roles.retain(|r| r.id != role_id);
                    state.own_roles.retain(|r| *r != role_id);
                }
                for ((g, _), info) in self.member_info.iter_mut() {
                    if *g == guild_id {
                        info.roles.retain(|r| *r != role_id);
                    }
                }
                self.recolor_members(guild_id);
                self.rebuild_rows(guild_id);
                cx.emit(StoreEvent::Guilds);
                cx.emit(StoreEvent::Channels);
                cx.emit(StoreEvent::Members(guild_id));
            }
            Event::MemberUpdate { guild_id, member } => {
                if let Some(user) = member.user.clone() {
                    self.learn_member(guild_id, &user, &member);
                    self.users.insert(user.id, user);
                }
                self.recolor_members(guild_id);
                cx.emit(StoreEvent::Members(guild_id));
            }
            Event::MemberRemove { guild_id, user_id } => {
                if let Some(rows) = self.member_lists.get_mut(&guild_id) {
                    rows.retain(|row| !matches!(row, MemberRow::Member { user, .. } if user.id == user_id));
                }
                self.member_info.remove(&(guild_id, user_id));
                cx.emit(StoreEvent::Members(guild_id));
            }
            Event::ScheduledEvent(event) => {
                if let Some(guild) = event.guild_id {
                    self.upsert_event(guild, *event, cx);
                }
            }
            Event::ScheduledEventDelete { guild_id, id } => {
                if let Some(list) = self.events.get_mut(&guild_id) {
                    list.retain(|e| e.id != id);
                }
                cx.emit(StoreEvent::Channels);
            }
            Event::EventInterest { event_id, user_id, added } => {
                if user_id == self.my_id() {
                    if added {
                        self.interested.insert(event_id);
                    } else {
                        self.interested.remove(&event_id);
                    }
                }
                cx.emit(StoreEvent::Channels);
            }
            Event::Emojis { guild_id, emojis } => {
                if let Some(state) = self.guild_mut(guild_id) {
                    state.guild.emojis = emojis;
                }
                cx.emit(StoreEvent::Guilds);
            }
            Event::Stickers { guild_id, stickers } => {
                if let Some(state) = self.guild_mut(guild_id) {
                    state.guild.stickers = stickers;
                }
                cx.emit(StoreEvent::Guilds);
            }
            Event::UserUpdate(user) => {
                if self.me.as_ref().is_some_and(|me| me.id == user.id) {
                    self.set_me(*user, cx);
                }
            }
            Event::Presence { user_id, status } => {
                self.presences.insert(user_id, status);
            }
            Event::MessageAck { channel_id, message_id } => {
                let marker = self.read.entry(channel_id).or_default();
                marker.last_read = Some(message_id);
                marker.mentions = 0;
                cx.emit(StoreEvent::Unread);
            }
            Event::MemberList(v) => self.apply_member_list(v, cx),
            Event::MembersChunk { guild_id, members, presences } => {
                for (user, status) in presences {
                    self.presences.insert(user, status);
                }
                let mut rows = Vec::new();
                for member in &members {
                    let Some(user) = member.user.clone() else { continue };
                    self.learn_member(guild_id, &user, member);
                    let info = self.member_info.get(&(guild_id, user.id)).cloned().unwrap_or_default();
                    let status = self.status_of(user.id);
                    rows.push(MemberRow::Member { user, nick: info.nick, color: info.color, status });
                }
                rows.sort_by_key(|r| match r {
                    MemberRow::Member { user, nick, .. } => {
                        nick.clone().unwrap_or_else(|| user.display_name().to_string()).to_lowercase()
                    }
                    _ => String::new(),
                });
                rows.insert(0, MemberRow::Group { name: "Members".into(), count: rows.len() as u32 });
                self.member_lists.insert(guild_id, rows);
                cx.emit(StoreEvent::Members(guild_id));
            }
        }
    }

    fn apply_ready(&mut self, ready: gateway::Ready, cx: &mut Context<Self>) {
        // A fresh READY replaces all cached state (e.g. after a failed resume).
        self.guilds.clear();
        self.channels.clear();
        self.dms.clear();
        self.read.clear();
        self.member_lists.clear();
        self.voice.clear();
        self.stages.clear();
        self.calls.clear();
        self.subscribed = None;
        for entry in self.messages.values_mut() {
            *entry = ChannelMessages::default();
        }

        self.gateway_session = ready.session_id.clone();
        self.relationships.clear();
        for (user, kind) in ready.relationships {
            self.relationships.push((user.id, kind));
            self.users.entry(user.id).or_insert(user);
        }
        for (id, muted) in ready.mutes {
            if muted {
                self.muted.insert(id);
            }
        }
        self.forums_loaded.clear();
        self.users.insert(ready.user.id, ready.user.clone());
        self.me = Some(ready.user);
        for (user, status) in ready.presences {
            self.presences.insert(user, status);
        }
        for state in ready.read_states {
            self.read.insert(state.id, ReadMarker { last_read: state.last_message_id, mentions: state.mention_count });
        }
        let mut roles: HashMap<Id, Vec<Id>> = ready.own_roles.into_iter().collect();
        for guild in ready.guilds {
            let own = roles.remove(&guild.id);
            self.add_guild(guild, own);
        }
        // Servers the user has not arranged yet sit at the top, as in Discord.
        let order: Vec<Id> = ready.folders.iter().flat_map(|f| f.guilds.iter().copied()).collect();
        if !order.is_empty() {
            self.guilds.sort_by_key(|g| order.iter().position(|id| *id == g.guild.id).map_or(-1, |p| p as i64));
        }
        self.folders = ready.folders.into_iter().filter(|f| f.id.is_some()).collect();
        self.slash_commands.clear();
        for channel in ready.private_channels {
            for user in &channel.recipients {
                self.users.entry(user.id).or_insert_with(|| user.clone());
            }
            self.dms.push(channel.id);
            self.channels.insert(channel.id, channel);
        }
        self.sort_dms();

        self.connection = Connection::Ready;
        cx.emit(StoreEvent::Connection);
        cx.emit(StoreEvent::Guilds);
        cx.emit(StoreEvent::Channels);

        // Restore the previous view, or fall back to direct messages.
        match self.selected_channel.filter(|c| self.channels.contains_key(c)) {
            Some(channel) => {
                self.selected_channel = None;
                self.select_channel(channel, cx);
            }
            None => {
                self.selected_channel = None;
                let guild = self.selected_guild.filter(|g| self.guild(*g).is_some());
                self.selected_guild = guild;
                self.select_guild(guild, cx);
                cx.emit(StoreEvent::Selection);
            }
        }
    }

    /// Applies a `GUILD_MEMBER_LIST_UPDATE` (user accounts' lazy member list).
    fn apply_member_list(&mut self, v: serde_json::Value, cx: &mut Context<Self>) {
        let Some(guild_id) = json_id(&v["guild_id"]) else { return };
        let group_name = |store: &Store, id: &str| match id {
            "online" => "Online".to_string(),
            "offline" => "Offline".to_string(),
            other => other
                .parse()
                .ok()
                .and_then(|rid| {
                    store.guild(guild_id)?.guild.roles.iter().find(|r| r.id == Id(rid)).map(|r| r.name.clone())
                })
                .unwrap_or_else(|| other.to_string()),
        };
        let mut rows = self.member_lists.remove(&guild_id).unwrap_or_default();

        let parse_item = |store: &mut Store, item: &serde_json::Value| -> MemberRow {
            if let Some(group) = item.get("group") {
                let id = group["id"].as_str().unwrap_or("");
                return MemberRow::Group {
                    name: group_name(store, id),
                    count: group["count"].as_u64().unwrap_or(0) as u32,
                };
            }
            let Some(raw) = item.get("member") else { return MemberRow::Placeholder };
            let Ok(member) = serde_json::from_value::<Member>(raw.clone()) else { return MemberRow::Placeholder };
            let Some(user) = member.user.clone() else { return MemberRow::Placeholder };
            store.learn_member(guild_id, &user, &member);
            let status = Status::parse(raw["presence"]["status"].as_str().unwrap_or("offline"));
            store.presences.insert(user.id, status);
            let color = store.name_color(Some(guild_id), user.id);
            MemberRow::Member { user, nick: member.nick, color, status }
        };

        for op in v["ops"].as_array().into_iter().flatten() {
            let index = op["index"].as_u64().unwrap_or(0) as usize;
            match op["op"].as_str().unwrap_or("") {
                "SYNC" => {
                    let start = op["range"][0].as_u64().unwrap_or(0) as usize;
                    let items = op["items"].as_array().cloned().unwrap_or_default();
                    if rows.len() < start + items.len() {
                        rows.resize(start + items.len(), MemberRow::Placeholder);
                    }
                    for (i, item) in items.iter().enumerate() {
                        rows[start + i] = parse_item(self, item);
                    }
                }
                "INSERT" => {
                    let row = parse_item(self, &op["item"]);
                    rows.insert(index.min(rows.len()), row);
                }
                "UPDATE" => {
                    let row = parse_item(self, &op["item"]);
                    if index < rows.len() {
                        rows[index] = row;
                    }
                }
                "DELETE" => {
                    if index < rows.len() {
                        rows.remove(index);
                    }
                }
                "INVALIDATE" => {
                    let start = op["range"][0].as_u64().unwrap_or(0) as usize;
                    let end = op["range"][1].as_u64().unwrap_or(0) as usize;
                    for row in rows.iter_mut().take(end + 1).skip(start) {
                        *row = MemberRow::Placeholder;
                    }
                }
                _ => {}
            }
        }
        // Group headers carry counts that only the top-level `groups` keeps current.
        if let Some(groups) = v["groups"].as_array() {
            for group in groups {
                let name = group_name(self, group["id"].as_str().unwrap_or(""));
                let count = group["count"].as_u64().unwrap_or(0) as u32;
                for row in rows.iter_mut() {
                    if let MemberRow::Group { name: n, count: c } = row {
                        if *n == name {
                            *c = count;
                        }
                    }
                }
            }
        }
        self.member_lists.insert(guild_id, rows);
        cx.emit(StoreEvent::Members(guild_id));
    }
}

/// Encodes image bytes as the data URI Discord expects for icons and avatars.
pub fn data_uri(mime: &str, bytes: &[u8]) -> String {
    use base64::Engine as _;
    format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes))
}

/// Guesses an image's type from its first bytes.
pub fn image_mime(bytes: &[u8]) -> Option<&'static str> {
    match bytes {
        [0x89, b'P', b'N', b'G', ..] => Some("image/png"),
        [0xFF, 0xD8, 0xFF, ..] => Some("image/jpeg"),
        [b'G', b'I', b'F', b'8', ..] => Some("image/gif"),
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => Some("image/webp"),
        _ => None,
    }
}

fn json_id(v: &serde_json::Value) -> Option<Id> {
    match v {
        serde_json::Value::String(s) => s.parse().ok().map(Id),
        serde_json::Value::Number(n) => n.as_u64().map(Id),
        _ => None,
    }
}
