// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! ozd: the associated machine for a site of CADR Lisp Machines, and
//! the switch of their one Chaosnet subnet over UDP.
//!
//! `docs/design.md` is the design, its §3 these modules, and `docs/protocols.md`
//! what each protocol is.

pub mod address;
pub mod chudp;
pub mod config;
pub mod daemon;
pub mod hosts_text;
pub mod lispm;
pub mod log;
pub mod ncp;
pub mod packet;
pub mod roots;
pub mod service;
