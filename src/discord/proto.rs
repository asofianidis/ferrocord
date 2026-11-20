//! Just enough protobuf decoding to read server folders out of Discord's
//! binary user settings.

use super::model::Id;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Folder {
    /// `None` for a server that sits outside any folder.
    pub id: Option<i64>,
    pub name: Option<String>,
    pub color: Option<u32>,
    pub guilds: Vec<Id>,
}

enum Field<'a> {
    Varint(u64),
    Fixed64(u64),
    Bytes(&'a [u8]),
    Fixed32,
}

struct Reader<'a> {
    buf: &'a [u8],
}

impl<'a> Reader<'a> {
    fn varint(&mut self) -> Option<u64> {
        let mut value = 0u64;
        for shift in (0..64).step_by(7) {
            let (&byte, rest) = self.buf.split_first()?;
            self.buf = rest;
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Some(value);
            }
        }
        None
    }

    fn take(&mut self, len: usize) -> Option<&'a [u8]> {
        if self.buf.len() < len {
            return None;
        }
        let (head, rest) = self.buf.split_at(len);
        self.buf = rest;
        Some(head)
    }

    fn fixed64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
}

impl<'a> Iterator for Reader<'a> {
    type Item = (u64, Field<'a>);

    fn next(&mut self) -> Option<Self::Item> {
        if self.buf.is_empty() {
            return None;
        }
        let key = self.varint()?;
        let field = match key & 7 {
            0 => Field::Varint(self.varint()?),
            1 => Field::Fixed64(self.fixed64()?),
            2 => {
                let len = self.varint()? as usize;
                Field::Bytes(self.take(len)?)
            }
            5 => {
                self.take(4)?;
                Field::Fixed32
            }
            _ => return None,
        };
        Some((key >> 3, field))
    }
}

/// The single varint inside a wrapper message such as `Int64Value`.
fn wrapped_varint(bytes: &[u8]) -> Option<u64> {
    Reader { buf: bytes }.find_map(|(n, f)| match f {
        Field::Varint(v) if n == 1 => Some(v),
        _ => None,
    })
}

fn wrapped_string(bytes: &[u8]) -> Option<String> {
    Reader { buf: bytes }.find_map(|(n, f)| match f {
        Field::Bytes(b) if n == 1 => String::from_utf8(b.to_vec()).ok(),
        _ => None,
    })
}

fn parse_folder(bytes: &[u8]) -> Folder {
    let mut folder = Folder::default();
    for (number, field) in (Reader { buf: bytes }) {
        match (number, field) {
            // `repeated fixed64`, packed or not.
            (1, Field::Bytes(packed)) => {
                folder.guilds.extend(packed.chunks_exact(8).map(|c| Id(u64::from_le_bytes(c.try_into().unwrap()))));
            }
            (1, Field::Fixed64(id)) => folder.guilds.push(Id(id)),
            (2, Field::Bytes(b)) => folder.id = wrapped_varint(b).map(|v| v as i64),
            (3, Field::Bytes(b)) => folder.name = wrapped_string(b).filter(|n| !n.is_empty()),
            (4, Field::Bytes(b)) => folder.color = wrapped_varint(b).map(|v| v as u32),
            _ => {}
        }
    }
    folder
}

/// Reads the folder list from a serialized `PreloadedUserSettings` message.
pub fn guild_folders(settings: &[u8]) -> Vec<Folder> {
    const GUILD_FOLDERS: u64 = 14;
    let mut folders = Vec::new();
    for (number, field) in (Reader { buf: settings }) {
        let (GUILD_FOLDERS, Field::Bytes(section)) = (number, field) else { continue };
        for (inner, value) in (Reader { buf: section }) {
            if let (1, Field::Bytes(bytes)) = (inner, value) {
                folders.push(parse_folder(bytes));
            }
        }
    }
    folders
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(number: u8, wire: u8) -> u8 {
        number << 3 | wire
    }

    fn message(number: u8, body: &[u8]) -> Vec<u8> {
        let mut out = vec![key(number, 2), body.len() as u8];
        out.extend_from_slice(body);
        out
    }

    #[test]
    fn folders_are_read_from_settings() {
        let mut packed = Vec::new();
        packed.extend_from_slice(&200u64.to_le_bytes());
        packed.extend_from_slice(&300u64.to_le_bytes());
        let mut named = message(1, &packed);
        named.extend(message(2, &[key(1, 0), 42]));
        named.extend(message(3, &message(1, b"Hobbies")));
        named.extend(message(4, &[key(1, 0), 0xff, 0x01]));
        let loose = message(1, &100u64.to_le_bytes());

        let mut section = message(1, &loose);
        section.extend(message(1, &named));
        // An unrelated field first, then the folders section.
        let mut settings = vec![key(1, 0), 7];
        settings.extend(message(14, &section));

        let folders = guild_folders(&settings);
        assert_eq!(folders.len(), 2);
        assert_eq!(folders[0], Folder { guilds: vec![Id(100)], ..Default::default() });
        assert_eq!(
            folders[1],
            Folder { id: Some(42), name: Some("Hobbies".into()), color: Some(255), guilds: vec![Id(200), Id(300)] }
        );
    }

    #[test]
    fn truncated_input_does_not_panic() {
        assert!(guild_folders(&[key(14, 2), 50, 1, 2, 3]).is_empty());
        assert!(guild_folders(&[0xff; 12]).is_empty());
    }
}
