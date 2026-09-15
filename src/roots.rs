// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The tree FILE serves: its roots, how a pathname is resolved in it, and
//! which roots are read-only (`docs/design.md` §6).
//!
//! **A tree is a base root and named roots mounted at its top level**, each
//! read-only or not, `,ro` (`docs/design.md` §6, "Roots"). A
//! pathname whose first component is exactly a mount's name is in that
//! mount, the rest of it under the mount's directory; any other pathname is
//! in the base. With mounts and no base, `/` names the mounts and nothing
//! else, and is read-only. One root without a name is a single root.
//!
//! **Nothing outside a root is ever named by a [`Place`]**, and so nothing
//! outside one is served. That is the whole of FILE's security, since FILE
//! answers anyone who reaches it (`docs/design.md` §6). [`Tree::resolve`]
//! splits a pathname on `/`, refuses `.` and `..` rather than normalising
//! them, puts an absolute pathname under the root, and canonicalises the
//! deepest part that exists. Beyond that:
//!
//! - **No second tree by symlink.** A link at a root's top level does not
//!   bring its target into the tree, which would serve a file outside the
//!   root to anyone who names the link (`docs/design.md` §6). A link anywhere is
//!   followed and must lead into its own root. A directory that lives
//!   elsewhere is served by mounting it.
//! - **What comes back is what was checked**: the canonical path, with the
//!   part that does not exist yet joined on, never the client's string.
//! - **A link that does not resolve is refused**, not walked up past to the
//!   nearest ancestor that canonicalises: a link to a file that does not
//!   exist yet, written through, makes that file wherever the link points,
//!   and that is outside as easily as in.
//! - **Roots do not overlap**, which startup refuses. A read-only mount
//!   inside the base would be written through the base: a client makes a
//!   link in the base to one of the mount's files with `CREATE-LINK` ---
//!   whose target resolves, the mount being a root --- and opens the link
//!   for writing, and the file it reaches lies under the base.
//! - **The service's temporaries cannot be named**, in any case. A client
//!   that could name one could delete it mid-write and put a link in its
//!   place, and the startup's cleanup would remove what the daemon did not
//!   make. A filesystem that folds case takes `#OZD-...#` for one, so a
//!   pathname is refused a temporary's name in any ASCII case; the cleanup
//!   still matches exactly.
//!
//! **Why checking a path and then opening it is safe here.** The path
//! checked is the path opened, and it contains no link when it is checked:
//! every component that exists was just canonicalised, and every one that
//! does not was just found absent. Nothing can change that before FILE
//! opens it, because nothing else runs --- the daemon is one thread, its
//! loop the only thing in it (`docs/design.md` §1), and it is the only writer of
//! a writable root, while a read-only root is written by nobody
//! (`docs/design.md` §6). **Both are conditions, not checks**:
//! a second thread, or another writer in a writable root, makes the gap
//! between check and open real, and the answer then is `openat` with
//! `O_NOFOLLOW`, which std does not offer (`docs/design.md` §2).
//!
//! **What no path check sees.** A hard link inside a root to a file outside
//! it canonicalises to a path under the root, and is out of scope; so is a
//! bind mount that shows one root's directory inside another, which
//! canonicalises the same way. The mitigation for both is operational: the
//! roots belong to the daemon's own unprivileged user and nobody else
//! writes into them, and a read-only root is read-only on disk as well
//! (the README, "Security").

use std::collections::BTreeSet;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// A refusal as FILE answers it: the error code and its message.
pub type Refusal = (&'static str, String);

/// The refusal of a pathname that may not be reached: `ATD`, which the band
/// turns into `INCORRECT-ACCESS-TO-DIRECTORY` (`sys/io/file/open.lisp:231`),
/// with a message of this host's own (`docs/protocols.md`, FILE).
fn denied() -> Refusal {
    ("ATD", "Access to directory denied".into())
}

/// The refusal of a write that is not allowed: `ATF`, "Access to file
/// denied", which the band turns into `INCORRECT-ACCESS-TO-FILE`
/// (`sys/io/file/open.lisp:224`). FILE has no code of its own for a refused
/// write (`docs/protocols.md`, FILE).
fn refused() -> Refusal {
    ("ATF", "Access to file denied".into())
}

/// A name at `/` that is not there, in a tree with no base: `FNF`, which
/// the band turns into `FILE-NOT-FOUND` (`sys/io/file/open.lisp:180`).
fn not_found() -> Refusal {
    ("FNF", "File not found".into())
}

/// A root as the config names it (`crate::config::Root`): a mount's name,
/// or none for the base; its absolute path; whether it is read-only.
pub use crate::config::Root;

/// The roots, checked. Made once at startup by [`Tree::new`], and read
/// only after.
#[derive(Clone, Debug)]
pub struct Tree {
    roots: Vec<Root>,
    warnings: Vec<String>,
}

/// What a pathname names, for reading.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resolved {
    /// `/` itself. Its listing is [`Tree::list_top`], not the base
    /// directory's own: that would miss the mounts, and show what they
    /// cover. It is never written.
    Top,
    /// Something in one root.
    Place(Place),
}

