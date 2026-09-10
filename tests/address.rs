// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A Chaosnet address, as the config writes it: `parse_address`.

/// **An address is read either way it is written.** A Chaosnet address is
/// sixteen bits, the subnet in the high byte and the host in the low, and
/// the memo and the host tables write the whole number in octal --- where
/// the byte boundary falls inside a digit, so `3050` hides "subnet 6, host
/// 50". The config takes both spellings, each half in octal too.
#[test]
fn an_address_is_read_either_way_it_is_written() {
    use ozd::address::parse_address;
    assert_eq!(parse_address("3050"), Some(0o3050));
    assert_eq!(parse_address("6:50"), Some(0o3050), "subnet 6, host 50");
    assert_eq!(parse_address("6:60"), Some(0o3060), "and the server beside it");
    assert_eq!(parse_address("23:6"), Some(0o11406), "MIT's own OZ");
    // Subnet 376 is Chaosnet's private range, the 192.168 of it: the
    // Global Chaosnet's own documentation says "no routing information
    // about that subnet should be sent outside that subnet, no packets
    // from that subnet should be sent to other subnets, and any packets
    // received from that subnet on another subnet should be dropped". A
    // local bridge and the emulators behind it live in 177001..177377.
    assert_eq!(parse_address("376:41"), Some(0o177041), "the private subnet");
    assert_eq!(parse_address("177001"), Some(0o177001), "its first host");
    assert_eq!(parse_address("177377"), Some(0o177377), "and its last");
    // Neither half may be zero: a zero host is the subnet's broadcast, not
    // a host, and MIT's interface takes the destination word "or 0 to
    // broadcast it". A machine configured as one would take the whole
    // subnet's traffic for its own.
    assert_eq!(parse_address("0:0"), None, "not a host");
    assert_eq!(parse_address("6:0"), None, "a zero host is subnet 6's broadcast");
    assert_eq!(parse_address("0:50"), None, "and a zero subnet is no subnet");
    assert_eq!(parse_address("0"), None, "the bare form too");
    assert_eq!(parse_address("377"), None, "which is every bare octal below 400");
    assert_eq!(parse_address("26000"), None, "subnet 54's broadcast");
    assert_eq!(parse_address("26001"), Some(0o26001), "and its first host");
    assert_eq!(parse_address("26377"), Some(0o26377), "and its last");
    assert_eq!(parse_address("377:377"), Some(0o177777));
    assert_eq!(parse_address("400:1"), None, "a subnet is eight bits");
    assert_eq!(parse_address("6:400"), None, "and so is a host");
    assert_eq!(parse_address("6:"), None);
    assert_eq!(parse_address(":50"), None);
    assert_eq!(parse_address("6:50:1"), None);
    assert_eq!(parse_address("3058"), None, "octal");
    assert_eq!(parse_address("6:58"), None, "in both halves");
    assert_eq!(parse_address("200000"), None, "sixteen bits");
    assert_eq!(parse_address(""), None);
}
