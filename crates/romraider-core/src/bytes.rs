use crate::error::{CoreError, CoreResult};

pub fn slice<'a>(buf: &'a [u8], offset: usize, len: usize) -> CoreResult<&'a [u8]> {
    let end = offset
        .checked_add(len)
        .ok_or(CoreError::OutOfBounds { offset, len, buf_len: buf.len() })?;
    buf.get(offset..end)
        .ok_or(CoreError::OutOfBounds { offset, len, buf_len: buf.len() })
}

pub fn slice_mut<'a>(buf: &'a mut [u8], offset: usize, len: usize) -> CoreResult<&'a mut [u8]> {
    let buf_len = buf.len();
    let end = offset
        .checked_add(len)
        .ok_or(CoreError::OutOfBounds { offset, len, buf_len })?;
    buf.get_mut(offset..end)
        .ok_or(CoreError::OutOfBounds { offset, len, buf_len })
}

/// Печатает байты в виде `DE AD BE EF` — формат, привычный по старому RomRaider.
#[must_use]
pub fn hex_dump(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(&format!("{b:02X}"));
    }
    out
}