/// A path in one root: the only thing FILE opens (`docs/design.md` §6, step 4).
///
/// **Good for the command that resolved it, and no longer.** The next
/// command may change the tree --- a client's RENAME and CREATE-LINK are
/// commands like any other --- so a place is not kept from one command to
/// the next; a later command resolves its pathname again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    /// What to open: the canonical path of the part of the pathname that
    /// exists, with the part that does not exist joined on. It contained
    /// no link when it was resolved.
    pub path: PathBuf,
    /// The root it is in: a mount's name, or `None` for the base.
    pub root: Option<String>,
    /// Whether that root is read-only.
    pub readonly: bool,
}

/// A name in a directory of one root, for what acts on the name itself and
/// not on what it leads to (`docs/design.md` §6, "FILE's rules"): DELETE and
/// RENAME, which remove and move a link and never its target, as Unix
/// does; and DIRECTORY, which describes by it a name that the
/// tree will not follow, so that a link that leads nowhere is still listed,
/// and can be deleted.
///
/// **Never opened.** Its directory is a [`Place`] --- canonical, in its
/// root, no link in it when resolved --- and its name is the last component
/// as that directory holds it, which may be a link to anywhere, or to
/// nothing. What uses an entry acts on the name itself: `symlink_metadata`,
/// `remove_file`, `remove_dir` and `rename`, none of which follows a link
/// that is a path's last component. Good for the command that resolved it,
/// and no longer, as a place is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// The directory the name is in, resolved as [`Tree::resolve`] resolves
    /// a pathname: what exists of it canonical, the rest joined on.
    pub directory: Place,
    /// The name: one component, not `.`, `..` or a temporary's.
    pub name: String,
}

impl Entry {
    /// The entry's path: its directory's, with its name joined on.
    pub fn path(&self) -> PathBuf {
        self.directory.path.join(&self.name)
    }
}

/// What [`Tree::find`] found, before reading or writing decides what it
/// means.
enum Found<'a> {
    Top,
    /// In a tree with no base, a name at `/` that is no mount's.
    Nowhere,
    In(&'a Root, Place),
}

/// What [`Tree::pick`] found: steps 1 and 2 of a resolution, before the
/// disk is looked at.
enum Picked<'a, 'p> {
    Top,
    /// In a tree with no base, a name at `/` that is no mount's.
    Nowhere,
    /// A root, and the components under it.
    In(&'a Root, Vec<&'p str>),
}

