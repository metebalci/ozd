// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The Lisp Machine character set as the protocols carry it: the
//! machine's newline, its text against Unix's, and protocol text a byte a
//! character.
//!
//! From muir's `src/chaos/file.rs`, where FILE alone used them; a module
//! of their own here, since HOSTAB and NAME speak the character set too
//! (`DESIGN.md` §3).

/// The Lisp Machine's newline, `#/NEWLINE`, which is what separates the
/// lines of a command and a reply: `CHNL` in `FILE.c`, `0200|'\r'`.
pub const NEWLINE: u8 = 0o215;

/// Lisp Machine text back to Unix, `FILE.c`'s `from_lispm`: the inverse
/// of [`to_lispm`], and the note beside it in the C is worth keeping ---
/// "0212 maps to 015 since 0215 must map to 012".
pub fn from_lispm(bytes: &[u8]) -> Vec<u8> {
    bytes
        .iter()
        .map(|&c| match c {
            0o10 | 0o11 | 0o12 | 0o14 | 0o15 | 0o177 => c | 0o200,
            0o212 => 0o15,
            NEWLINE => b'\n',
            0o210 | 0o211 | 0o214 | 0o377 => c & 0o177,
            _ => c,
        })
        .collect()
}

/// Unix text to the Lisp Machine character set, `FILE.c`'s `to_lispm`:
/// newline to the Lisp Machine's, the format effectors 010 to 015 and
/// 0177 up into the 0200s where the Lisp Machine keeps them, and the
/// bytes 0210 to 0215 and 0377 --- a Lisp Machine file's format
/// effectors as Unix stored them --- back down.
pub fn to_lispm(bytes: &[u8]) -> Vec<u8> {
    bytes
        .iter()
        .map(|&c| match c {
            0o210 | 0o211 | 0o212 | 0o214 | 0o215 | 0o377 => c & 0o177,
            b'\n' => NEWLINE,
            0o15 => 0o212,
            0o10 | 0o11 | 0o14 | 0o177 => c | 0o200,
            _ => c,
        })
        .collect()
}

/// Protocol text as bytes: each character is one byte, so the Lisp
/// Machine's newline at 0o215 survives.
pub fn lispm_text(s: &str) -> Vec<u8> {
    s.chars().map(|c| if (c as u32) < 256 { c as u32 as u8 } else { b'?' }).collect()
}

/// The bytes of a command as one character each, the inverse of
/// [`lispm_text`].
///
/// **Not UTF-8.** The protocol's own newline is 0o215, which is a
/// continuation byte in UTF-8, so reading a command with
/// `String::from_utf8_lossy` replaces it and every line after the first
/// is lost --- an `OPEN` then reads its pathname as empty and answers
/// about the root directory. `the_file_service_serves_files_and_directories`
/// in muir's `tests/chaos.rs` is the regression there, and
/// `protocol_text_is_a_byte_a_character` in `tests/services.rs` here.
pub fn from_bytes(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| b as char).collect()
}
