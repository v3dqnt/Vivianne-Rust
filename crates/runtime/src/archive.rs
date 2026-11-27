//! A ZIP file written in memory: stored names are checked, entries are deflated, and the total size is capped. Only
//! what a conversation export needs: no reading, no encryption, no ZIP64 (so no entry or archive reaches 4 GiB).

use std::io::Write as _;

use flate2::{Compression, Crc, write::DeflateEncoder};

/// The most an archive may hold, uncompressed.
pub const MAX_TOTAL: u64 = 512 * 1024 * 1024;
const MAX_ENTRIES: usize = 10_000;
const MAX_NAME: usize = 240;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ArchiveError {
    #[error("`{0}` is not a safe name for an archive entry")]
    Name(String),
    #[error("the archive would pass its size or entry limit")]
    TooBig,
    #[error("an entry is named twice: {0}")]
    Duplicate(String),
}

/// Whether `name` stays inside the archive when unpacked: relative, forward slashes, no `..`, no empty parts, no
/// drive letters or control characters.
#[must_use]
pub fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME
        && !name.starts_with('/')
        && !name
            .chars()
            .any(|c| c.is_control() || matches!(c, '\\' | ':'))
        && name
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

#[derive(Debug, Default)]
pub struct Zip {
    bytes: Vec<u8>,
    central: Vec<u8>,
    names: std::collections::BTreeSet<String>,
    total: u64,
}

fn le16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn le32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

impl Zip {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a file. `time` is seconds since 1970, kept as the file's modified time (clamped to what ZIP can say).
    pub fn add(&mut self, name: &str, data: &[u8], time: u64) -> Result<(), ArchiveError> {
        if !safe_name(name) {
            return Err(ArchiveError::Name(name.to_owned()));
        }
        if !self.names.insert(name.to_owned()) {
            return Err(ArchiveError::Duplicate(name.to_owned()));
        }
        self.total += data.len() as u64;
        if self.total > MAX_TOTAL || self.names.len() > MAX_ENTRIES {
            return Err(ArchiveError::TooBig);
        }
        let mut crc = Crc::new();
        crc.update(data);
        let mut deflate = DeflateEncoder::new(Vec::new(), Compression::default());
        let packed = deflate
            .write_all(data)
            .and_then(|()| deflate.finish())
            .map_err(|_| ArchiveError::TooBig)?;
        let (size, packed_size) = (
            u32::try_from(data.len()).map_err(|_| ArchiveError::TooBig)?,
            u32::try_from(packed.len()).map_err(|_| ArchiveError::TooBig)?,
        );
        let offset = u32::try_from(self.bytes.len()).map_err(|_| ArchiveError::TooBig)?;
        let (date, clock) = dos_time(time);
        // Flag bit 11: the name is UTF-8.
        let flags = 0x0800;
        let name_len = u16::try_from(name.len()).map_err(|_| ArchiveError::TooBig)?;

        let out = &mut self.bytes;
        le32(out, 0x0403_4b50);
        le16(out, 20);
        le16(out, flags);
        le16(out, 8);
        le16(out, clock);
        le16(out, date);
        le32(out, crc.sum());
        le32(out, packed_size);
        le32(out, size);
        le16(out, name_len);
        le16(out, 0);
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(&packed);

        let c = &mut self.central;
        le32(c, 0x0201_4b50);
        le16(c, 20);
        le16(c, 20);
        le16(c, flags);
        le16(c, 8);
        le16(c, clock);
        le16(c, date);
        le32(c, crc.sum());
        le32(c, packed_size);
        le32(c, size);
        le16(c, name_len);
        le16(c, 0);
        le16(c, 0);
        le16(c, 0);
        le16(c, 0);
        le32(c, 0);
        le32(c, offset);
        c.extend_from_slice(name.as_bytes());
        Ok(())
    }

    /// The finished archive.
    pub fn finish(mut self) -> Result<Vec<u8>, ArchiveError> {
        let start = u32::try_from(self.bytes.len()).map_err(|_| ArchiveError::TooBig)?;
        let size = u32::try_from(self.central.len()).map_err(|_| ArchiveError::TooBig)?;
        let count = u16::try_from(self.names.len()).map_err(|_| ArchiveError::TooBig)?;
        self.bytes.extend_from_slice(&self.central);
        let out = &mut self.bytes;
        le32(out, 0x0605_4b50);
        le16(out, 0);
        le16(out, 0);
        le16(out, count);
        le16(out, count);
        le32(out, size);
        le32(out, start);
        le16(out, 0);
        Ok(self.bytes)
    }
}