impl Tree {
    /// The tree of these roots, if every one can be served safely, or why
    /// not (`docs/design.md` §6, "At startup"):
    ///
    /// - there is at least one root, at most one base, and no mount's name
    ///   twice;
    /// - a mount's name is one component of a pathname --- not empty, no
    ///   `/`, not `.` or `..`, and not a temporary's (see [`is_temporary`])
    ///   --- or no pathname could reach it;
    /// - each root is an absolute path, canonicalises, is a directory, and
    ///   is not `/`, read-only or not;
    /// - no root is another, or lies in another, since a link in the outer
    ///   would reach the inner's files under the outer's rules;
    /// - each writable root can be written: a probe file is made in it and
    ///   removed. It is named as a temporary, of client 0, which is no
    ///   host's address, so one a crash leaves is removed at the next start
    ///   like any other. A read-only root is never written, so never
    ///   probed.
    ///
    /// Nothing is written until every other check has passed, so a tree
    /// that does not start has touched nothing. The base's entries that a
    /// mount covers are not refused but warned about ([`Tree::warnings`]).
    pub fn new(roots: Vec<Root>) -> Result<Tree, String> {
        if roots.is_empty() {
            return Err("no root: FILE serves nothing it is not given".into());
        }
        let bases: Vec<&Root> = roots.iter().filter(|r| r.name.is_none()).collect();
        if let [first, second, ..] = bases[..] {
            return Err(format!(
                "more than one base: {} and {}",
                describe(first),
                describe(second)
            ));
        }
        for (i, r) in roots.iter().enumerate() {
            if r.name.is_some() && roots[..i].iter().any(|q| q.name == r.name) {
                return Err(format!("{}: named twice", describe(r)));
            }
        }
        let roots = roots.into_iter().map(checked).collect::<Result<Vec<Root>, String>>()?;
        for (i, a) in roots.iter().enumerate() {
            for b in &roots[i + 1..] {
                if a.path.starts_with(&b.path) || b.path.starts_with(&a.path) {
                    return Err(format!(
                        "{}: overlaps {}, and no root may be another or lie in one",
                        describe(a),
                        describe(b)
                    ));
                }
            }
        }
        for r in roots.iter().filter(|r| !r.readonly) {
            probe(r)?;
        }
        let warnings = covered(&roots);
        Ok(Tree { roots, warnings })
    }

    /// What startup has to say that is not a refusal: each entry of the
    /// base that a mount covers, by its path. It exists and no pathname
    /// reaches it, as a Unix mount point covers the directory under it
    /// (`docs/design.md` §6).
    ///
    /// On a filesystem that folds case, as macOS's usually does, such an
    /// entry may still be reached under another case --- `/TREE` in the
    /// base beside a mount `tree` --- though only as the base's own, under
    /// the base's rules; **unverified**, not run there.
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    fn base(&self) -> Option<&Root> {
        self.roots.iter().find(|r| r.name.is_none())
    }

    /// What a pathname names, for reading (`docs/design.md` §6, `resolve`):
    ///
    /// 1. Split on `/`, empty components dropped. A `.` or a `..` is
    ///    refused, `ATD`, not normalised; so is a temporary's name, in any
    ///    ASCII case (see [`is_temporary`]). No components at all is
    ///    [`Resolved::Top`].
    /// 2. The first component is a mount if it is exactly a mount's name,
    ///    case and all --- FILE folds no case in a pathname, and a
    ///    band sends its pathnames in lower case (`docs/design.md` §6) --- and
    ///    the rest is under the mount. Otherwise the whole pathname is under
    ///    the base; with no base it names nothing, `FNF`.
    /// 3. The deepest part that exists as an entry --- a link counts,
    ///    whether or not it leads anywhere --- is canonicalised, every link
    ///    on the way followed. If it does not canonicalise, being a link
    ///    that leads nowhere or round in a circle, it is refused, `ATD`; so
    ///    is a result that is not the root or under it, which is what a link
    ///    out of its own root comes to. Since roots do not overlap, "under
    ///    the root" is "in no other root" as well.
    /// 4. The [`Place`] is that canonical path with the rest joined on. So a
    ///    link is resolved wherever it is, the last component included.
    ///    DELETE and RENAME act on a link itself, not on what it leads to,
    ///    and resolve their pathnames as entries instead
    ///    ([`Tree::resolve_entry_for_writing`]).
    ///
    /// **FILE opens the path returned, and that is safe only because nothing
    /// runs between this check and the open**: one thread, this process the
    /// only writer of a writable root, and a read-only root written by
    /// nobody (`docs/design.md` §6; the module documentation).
    pub fn resolve(&self, pathname: &str) -> Result<Resolved, Refusal> {
        match self.find(pathname)? {
            Found::Top => Ok(Resolved::Top),
            Found::Nowhere => Err(not_found()),
            Found::In(_, place) => Ok(Resolved::Place(place)),
        }
    }

