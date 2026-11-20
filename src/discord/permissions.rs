//! Permission computation, following Discord's documented algorithm.

use super::model::{Channel, Guild, Id};

pub const VIEW_CHANNEL: u64 = 1 << 10;
pub const SEND_MESSAGES: u64 = 1 << 11;
pub const MANAGE_MESSAGES: u64 = 1 << 13;
pub const ADMINISTRATOR: u64 = 1 << 3;
pub const KICK_MEMBERS: u64 = 1 << 1;
pub const BAN_MEMBERS: u64 = 1 << 2;
pub const MANAGE_CHANNELS: u64 = 1 << 4;
pub const MANAGE_GUILD: u64 = 1 << 5;
pub const CONNECT: u64 = 1 << 20;
pub const SPEAK: u64 = 1 << 21;
pub const MUTE_MEMBERS: u64 = 1 << 22;
pub const DEAFEN_MEMBERS: u64 = 1 << 23;
pub const MOVE_MEMBERS: u64 = 1 << 24;
pub const MANAGE_NICKNAMES: u64 = 1 << 27;
pub const MANAGE_ROLES: u64 = 1 << 28;
pub const MODERATE_MEMBERS: u64 = 1 << 40;
pub const ALL: u64 = u64::MAX;

pub fn base(guild: &Guild, user: Id, member_roles: &[Id]) -> u64 {
    if guild.owner_id == Some(user) {
        return ALL;
    }
    let mut perms = 0;
    for role in &guild.roles {
        // The @everyone role shares the guild's id.
        if role.id == guild.id || member_roles.contains(&role.id) {
            perms |= role.permissions;
        }
    }
    if perms & ADMINISTRATOR != 0 {
        return ALL;
    }
    perms
}

pub fn in_channel(guild: &Guild, channel: &Channel, user: Id, member_roles: &[Id]) -> u64 {
    let mut perms = base(guild, user, member_roles);
    if perms == ALL {
        return ALL;
    }
    let ows = &channel.permission_overwrites;
    if let Some(everyone) = ows.iter().find(|o| o.id == guild.id) {
        perms &= !everyone.deny;
        perms |= everyone.allow;
    }
    let (mut allow, mut deny) = (0, 0);
    for o in ows.iter().filter(|o| o.kind == 0 && o.id != guild.id && member_roles.contains(&o.id)) {
        allow |= o.allow;
        deny |= o.deny;
    }
    perms &= !deny;
    perms |= allow;
    if let Some(o) = ows.iter().find(|o| o.kind == 1 && o.id == user) {
        perms &= !o.deny;
        perms |= o.allow;
    }
    perms
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discord::model::{Overwrite, Role};

    fn guild() -> Guild {
        Guild {
            id: Id(1),
            owner_id: Some(Id(99)),
            roles: vec![
                Role { id: Id(1), permissions: VIEW_CHANNEL | SEND_MESSAGES, ..Default::default() },
                Role { id: Id(2), permissions: MANAGE_MESSAGES, ..Default::default() },
                Role { id: Id(3), permissions: ADMINISTRATOR, ..Default::default() },
            ],
            ..Default::default()
        }
    }

    #[test]
    fn everyone_deny_hides_channel_unless_role_allows() {
        let guild = guild();
        let channel = Channel {
            id: Id(5),
            permission_overwrites: vec![
                Overwrite { id: Id(1), kind: 0, allow: 0, deny: VIEW_CHANNEL },
                Overwrite { id: Id(2), kind: 0, allow: VIEW_CHANNEL, deny: 0 },
            ],
            ..Default::default()
        };
        assert_eq!(in_channel(&guild, &channel, Id(7), &[]) & VIEW_CHANNEL, 0);
        assert_ne!(in_channel(&guild, &channel, Id(7), &[Id(2)]) & VIEW_CHANNEL, 0);
    }

    #[test]
    fn member_overwrite_wins_over_roles() {
        let guild = guild();
        let channel = Channel {
            id: Id(5),
            permission_overwrites: vec![Overwrite { id: Id(7), kind: 1, allow: 0, deny: SEND_MESSAGES }],
            ..Default::default()
        };
        assert_eq!(in_channel(&guild, &channel, Id(7), &[]) & SEND_MESSAGES, 0);
        assert_ne!(in_channel(&guild, &channel, Id(8), &[]) & SEND_MESSAGES, 0);
    }

    #[test]
    fn owner_and_administrator_bypass_overwrites() {
        let guild = guild();
        let channel = Channel {
            id: Id(5),
            permission_overwrites: vec![Overwrite { id: Id(1), kind: 0, allow: 0, deny: VIEW_CHANNEL }],
            ..Default::default()
        };
        assert_eq!(in_channel(&guild, &channel, Id(99), &[]), ALL);
        assert_eq!(in_channel(&guild, &channel, Id(7), &[Id(3)]), ALL);
    }
}
