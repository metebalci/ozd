// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A Chaosnet address, as the config writes it.

/// Reads an address as the config takes it: the
/// sixteen bits in octal, `3050`, or as `subnet:host` with each half in
/// octal, `6:50`. The two are one number --- the subnet is the high byte
/// and the host the low (AIM-628) --- which octal digits do not show,
/// three bits to a digit against eight to a byte; that is why `3050` reads
/// as "subnet 6, host 50" only once split. Each half must fit its byte.
///
/// **Neither half may be zero, because a zero host is not a host.** MIT's
/// own description of the interface, AIM-628 §7: the destination word is
/// "the cable address of the destination of the packet, or 0 to broadcast
/// it", and a receiver
/// stores the next packet "addressed to this node, or is broadcast". So
/// `6:0` names every host on subnet 6 rather than one of them, and a host
/// configured as it would take the whole subnet's traffic for its own. A
/// zero subnet is refused for the same reason, which also refuses every
/// bare octal below `400`.
///
/// Anything else is `None`.
pub fn parse_address(s: &str) -> Option<u16> {
    let byte = |s: &str| u16::from_str_radix(s, 8).ok().filter(|&b| b <= 0o377);
    let both = |a: u16| (a >> 8 != 0 && a & 0o377 != 0).then_some(a);
    match s.split_once(':') {
        Some((subnet, host)) => both(byte(subnet)? << 8 | byte(host)?),
        None => u16::from_str_radix(s, 8).ok().and_then(both),
    }
}