    /// What a pathname names, for writing: what [`Tree::resolve`] allows,
    /// less a root itself --- FILE may not delete, rename, replace or make
    /// one --- and less everything in a read-only root. Every command
    /// that writes resolves its pathnames here, or as an entry with the
    /// same refusals: OPEN for output, CREATE-DIRECTORY, CREATE-LINK's link
    /// and CHANGE-PROPERTIES here; DELETE and RENAME, which act on a link
    /// itself, through [`Tree::resolve_entry_for_writing`] (`docs/design.md` §6,
    /// "A read-only root" and "FILE's rules").
    ///
    /// Containment is decided first, so a pathname that leaves its root is
    /// `ATD` whatever root it starts in. Then a read-only root refuses with
    /// `ATF`, "Access to file denied", before anything is touched; so does
    /// `/` with no base, which is read-only. Then a root itself is refused
    /// with `ATD`; so nothing can be made at `/` under a mount's name, since
    /// that pathname is the mount itself.
    ///
    /// The place's directory is the root or lies in it, so a write's
    /// temporary made there --- beside the file, as FILE makes it ---
    /// is in the root too (see [`temporary_name`]).
    pub fn resolve_for_writing(&self, pathname: &str) -> Result<Place, Refusal> {
        match self.find(pathname)? {
            Found::Top => Err(self.top_for_writing()),
            Found::Nowhere => Err(refused()),
            Found::In(_, place) if place.readonly => Err(refused()),
            Found::In(root, place) if place.path == root.path => Err(denied()),
            Found::In(_, place) => Ok(place),
        }
    }

    /// Both ends of a RENAME, each resolved for writing, the old first. Two
    /// ends in different roots are refused with `ATF`: that would be a copy
    /// and not a rename, carrying a file across the line between two roots'
    /// rules (`docs/design.md` §6). FILE's RENAME, which moves a link itself,
    /// resolves its ends as entries, by the same rule
    /// ([`Tree::resolve_entries_for_renaming`]).
    pub fn resolve_for_renaming(&self, from: &str, to: &str) -> Result<(Place, Place), Refusal> {
        let from = self.resolve_for_writing(from)?;
        let to = self.resolve_for_writing(to)?;
        if from.root != to.root {
            return Err(refused());
        }
        Ok((from, to))
    }

    /// What a pathname names as an entry, for reading: its directory
    /// resolved as [`Tree::resolve`] resolves a pathname, and its last
    /// component joined on as the directory holds it, not followed
    /// ([`Entry`]). DIRECTORY describes by it a name that the tree will not
    /// follow --- a link out of its root, into another, or nowhere --- as
    /// the link itself, never as what it leads to.
    ///
    /// Refused as `resolve` refuses: a `.`, a `..` or a temporary's name
    /// anywhere, `ATD`; a directory on the way that leaves its root, leads
    /// nowhere or loops, `ATD`; in a tree with no base, a name at `/` that
    /// is no mount's, `FNF`. `/` and a root itself are no name in a
    /// directory, `ATD`. The last component is refused for nothing it
    /// leads to, since nothing is followed there.
    pub fn resolve_entry(&self, pathname: &str) -> Result<Entry, Refusal> {
        match self.pick(pathname)? {
            Picked::Top => Err(denied()),
            Picked::Nowhere => Err(not_found()),
            Picked::In(root, rest) => {
                let Some((name, above)) = rest.split_last() else {
                    return Err(denied());
                };
                Ok(Entry { directory: root.locate(above)?, name: (*name).to_string() })
            }
        }
    }

    /// What a pathname names as an entry, for writing:
    /// [`Tree::resolve_entry`], refused as [`Tree::resolve_for_writing`]
    /// refuses and in the same order. Containment first, `ATD`; then a
    /// read-only root, and `/` with no base or a read-only one, `ATF`,
    /// before anything is touched; then `/` and a root itself, `ATD`.
    ///
    /// DELETE resolves its pathname here, and RENAME both of its (through
    /// [`Tree::resolve_entries_for_renaming`]): both act on a link itself,
    /// its directory resolved and its own name not followed, as Unix does
    /// (`docs/design.md` §6, "FILE's rules"). So a link that
    /// leads nowhere, which no read or write will touch, can still be
    /// deleted. The entry's directory is the root or lies in it, and its
    /// name, a link or not, is a name in that directory: removing or
    /// renaming it changes that directory, and nothing a link leads to.
    pub fn resolve_entry_for_writing(&self, pathname: &str) -> Result<Entry, Refusal> {
        match self.pick(pathname)? {
            Picked::Top => Err(self.top_for_writing()),
            Picked::Nowhere => Err(refused()),
            Picked::In(root, rest) => {
                let Some((name, above)) = rest.split_last() else {
                    // The root itself, refused as `resolve_for_writing`
                    // refuses it once it has found it.
                    root.locate(&[])?;
                    return Err(if root.readonly { refused() } else { denied() });
                };
                let directory = root.locate(above)?;
                if directory.readonly {
                    return Err(refused());
                }
                Ok(Entry { directory, name: (*name).to_string() })
            }
        }
    }