/// MS-DOS date and time words for `seconds` since 1970 (UTC); 1980 at the earliest.
fn dos_time(seconds: u64) -> (u16, u16) {
    let days = i64::try_from(seconds / 86_400).unwrap_or(0);
    let rest = seconds % 86_400;
    // Civil date from days since 1970 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    if year < 1980 {
        return (0x0021, 0);
    }
    let year = u16::try_from((year - 1980).min(127)).unwrap_or(0);
    let date =
        (year << 9) | (u16::try_from(month).unwrap_or(1) << 5) | u16::try_from(day).unwrap_or(1);
    let clock = (u16::try_from(rest / 3600).unwrap_or(0) << 11)
        | (u16::try_from(rest % 3600 / 60).unwrap_or(0) << 5)
        | u16::try_from(rest % 60 / 2).unwrap_or(0);
    (date, clock)
}

#[cfg(test)]
mod tests {
    use std::io::Read as _;

    use flate2::read::DeflateDecoder;

    use super::*;

    /// Reads one entry back out of the archive by walking its central directory.
    fn read(zip: &[u8], name: &str) -> Option<Vec<u8>> {
        let end = zip.len() - 22;
        let count = u16::from_le_bytes([zip[end + 10], zip[end + 11]]) as usize;
        let mut at = u32::from_le_bytes(zip[end + 16..end + 20].try_into().ok()?) as usize;
        for _ in 0..count {
            let size = u32::from_le_bytes(zip[at + 24..at + 28].try_into().ok()?) as usize;
            let packed = u32::from_le_bytes(zip[at + 20..at + 24].try_into().ok()?) as usize;
            let n = u16::from_le_bytes([zip[at + 28], zip[at + 29]]) as usize;
            let offset = u32::from_le_bytes(zip[at + 42..at + 46].try_into().ok()?) as usize;
            let found = &zip[at + 46..at + 46 + n];
            at += 46 + n;
            if found == name.as_bytes() {
                let local_n = u16::from_le_bytes([zip[offset + 26], zip[offset + 27]]) as usize;
                let extra = u16::from_le_bytes([zip[offset + 28], zip[offset + 29]]) as usize;
                let from = offset + 30 + local_n + extra;
                let mut out = Vec::new();
                DeflateDecoder::new(&zip[from..from + packed])
                    .read_to_end(&mut out)
                    .ok()?;
                assert_eq!(out.len(), size);
                return Some(out);
            }
        }
        None
    }

    #[test]
    fn entries_come_back_whole_by_name() {
        let mut zip = Zip::new();
        zip.add("a.txt", b"hello hello hello", 1_700_000_000)
            .expect("added");
        zip.add("dir/b.bin", &[0, 1, 2, 255], 0).expect("added");
        zip.add("empty", b"", 0).expect("added");
        let bytes = zip.finish().expect("done");
        assert_eq!(&bytes[..4], b"PK\x03\x04");
        assert_eq!(
            read(&bytes, "a.txt").as_deref(),
            Some(&b"hello hello hello"[..])
        );
        assert_eq!(
            read(&bytes, "dir/b.bin").as_deref(),
            Some(&[0_u8, 1, 2, 255][..])
        );
        assert_eq!(read(&bytes, "empty").as_deref(), Some(&b""[..]));
        assert_eq!(read(&bytes, "missing"), None);
    }

    #[test]
    fn a_name_that_could_escape_the_folder_it_is_unpacked_into_is_refused() {
        let mut zip = Zip::new();
        for bad in [
            "",
            "/etc/passwd",
            "../x",
            "a/../b",
            "a//b",
            "a\\b",
            "C:/x",
            "a/./b",
            "a\nb",
            &"x".repeat(241),
        ] {
            assert_eq!(
                zip.add(bad, b"x", 0),
                Err(ArchiveError::Name(bad.to_owned())),
                "{bad}"
            );
        }
        zip.add("ok", b"x", 0).expect("added");
        assert_eq!(
            zip.add("ok", b"y", 0),
            Err(ArchiveError::Duplicate("ok".into()))
        );
    }

    #[test]
    fn the_modified_time_is_a_real_dos_date() {
        // 2023-11-14 22:13:20 UTC
        let (date, clock) = dos_time(1_700_000_000);
        assert_eq!(
            ((date >> 9) + 1980, (date >> 5) & 15, date & 31),
            (2023, 11, 14)
        );
        assert_eq!(
            (clock >> 11, (clock >> 5) & 63, (clock & 31) * 2),
            (22, 13, 20)
        );
        assert_eq!(dos_time(0), (0x0021, 0), "before 1980 is 1980-01-01");
    }
}