    /// Both ends of a RENAME, each resolved as an entry for writing, the
    /// old first; two ends in different roots are refused with `ATF`, as
    /// [`Tree::resolve_for_renaming`] refuses them. A link at either end is
    /// the link itself: the old one is moved, whatever it leads to, and a
    /// new name that is a link is there already, whether or not it leads
    /// anywhere (`docs/design.md` §6, "FILE's rules").
    pub fn resolve_entries_for_renaming(
        &self,
        from: &str,
        to: &str,
    ) -> Result<(Entry, Entry), Refusal> {
        let from = self.resolve_entry_for_writing(from)?;
        let to = self.resolve_entry_for_writing(to)?;
        if from.directory.root != to.directory.root {
            return Err(refused());
        }
        Ok((from, to))
    }

    /// `/` for writing: `ATD` over a writable base, as a root itself is;
    /// `ATF` with no base or a read-only one, since `/` is then read-only.
    fn top_for_writing(&self) -> Refusal {
        match self.base() {
            Some(b) if !b.readonly => denied(),
            _ => refused(),
        }
    }

    /// Steps 1 and 2 of [`Tree::resolve`]: the components, checked, and
    /// the root they are in, with the components under it.
    fn pick<'p>(&self, pathname: &'p str) -> Result<Picked<'_, 'p>, Refusal> {
        let parts: Vec<&str> = pathname.split('/').filter(|c| !c.is_empty()).collect();
        if parts.iter().any(|c| *c == "." || *c == ".." || names_a_temporary(c)) {
            return Err(denied());
        }
        let Some(&first) = parts.first() else {
            return Ok(Picked::Top);
        };
        Ok(match self.roots.iter().find(|r| r.name.as_deref() == Some(first)) {
            Some(mount) => Picked::In(mount, parts[1..].to_vec()),
            None => match self.base() {
                Some(base) => Picked::In(base, parts),
                None => Picked::Nowhere,
            },
        })
    }

    /// Steps 1 to 3 of [`Tree::resolve`].
    fn find(&self, pathname: &str) -> Result<Found<'_>, Refusal> {
        Ok(match self.pick(pathname)? {
            Picked::Top => Found::Top,
            Picked::Nowhere => Found::Nowhere,
            Picked::In(root, rest) => Found::In(root, root.locate(&rest)?),
        })
    }

    /// The names at `/`: the base's entries and the mounts' names, each
    /// once, a mount's name standing for the base's entry of that name; with
    /// no base, the mounts' names alone (`docs/design.md` §6). Sorted.
    ///
    /// These are names, and nothing is known about them yet: a base entry
    /// may be a link out of the root. Anything about one --- its length,
    /// its date, whether it is a directory --- is read from what
    /// `/<name>` resolves to, never from the entry itself.
    pub fn list_top(&self) -> std::io::Result<Vec<String>> {
        let mut names = BTreeSet::new();
        if let Some(base) = self.base() {
            for e in std::fs::read_dir(&base.path)? {
                names.insert(e?.file_name().to_string_lossy().into_owned());
            }
        }
        names.extend(self.roots.iter().filter_map(|r| r.name.clone()));
        Ok(names.into_iter().collect())
    }

    /// Removes the stale temporaries of every writable root, at startup: a
    /// daemon killed in the middle of a write leaves its temporary behind
    /// (`docs/design.md` §6, §10).
    ///
    /// A temporary is a regular file whose name [`is_temporary`], in any
    /// directory of a writable root, since FILE makes it beside the file it
    /// writes. Nothing else is touched: not a directory or a link with such
    /// a name, since FILE makes neither; not what a link leads to, since the
    /// walk follows none; not a read-only root, which is never written.
    /// That the name is FILE's alone is [`Tree::resolve`]'s doing: no
    /// client can name a temporary, so none can make a file of its own that
    /// looks like one.
    ///
    /// Each thing done, for the log: a temporary removed, or what could not
    /// be removed or read, and why.
    pub fn remove_temporaries(&self) -> Vec<Result<PathBuf, String>> {
        let mut done = Vec::new();
        for root in self.roots.iter().filter(|r| !r.readonly) {
            let mut directories = vec![root.path.clone()];
            while let Some(dir) = directories.pop() {
                let entries = match std::fs::read_dir(&dir) {
                    Ok(entries) => entries,
                    Err(e) => {
                        done.push(Err(format!("{}: {e}", dir.display())));
                        continue;
                    }
                };
                for e in entries.flatten() {
                    // `file_type` does not follow a link.
                    let Ok(kind) = e.file_type() else { continue };
                    if kind.is_dir() {
                        directories.push(e.path());
                    } else if kind.is_file() && e.file_name().to_str().is_some_and(is_temporary) {
                        let p = e.path();
                        done.push(match std::fs::remove_file(&p) {
                            Ok(()) => Ok(p),
                            Err(e) => Err(format!("{}: {e}", p.display())),
                        });
                    }
                }
            }
        }
        done
    }
}

impl Root {
    /// Steps 3 and 4 of [`Tree::resolve`], for the components `parts`
    /// under this root: none of them empty, `.`, `..`, or with a `/` in it.
    fn locate(&self, parts: &[&str]) -> Result<Place, Refusal> {
        // The deepest part that is there as an entry. `symlink_metadata`
        // sees a link itself, so one that leads nowhere is found here and
        // refused below, where a walk up past everything that does not
        // canonicalise would join it on to the path returned. Absent is
        // "not found" or "not a directory"; any other failure --- a
        // directory that may not be searched, a link that loops --- is a
        // refusal.
        let mut at = parts.iter().fold(self.path.clone(), |p, c| p.join(c));
        let mut there = parts.len();
        loop {
            match std::fs::symlink_metadata(&at) {
                Ok(_) => break,
                Err(e)
                    if there > 0
                        && matches!(e.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) =>
                {
                    at.pop();
                    there -= 1;
                }
                Err(_) => return Err(denied()),
            }
        }
        let real = std::fs::canonicalize(&at).map_err(|_| denied())?;
        if !real.starts_with(&self.path) {
            return Err(denied());
        }
        let path = parts[there..].iter().fold(real, |p, c| p.join(c));
        Ok(Place { path, root: self.name.clone(), readonly: self.readonly })
    }
}

impl Place {
    /// What is at the place, if FILE may open it: a regular file or a
    /// directory, or `None` if nothing is there. Anything else --- a FIFO,
    /// a device, a socket --- is refused with `ATD` (`docs/design.md` §6,
    /// "Opening"): the loop is one thread, and opening a FIFO that has no
    /// writer blocks it, and every client with it. So is a
    /// link, which a place cannot hold unless the tree changed since it was
    /// resolved.
    ///
    /// Read with `symlink_metadata`, which opens nothing and follows
    /// nothing. FILE tells the case apart and answers it with `WKF`, the
    /// band's `WRONG-KIND-OF-FILE` (`openable`, in `crate::service::file`).
    pub fn metadata(&self) -> Result<Option<std::fs::Metadata>, Refusal> {
        match std::fs::symlink_metadata(&self.path) {
            Ok(m) if m.is_file() || m.is_dir() => Ok(Some(m)),
            Ok(_) => Err(("ATD", "Not a regular file or a directory".into())),
            Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => {
                Ok(None)
            }
            Err(_) => Err(denied()),
        }
    }
}

/// How FILE's temporaries are named: `#ozd-`, the client's address in
/// decimal, `-`, a count, `#`.
const TEMPORARY_PREFIX: &str = "#ozd-";
const TEMPORARY_SUFFIX: &str = "#";

/// The count in a temporary's name, taken once per name for the whole
/// process and shared across control connections: a count kept per
/// connection would let two connections of one client make the same name,
/// the second truncating the first's temporary.
static NEXT_TEMPORARY: AtomicU64 = AtomicU64::new(0);

/// A new name for a write's temporary, for the host at `client`, never
/// given before in this process: `#ozd-1576-0#` for 3050, the first time.
///
/// FILE writes into a temporary and renames it over the file on CLOSE.
/// What keeps that in the root:
///
/// - The temporary is made in the directory of the place
///   [`Tree::resolve_for_writing`] returned, beside the file: never a root
///   itself, so that directory is the root or in it.
/// - It is made once, in the command that resolved that place, as a new
///   file --- `OpenOptions::create_new`, which will not open a name that is
///   there, a link included --- and never made again by name in a later
///   command.
/// - Its name is unique in the process and no client can name it
///   ([`Tree::resolve`] refuses one), so nothing can be put where it is,
///   or where it will be.
///
/// Then a later command that swaps a directory on the file's path for a
/// link moves the temporary's path with it --- the two share every
/// directory --- and the link leads to where no temporary is: the rename
/// fails, and nothing is written there.
pub fn temporary_name(client: u16) -> String {
    let n = NEXT_TEMPORARY.fetch_add(1, Ordering::Relaxed);
    format!("{TEMPORARY_PREFIX}{client}-{n}{TEMPORARY_SUFFIX}")
}

/// Whether a client's `name` could reach a temporary: [`is_temporary`] in
/// any ASCII case, since on a filesystem that folds case, as macOS's
/// usually does, `#OZD-1576-0#` opens `#ozd-1576-0#`. What a pathname may
/// not hold; the cleanup matches exactly, and takes only what it made.
fn names_a_temporary(name: &str) -> bool {
    is_temporary(&name.to_ascii_lowercase())
}

/// Whether `name` is a temporary's name, as [`temporary_name`] makes them:
/// `#ozd-`, digits, `-`, digits, `#`, exactly.
pub fn is_temporary(name: &str) -> bool {
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    name.strip_prefix(TEMPORARY_PREFIX)
        .and_then(|n| n.strip_suffix(TEMPORARY_SUFFIX))
        .and_then(|n| n.split_once('-'))
        .is_some_and(|(client, count)| digits(client) && digits(count))
}

/// A root as the config writes it, `root [name] path`, for a message.
fn describe(r: &Root) -> String {
    match &r.name {
        Some(name) => format!("root {name} {}", r.path.display()),
        None => format!("root {}", r.path.display()),
    }
}

/// A root's own checks, [`Tree::new`]'s first four, each failure named;
/// the root back with its path canonical.
fn checked(r: Root) -> Result<Root, String> {
    let what = describe(&r);
    if let Some(name) = &r.name
        && (name.is_empty()
            || name.contains(['/', '\0'])
            || name == "."
            || name == ".."
            || is_temporary(name))
    {
        return Err(format!("{what}: {name:?} is not a name for a mount, one component of a path"));
    }
    if !r.path.is_absolute() {
        return Err(format!("{what}: not an absolute path"));
    }
    let path =
        std::fs::canonicalize(&r.path).map_err(|e| format!("{what}: cannot be resolved: {e}"))?;
    if path == Path::new("/") {
        return Err(format!("{what}: is /, the whole host, which is never served"));
    }
    if !std::fs::metadata(&path).is_ok_and(|m| m.is_dir()) {
        return Err(format!("{what}: not a directory"));
    }
    Ok(Root { path, ..r })
}

/// Whether a writable root can be written: a probe file made and removed.
/// Made as a new file, so an existing name is never opened; one a crash
/// left, which the count restarting at 0 can meet again, is stepped past.
fn probe(r: &Root) -> Result<(), String> {
    let what = describe(r);
    for _ in 0..16 {
        let p = r.path.join(temporary_name(0));
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&p) {
            Ok(_) => {
                return std::fs::remove_file(&p).map_err(|e| {
                    format!("{what}: not writable: the probe {} is not removed: {e}", p.display())
                });
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
            Err(e) => {
                return Err(format!(
                    "{what}: not writable ({e}); if it is not meant to be, mark it read-only with ,ro"
                ));
            }
        }
    }
    Err(format!("{what}: not writable: no free name for a probe"))
}

/// The warnings of [`Tree::warnings`].
fn covered(roots: &[Root]) -> Vec<String> {
    let Some(base) = roots.iter().find(|r| r.name.is_none()) else {
        return Vec::new();
    };
    roots
        .iter()
        .filter_map(|mount| {
            let name = mount.name.as_deref()?;
            let entry = base.path.join(name);
            std::fs::symlink_metadata(&entry).is_ok().then(|| {
                format!("{}: covered by the mount {name}, and not reachable", entry.display())
            })
        })
        .collect()
}
