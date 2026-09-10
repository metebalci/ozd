// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Containment: the tree FILE serves, and nothing outside it (`CLAUDE.md`
//! §3 and §10, item 4; `DESIGN.md` §6 and §11, item 6).
//!
//! Written before FILE comes across, against [`muir_ah::roots`] alone:
//! every pathname a client can send is resolved here as FILE will resolve
//! it, and every refusal is asserted twice --- its code, and that nothing
//! on disk changed while it was tried. Each test's world is one directory
//! under `std::env::temp_dir()`, holding the roots and, beside them, what
//! must never be reached, so that "nothing changed" covers both.
//!
//! **One case of `CLAUDE.md` §10.4 is not here, on purpose**: "a component
//! that is a symlink swapped between the check and the open". No check can
//! stop that. What makes it impossible is that nothing runs between the
//! two: one thread, this process the only writer of a writable root, and a
//! read-only root written by nobody (`DESIGN.md` §1, §6). That invariant
//! is written down where it is relied on, in `src/roots.rs` at
//! `Tree::resolve`; a test that swapped a link from a second thread would
//! test the invariant broken, not the code. What is tested is the nearest
//! thing that can happen: a link made, or a directory swapped for one,
//! *between two commands*, which the next resolution sees
//! (`a_symlink_made_later_pointing_outside_is_refused`).

use muir_ah::roots::{Place, Resolved, Root, Tree, is_temporary, temporary_name};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// A refusal as FILE answers it: the error code and its message, as muir's
/// `denied()` (`src/chaos/file.rs:93`).
type Refusal = (&'static str, String);

/// Names that look like a temporary's and are not one.
const LOOKALIKES: [&str; 16] = [
    "#muir-1576-x#",
    "#muir-1576-1",
    "muir-1576-1#",
    "#muir--1#",
    "#muir-1576-#",
    "#muir-#",
    "#muir-ah-1-2#",
    "#MUIR-1576-1#",
    "x#muir-1-2#",
    "#muir-1-2#.text",
    "#muir-1-2-3#",
    "#muir-+1-2#",
    "#muir-1-+2#",
    "#muir-1-2# ",
    "##",
    "#",
];

/// One test's world: a directory of its own under the system's temporary
/// directory, canonical so that paths compare with what the tree returns,
/// and removed when the test ends however it ends.
struct Scratch {
    dir: PathBuf,
}

impl Scratch {
    fn new(name: &str) -> Scratch {
        let dir =
            std::env::temp_dir().join(format!("muir-ah-containment-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Scratch { dir: std::fs::canonicalize(&dir).unwrap() }
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.dir.join(relative)
    }

    /// A directory, with its parents.
    fn dir(&self, relative: &str) -> PathBuf {
        let p = self.path(relative);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    /// A file holding `contents`, its directory made first.
    fn file(&self, relative: &str, contents: &str) -> PathBuf {
        let p = self.path(relative);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, contents).unwrap();
        p
    }

    /// A symbolic link at `relative` whose content is `target`, as given.
    #[cfg(unix)]
    fn link(&self, relative: &str, target: impl AsRef<Path>) -> PathBuf {
        let p = self.path(relative);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(target, &p).unwrap();
        p
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // A test that took write permission away gives it back first, so
        // that the world goes even after a failed assertion.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut stack = vec![self.dir.clone()];
            while let Some(d) = stack.pop() {
                let _ = std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o755));
                for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                    if e.file_type().is_ok_and(|t| t.is_dir()) {
                        stack.push(e.path());
                    }
                }
            }
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// An entry on disk as [`snapshot`] records it.
#[derive(Debug, PartialEq, Eq)]
enum Entry {
    File(Vec<u8>),
    Directory,
    Link(PathBuf),
    /// A FIFO or a socket: that it is there, never what it holds, since
    /// reading a FIFO would block.
    Other,
}

/// Everything under `dir`, links not followed: each entry by its path
/// relative to `dir`, with a file's bytes and a link's content.
fn snapshot(dir: &Path) -> BTreeMap<PathBuf, Entry> {
    let mut seen = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let p = e.path();
            let t = e.file_type().unwrap();
            let entry = if t.is_symlink() {
                Entry::Link(std::fs::read_link(&p).unwrap())
            } else if t.is_dir() {
                stack.push(p.clone());
                Entry::Directory
            } else if t.is_file() {
                Entry::File(std::fs::read(&p).unwrap())
            } else {
                Entry::Other
            };
            seen.insert(p.strip_prefix(dir).unwrap().to_path_buf(), entry);
        }
    }
    seen
}

/// Asserts that `attempt` is refused with `code`, and that nothing in the
/// test's world changed while it was tried.
#[track_caller]
fn refused<T: std::fmt::Debug>(
    s: &Scratch,
    code: &str,
    what: &str,
    attempt: impl FnOnce() -> Result<T, Refusal>,
) {
    let before = snapshot(&s.dir);
    let r = attempt();
    assert!(matches!(&r, Err((c, _)) if *c == code), "{what}: wanted {code}, got {r:?}");
    assert_eq!(snapshot(&s.dir), before, "{what}: something on disk changed");
}

/// Every use FILE makes of a pathname, refused with `code`: read, written,
/// and either end of a rename whose other end is `/ok.text` in the base.
#[track_caller]
fn refused_everywhere(s: &Scratch, t: &Tree, code: &str, pathname: &str) {
    refused(s, code, &format!("read {pathname:?}"), || t.resolve(pathname));
    refused(s, code, &format!("write {pathname:?}"), || t.resolve_for_writing(pathname));
    refused(s, code, &format!("rename from {pathname:?}"), || {
        t.resolve_for_renaming(pathname, "/ok.text")
    });
    refused(s, code, &format!("rename to {pathname:?}"), || {
        t.resolve_for_renaming("/ok.text", pathname)
    });
}

/// Asserts that the roots do not start, with `reason` in the error, and
/// that nothing on disk changed in trying.
#[track_caller]
fn does_not_start(s: &Scratch, roots: Vec<Root>, reason: &str) {
    let before = snapshot(&s.dir);
    match Tree::new(roots) {
        Ok(t) => panic!("started, and wanted {reason:?}: {t:?}"),
        Err(e) => assert!(e.contains(reason), "{e:?} does not say {reason:?}"),
    }
    assert_eq!(snapshot(&s.dir), before, "{reason}: something on disk changed");
}

/// Asserts that a place lies in `root`, and that no part of it that exists
/// is a link: what FILE opens is exactly the path that was checked.
#[track_caller]
fn assert_inside(place: &Place, root: &Path) {
    assert!(place.path.starts_with(root), "{place:?} is not in {root:?}");
    for a in place.path.ancestors().take_while(|a| a.starts_with(root)) {
        if let Ok(m) = std::fs::symlink_metadata(a) {
            assert!(!m.file_type().is_symlink(), "{place:?}: {a:?} is a link");
        }
    }
}

/// The place a pathname names, for reading, or the test fails.
#[track_caller]
fn place(t: &Tree, pathname: &str) -> Place {
    match t.resolve(pathname) {
        Ok(Resolved::Place(p)) => p,
        r => panic!("{pathname:?}: {r:?}"),
    }
}

/// The place a pathname names, for writing, or the test fails.
#[track_caller]
fn writable(t: &Tree, pathname: &str) -> Place {
    t.resolve_for_writing(pathname).unwrap_or_else(|e| panic!("{pathname:?}: {e:?}"))
}

fn base(path: &Path) -> Root {
    Root { name: None, path: path.to_path_buf(), readonly: false }
}

fn mount(name: &str, path: &Path) -> Root {
    Root { name: Some(name.to_string()), path: path.to_path_buf(), readonly: false }
}

fn readonly(root: Root) -> Root {
    Root { readonly: true, ..root }
}

/// **`.` and `..` are refused at every depth, in every root**, with `ATD`,
/// and are never normalised away, as muir's `resolve` refuses them
/// (`src/chaos/file.rs:375`). So no pathname climbs out of a root, or from
/// the base into a mount or back. They are refused before the disk is
/// looked at, so a climb through a directory that does not exist is
/// refused the same way.
#[test]
fn dot_and_dot_dot_are_refused_at_every_depth() {
    let s = Scratch::new("dots");
    let b = s.dir("base");
    s.dir("base/a/b/c");
    s.dir("tree-src/x/y");
    s.file("outside/victim.text", "keep\n");
    let t = Tree::new(vec![base(&b), mount("tree", &s.path("tree-src"))]).unwrap();
    for prefix in ["", "/a", "/a/b", "/a/b/c", "/nowhere", "/tree", "/tree/x", "/tree/x/y"] {
        for rest in [
            "..",
            ".",
            "../victim.text",
            "../../outside/victim.text",
            "../../../../../../../../etc/passwd",
            "./new.text",
            "../tree/x",
            "x/../y",
            "x/.",
        ] {
            refused_everywhere(&s, &t, "ATD", &format!("{prefix}/{rest}"));
        }
    }
    for pathname in ["..", ".", "../outside", "a/../../outside", "//..//", "/a/./b", "/a/b/."] {
        refused_everywhere(&s, &t, "ATD", pathname);
    }
    // Three dots are a name, not a climb.
    assert_inside(&writable(&t, "/a/..."), &b);
}

/// **A host's absolute pathname lands under the root.** A client's `/` is
/// the top of the tree, not the host's: `/etc/passwd` names `etc/passwd` in
/// the base whatever the host has at `/etc`, and so does the absolute path
/// of this test's own world.
#[test]
fn absolute_host_names_land_under_the_root() {
    let s = Scratch::new("absolute");
    let b = s.dir("base");
    s.file("base/ok.text", "ok\n");
    let victim = s.file("outside/victim.text", "keep\n");
    let t = Tree::new(vec![base(&b)]).unwrap();
    let own = victim.to_str().unwrap().to_string();
    for pathname in [
        "/etc/passwd",
        "//etc//passwd",
        "etc/passwd",
        "/usr/bin/env",
        "/proc/self/root/etc/passwd",
        own.as_str(),
    ] {
        let before = snapshot(&s.dir);
        let expected =
            pathname.split('/').filter(|c| !c.is_empty()).fold(b.clone(), |p, c| p.join(c));
        let read = place(&t, pathname);
        assert_eq!(read, Place { path: expected.clone(), root: None, readonly: false });
        assert_inside(&read, &b);
        assert_eq!(writable(&t, pathname).path, expected);
        let (from, to) = t.resolve_for_renaming("/ok.text", pathname).unwrap();
        assert_eq!((from.path, to.path), (b.join("ok.text"), expected));
        assert_eq!(snapshot(&s.dir), before, "{pathname:?}: something on disk changed");
    }
}

/// **A link under a root that leads out of it is refused**, with `ATD`,
/// however it is written and wherever it is: absolute or relative, at the
/// top of a root or deep in it, to a directory or to a file, in the base or
/// in a mount. So is a link that does not resolve at all: one whose target
/// does not exist yet would, written through, make that target outside, and
/// one that loops has no place to judge.
#[cfg(unix)]
#[test]
fn a_symlink_under_a_root_pointing_outside_is_refused() {
    let s = Scratch::new("links-out");
    let b = s.dir("base");
    s.dir("base/a/b");
    s.dir("tree-src/x");
    s.file("base/ok.text", "ok\n");
    let victim = s.file("outside/victim.text", "keep\n");
    let outside = s.path("outside");
    s.link("base/out", &outside);
    s.link("base/a/rel", "../../outside");
    s.link("base/a/file-link", &victim);
    s.link("base/a/b/deep", &outside);
    s.link("tree-src/x/out", &outside);
    s.link("base/host-etc", "/etc");
    s.link("base/dangling", outside.join("new.text"));
    s.link("base/dangling-dir", outside.join("new-dir"));
    s.link("base/loop", "loop");
    let t = Tree::new(vec![base(&b), mount("tree", &s.path("tree-src"))]).unwrap();
    for pathname in [
        "/out",
        "/out/victim.text",
        "/out/new.text",
        "/out/new-dir/new.text",
        "/a/rel",
        "/a/rel/victim.text",
        "/a/file-link",
        "/a/b/deep",
        "/a/b/deep/victim.text",
        "/tree/x/out",
        "/tree/x/out/victim.text",
        "/host-etc/passwd",
        "/dangling",
        "/dangling-dir/new.text",
        "/loop",
        "/loop/x",
    ] {
        refused_everywhere(&s, &t, "ATD", pathname);
    }
    assert!(!outside.join("new.text").exists() && !outside.join("new-dir").exists());
}

/// **A link at the top of the base is not a second tree.** muir admits the
/// target of every link in its root's top level as part of the tree it
/// serves, so that its fetch scripts can link a release in
/// (`src/chaos/file.rs:375`). That is a file outside the root served to
/// anyone who can name the link, and it does not come across (`CLAUDE.md`
/// §3). The same directory is served by mounting it, and then by the
/// mount's own rules --- here, read-only.
#[cfg(unix)]
#[test]
fn a_top_level_symlink_is_not_a_second_tree() {
    let s = Scratch::new("second-tree");
    let b = s.dir("base");
    s.file("base/ok.text", "ok\n");
    let release = s.dir("release");
    s.file("release/hello.text", "hello\n");
    s.link("base/sys", &release);
    let linked = Tree::new(vec![base(&b)]).unwrap();
    for pathname in ["/sys", "/sys/hello.text", "/sys/new.text"] {
        refused_everywhere(&s, &linked, "ATD", pathname);
    }

    let mounted = Tree::new(vec![base(&b), readonly(mount("sys", &release))]).unwrap();
    let hello = place(&mounted, "/sys/hello.text");
    assert_eq!(
        hello,
        Place { path: release.join("hello.text"), root: Some("sys".into()), readonly: true }
    );
    refused(&s, "ATF", "a write into the mounted release", || {
        mounted.resolve_for_writing("/sys/new.text")
    });
}

/// **A link made after startup is judged when it is used**, as one a client
/// makes with `CREATE-LINK` would be: nothing about the tree is remembered
/// from one resolution to the next. A directory swapped for a link between
/// two commands, a new link to outside, a relative one that climbs, and
/// links between the base and a mount --- a link must stay in its own root
/// --- are each refused the next time they are named.
#[cfg(unix)]
#[test]
fn a_symlink_made_later_pointing_outside_is_refused() {
    let s = Scratch::new("links-later");
    let b = s.dir("base");
    s.dir("base/later");
    s.file("base/ok.text", "ok\n");
    let tree = s.dir("tree-src/x");
    let outside = s.dir("outside");
    s.file("outside/victim.text", "keep\n");
    let t = Tree::new(vec![base(&b), mount("tree", &s.path("tree-src"))]).unwrap();

    // Resolved while it is a directory, and swapped for a link to outside
    // before the next command.
    assert_inside(&writable(&t, "/later/new.text"), &b);
    std::fs::rename(b.join("later"), b.join("was-later")).unwrap();
    s.link("base/later", &outside);
    refused_everywhere(&s, &t, "ATD", "/later/new.text");
    refused_everywhere(&s, &t, "ATD", "/later/victim.text");

    // New links, absolute and relative.
    s.link("base/made", outside.join("victim.text"));
    refused_everywhere(&s, &t, "ATD", "/made");
    s.link("base/was-later/up", "../..");
    refused_everywhere(&s, &t, "ATD", "/was-later/up/outside/victim.text");

    // From the base into the mount, and from the mount into the base.
    s.link("base/into-tree", &tree);
    refused_everywhere(&s, &t, "ATD", "/into-tree");
    refused_everywhere(&s, &t, "ATD", "/into-tree/new.text");
    s.link("tree-src/x/into-base", &b);
    refused_everywhere(&s, &t, "ATD", "/tree/x/into-base/ok.text");
}

/// **A write's temporary and a rename's two ends stay in the root.** FILE
/// writes into a temporary beside the file and renames it into place
/// (`src/chaos/file.rs:865`), so a write is contained if the directory its
/// pathname resolves to is: every write that resolves has that directory
/// in its root and free of links, and every one that would put the
/// temporary or the file outside is refused before either exists. The
/// service's own temporaries cannot be named by a client at all, so one
/// cannot be read, replaced, renamed or linked over while it is written.
#[cfg(unix)]
#[test]
fn a_write_whose_temporary_or_rename_target_would_be_outside_is_refused() {
    let s = Scratch::new("temporary-out");
    let b = s.dir("base");
    s.dir("base/a/b");
    s.file("base/ok.text", "ok\n");
    s.file("base/a/old.text", "old\n");
    let outside = s.dir("outside");
    s.file("outside/victim.text", "keep\n");
    s.link("base/out", &outside);
    s.link("base/a/dangling", outside.join("new.text"));
    s.link("base/inside", b.join("a"));
    let t = Tree::new(vec![base(&b)]).unwrap();

    for pathname in ["/new.text", "/a/new.text", "/a/b/new.text", "/a/old.text", "/inside/new.text"]
    {
        let p = writable(&t, pathname);
        assert_inside(&p, &b);
        let dir = p.path.parent().unwrap();
        assert!(dir.starts_with(&b), "{pathname:?}: the temporary would be in {dir:?}");
        assert_eq!(std::fs::canonicalize(dir).unwrap(), dir, "{pathname:?}: {dir:?}");
    }
    for pathname in ["/out/new.text", "/out/victim.text", "/out/a/b/new.text", "/a/dangling"] {
        refused_everywhere(&s, &t, "ATD", pathname);
    }
    refused(&s, "ATD", "rename out", || t.resolve_for_renaming("/a/old.text", "/out/moved.text"));
    refused(&s, "ATD", "rename in", || t.resolve_for_renaming("/out/victim.text", "/moved.text"));
    refused(&s, "ATD", "rename up", || t.resolve_for_renaming("/a/old.text", "/../moved.text"));

    // A temporary in the middle of being written.
    let temp = temporary_name(0o3050);
    std::fs::write(b.join("a").join(&temp), "half a file\n").unwrap();
    for pathname in [format!("/{temp}"), format!("/a/{temp}"), format!("/a/{temp}/x")] {
        refused_everywhere(&s, &t, "ATD", &pathname);
    }
    assert!(!outside.join("new.text").exists());
}

/// **A rename from one root into another is refused**, with `ATF`: it
/// would be a copy and not a rename, and it would carry a file across the
/// line between two roots' rules (`DESIGN.md` §6). Within one root, both
/// ends resolve in it.
#[test]
fn a_rename_across_roots_is_refused() {
    let s = Scratch::new("rename-roots");
    let b = s.dir("base");
    let one = s.dir("one");
    let two = s.dir("two");
    s.file("base/a.text", "a\n");
    s.file("one/b.text", "b\n");
    s.file("two/c.text", "c\n");
    let t = Tree::new(vec![base(&b), mount("one", &one), mount("two", &two)]).unwrap();
    for (from, to) in [
        ("/a.text", "/one/a.text"),
        ("/one/b.text", "/b.text"),
        ("/one/b.text", "/two/b.text"),
        ("/two/c.text", "/one/c.text"),
        ("/two/c.text", "/c.text"),
    ] {
        refused(&s, "ATF", &format!("{from} to {to}"), || t.resolve_for_renaming(from, to));
    }
    let (from, to) = t.resolve_for_renaming("/a.text", "/moved.text").unwrap();
    assert_eq!((from.path, to.path), (b.join("a.text"), b.join("moved.text")));
    let (from, to) = t.resolve_for_renaming("/one/b.text", "/one/sub/b.text").unwrap();
    assert_eq!((from.root, to.root), (Some("one".into()), Some("one".into())));
    assert_eq!((from.path, to.path), (one.join("b.text"), one.join("sub/b.text")));
}

/// **A FIFO, or anything else that is neither a regular file nor a
/// directory, is refused before it is opened**, with `ATD` (`DESIGN.md`
/// §6). The loop is one thread, and opening a FIFO that has no writer
/// blocks it, and every client with it (`CLAUDE.md` §8d); the kind is read
/// with `symlink_metadata`, which opens nothing. muir answers the same
/// case with `WKF` (`src/chaos/file.rs`, `open`). A socket stands for a
/// device node, which a test cannot make. If this test hangs, something
/// opened the FIFO.
#[cfg(unix)]
#[test]
fn a_fifo_is_refused() {
    let s = Scratch::new("fifo");
    let b = s.dir("base");
    s.dir("base/a");
    s.file("base/file.text", "file\n");
    s.dir("tree-src");
    let made = std::process::Command::new("mkfifo")
        .arg(b.join("a/pipe"))
        .arg(s.path("tree-src/pipe"))
        .status();
    let fifos = made.is_ok_and(|m| m.success());
    let mut odd = Vec::new();
    if fifos {
        s.link("base/to-pipe", b.join("a/pipe"));
        odd.extend(["/a/pipe", "/tree/pipe", "/to-pipe"]);
    } else {
        eprintln!("skipped the FIFOs: mkfifo is not available");
    }
    // A socket's path has a length limit, which a deep temporary
    // directory can pass.
    let socket = std::os::unix::net::UnixListener::bind(b.join("sock"));
    if socket.is_ok() {
        odd.push("/sock");
    } else {
        eprintln!("skipped the socket: {socket:?}");
    }
    let t = Tree::new(vec![base(&b), mount("tree", &s.path("tree-src"))]).unwrap();
    for pathname in odd {
        let read = place(&t, pathname);
        refused(&s, "ATD", &format!("read {pathname:?}"), || read.metadata());
        let write = writable(&t, pathname);
        refused(&s, "ATD", &format!("write {pathname:?}"), || write.metadata());
    }
    // What may be opened, and what is not there, are told apart.
    assert!(place(&t, "/file.text").metadata().unwrap().unwrap().is_file());
    assert!(place(&t, "/a").metadata().unwrap().unwrap().is_dir());
    assert!(place(&t, "/missing.text").metadata().unwrap().is_none());
    assert!(place(&t, "/file.text/under-a-file").metadata().unwrap().is_none());
}

/// **A tree that cannot be served safely does not start, and says why**:
/// no root at all; a root that is not an absolute path, does not exist or
/// does not resolve, is not a directory, or is the host's `/` --- even
/// read-only, and even by a link; two bases; a mount's name twice, or a
/// name that is not one component; and two roots of which one is, or lies
/// in, the other, which would let a path through the outer reach the
/// inner's files under the outer's rules. Nothing is left on disk by any
/// of it, and the probe of a writable root that does start leaves nothing
/// either (`CLAUDE.md` §3, §10.4; `DESIGN.md` §6).
#[test]
fn a_root_that_is_missing_not_a_directory_slash_or_relative_does_not_start() {
    let s = Scratch::new("startup");
    let b = s.dir("base");
    let one = s.dir("one");
    let inner = s.dir("base/inner");
    let file = s.file("file.text", "not a directory\n");
    let missing = s.path("missing");

    does_not_start(&s, vec![], "no root");
    does_not_start(&s, vec![base(&missing)], "cannot be resolved");
    does_not_start(&s, vec![readonly(base(&missing))], "cannot be resolved");
    does_not_start(&s, vec![base(&b), mount("gone", &missing)], "cannot be resolved");
    does_not_start(&s, vec![base(&file)], "not a directory");
    does_not_start(&s, vec![base(Path::new("/"))], "is /");
    does_not_start(&s, vec![readonly(base(Path::new("/")))], "is /");
    does_not_start(&s, vec![base(&b), readonly(mount("all", Path::new("/")))], "is /");
    for relative in ["base", "./base", "../base", ""] {
        does_not_start(&s, vec![base(Path::new(relative))], "not an absolute path");
    }
    does_not_start(&s, vec![base(&b), base(&one)], "more than one base");
    does_not_start(&s, vec![mount("tree", &b), mount("tree", &one)], "named twice");
    for name in ["", "a/b", "/tree", ".", "..", "#muir-1576-0#"] {
        does_not_start(&s, vec![base(&b), mount(name, &one)], "not a name");
    }
    does_not_start(&s, vec![base(&b), readonly(mount("inner", &inner))], "overlaps");
    does_not_start(&s, vec![base(&inner), mount("all", &b)], "overlaps");
    does_not_start(&s, vec![base(&b), readonly(mount("again", &b))], "overlaps");
    does_not_start(&s, vec![mount("one", &one), mount("two", &one)], "overlaps");
    #[cfg(unix)]
    {
        s.link("slash", "/");
        does_not_start(&s, vec![readonly(base(&s.path("slash")))], "is /");
        s.link("dangling", &missing);
        does_not_start(&s, vec![base(&s.path("dangling"))], "cannot be resolved");
        s.link("to-inner", &inner);
        does_not_start(&s, vec![base(&b), mount("inner", &s.path("to-inner"))], "overlaps");
    }

    // Each refusal names the root it is about.
    let e = Tree::new(vec![base(&missing)]).unwrap_err();
    assert!(e.contains(&missing.display().to_string()), "{e}");
    let e = Tree::new(vec![base(&b), mount("inner", &inner)]).unwrap_err();
    assert!(e.contains("inner") && e.contains(&b.display().to_string()), "{e}");

    // The probe of writable roots that do start leaves nothing behind.
    let before = snapshot(&s.dir);
    Tree::new(vec![base(&b), mount("one", &one)]).unwrap();
    assert_eq!(snapshot(&s.dir), before, "the probe left something");
    // And `/` was refused before any probe was made in it.
    let probes = std::fs::read_dir("/")
        .unwrap()
        .flatten()
        .filter(|e| is_temporary(&e.file_name().to_string_lossy()))
        .count();
    assert_eq!(probes, 0);
}

/// **A writable root that this process cannot write does not start, and a
/// read-only one does**: whether a root can be written is found by making
/// a probe file in it and removing it, and a read-only root is never
/// written, so never probed.
#[cfg(unix)]
#[test]
fn a_writable_root_that_cannot_be_written_does_not_start() {
    use std::os::unix::fs::PermissionsExt;
    let s = Scratch::new("unwritable");
    let ro = s.dir("ro");
    s.file("ro/kept.text", "kept\n");
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o555)).unwrap();
    if std::fs::write(ro.join("privileged"), "").is_ok() {
        eprintln!("skipped: this user writes into a directory of mode 555");
        return;
    }
    does_not_start(&s, vec![base(&ro)], "not writable");
    let t = Tree::new(vec![readonly(base(&ro))]).unwrap();
    assert!(place(&t, "/kept.text").readonly);
}

/// **A read-only root refuses every write, with `ATF`, and serves every
/// read.** "Access to file denied" is what the band turns into
/// `INCORRECT-ACCESS-TO-FILE` (`sys/io/file/open.lisp:224`); FILE has no
/// code of its own for a refused write (`PROTOCOLS.md`, FILE). Every
/// command that writes resolves its pathnames for writing --- OPEN for
/// output, DELETE, both ends of RENAME, CREATE-DIRECTORY, CREATE-LINK's
/// link, CHANGE-PROPERTIES --- so each pathname here stands for all of
/// them. A climb out of a read-only root is still a climb, `ATD`. With
/// mounts and no base, `/` itself is read-only, and names nothing but the
/// mounts.
#[test]
fn a_readonly_root_refuses_every_write_with_atf() {
    let s = Scratch::new("readonly");
    let b = s.dir("base");
    s.file("base/file.text", "base\n");
    let sys = s.dir("sys-src");
    s.file("sys-src/file.text", "sys\n");
    s.file("sys-src/dir/inner.text", "inner\n");
    let users = s.dir("users-src");

    let t = Tree::new(vec![base(&b), readonly(mount("sys", &sys))]).unwrap();
    for pathname in [
        "/sys",
        "/sys/",
        "/sys/file.text",
        "/sys/new.text",
        "/sys/dir",
        "/sys/dir/inner.text",
        "/sys/dir/new/deeper.text",
        "/sys/nowhere/x",
    ] {
        refused(&s, "ATF", &format!("write {pathname:?}"), || t.resolve_for_writing(pathname));
    }
    for (from, to) in [
        ("/sys/file.text", "/sys/moved.text"),
        ("/file.text", "/sys/file.text"),
        ("/sys/file.text", "/stolen.text"),
    ] {
        refused(&s, "ATF", &format!("{from} to {to}"), || t.resolve_for_renaming(from, to));
    }
    refused(&s, "ATD", "a climb", || t.resolve_for_writing("/sys/../base/file.text"));
    assert_eq!(
        place(&t, "/sys/dir/inner.text"),
        Place { path: sys.join("dir/inner.text"), root: Some("sys".into()), readonly: true }
    );
    assert!(!writable(&t, "/file.text").readonly);

    let t = Tree::new(vec![readonly(base(&b))]).unwrap();
    for pathname in ["", "/", "/file.text", "/new.text", "/a/b"] {
        refused(&s, "ATF", &format!("write {pathname:?}"), || t.resolve_for_writing(pathname));
    }
    assert!(place(&t, "/file.text").readonly);

    let t = Tree::new(vec![mount("users", &users), readonly(mount("sys", &sys))]).unwrap();
    for pathname in ["", "/", "/elsewhere.text", "/elsewhere/x", "/sys/file.text"] {
        refused(&s, "ATF", &format!("write {pathname:?}"), || t.resolve_for_writing(pathname));
    }
    refused(&s, "FNF", "read a name that is no mount", || t.resolve("/elsewhere.text"));
    assert_eq!(writable(&t, "/users/new.text").path, users.join("new.text"));
}

/// **A link inside a root, to somewhere in the same root, is followed**,
/// wherever it is and however it is written, and what comes back is where
/// it leads: the canonical path, every link resolved, with the part that
/// does not exist yet joined on. That is what FILE opens (`DESIGN.md` §6,
/// step 4).
#[cfg(unix)]
#[test]
fn a_symlink_inside_a_root_is_followed() {
    let s = Scratch::new("links-in");
    let b = s.dir("base");
    let real = s.dir("base/real");
    s.file("base/real/file.text", "real\n");
    let tree = s.dir("tree-src");
    s.file("tree-src/x/file.text", "x\n");
    s.link("base/abs", &real);
    s.link("base/a/rel", "../real");
    s.link("base/flink", "real/file.text");
    s.link("base/chain", "abs");
    s.link("base/self", &b);
    s.link("tree-src/l", "x");
    let t = Tree::new(vec![base(&b), mount("tree", &tree)]).unwrap();

    let file = real.join("file.text");
    for pathname in
        ["/abs/file.text", "/a/rel/file.text", "/flink", "/chain/file.text", "/self/real/file.text"]
    {
        let p = place(&t, pathname);
        assert_eq!(p, Place { path: file.clone(), root: None, readonly: false }, "{pathname:?}");
        assert_inside(&p, &b);
    }
    assert!(place(&t, "/flink").metadata().unwrap().unwrap().is_file());
    assert_eq!(writable(&t, "/abs/new.text").path, real.join("new.text"));
    assert_eq!(writable(&t, "/a/rel/sub/new.text").path, real.join("sub/new.text"));
    let x = place(&t, "/tree/l/file.text");
    assert_eq!(
        x,
        Place { path: tree.join("x/file.text"), root: Some("tree".into()), readonly: false }
    );
    assert_inside(&x, &tree);
}

/// **A mount is reached by its name, and only by it.** `/tree/...` is the
/// mount; no climb out of the base reaches the directory it serves, and
/// neither does a link from the base into it --- which is how a client
/// would otherwise write a read-only mount, by `CREATE-LINK` in the base to
/// a file in the mount and then a write through the link. The other way
/// about, likewise.
#[cfg(unix)]
#[test]
fn a_mount_is_reached_by_its_name_and_not_by_dot_dot() {
    let s = Scratch::new("mount-by-name");
    let b = s.dir("base");
    s.dir("base/a");
    s.file("base/ok.text", "ok\n");
    let tree = s.dir("tree-src");
    s.file("tree-src/x/file.text", "x\n");
    s.link("base/to-tree", &tree);
    s.link("base/to-file", tree.join("x/file.text"));
    s.link("tree-src/x/to-base", &b);
    let t = Tree::new(vec![base(&b), readonly(mount("tree", &tree))]).unwrap();

    assert_eq!(
        place(&t, "/tree/x/file.text"),
        Place { path: tree.join("x/file.text"), root: Some("tree".into()), readonly: true }
    );
    for pathname in [
        "/../tree-src/x/file.text",
        "/a/../../tree-src/x/file.text",
        "/../tree/x/file.text",
        "/tree/../base/ok.text",
        "/to-tree",
        "/to-tree/x/file.text",
        "/to-tree/x/new.text",
        "/to-file",
        "/tree/x/to-base/ok.text",
    ] {
        refused_everywhere(&s, &t, "ATD", pathname);
    }
}

/// **A mount covers the base's entry of its name, and startup says so**, as
/// a Unix mount point covers the directory under it (`DESIGN.md` §6):
/// every pathname starting with the name is the mount's, the base's own
/// entry cannot be reached, and nothing can be made in the base under that
/// name. A tree whose base has no such entry warns of nothing.
#[test]
fn a_mount_covers_a_base_directory_of_its_name_and_startup_warns() {
    let s = Scratch::new("covered");
    let b = s.dir("base");
    s.file("base/tree/secret.text", "the base's\n");
    s.file("base/zeta", "a file\n");
    s.dir("base/other");
    let tree = s.dir("tree-src/sys");
    let tree = tree.parent().unwrap().to_path_buf();
    let zeta = s.dir("zeta-src");

    let t = Tree::new(vec![base(&b), mount("tree", &tree)]).unwrap();
    assert_eq!(t.warnings().len(), 1, "{:?}", t.warnings());
    let warning = &t.warnings()[0];
    assert!(warning.contains("tree") && warning.contains(&b.join("tree").display().to_string()));
    for pathname in ["/tree/secret.text", "//tree//secret.text", "tree/secret.text"] {
        assert_eq!(
            place(&t, pathname),
            Place { path: tree.join("secret.text"), root: Some("tree".into()), readonly: false }
        );
    }
    assert_eq!(place(&t, "/tree").path, tree);
    refused(&s, "ATD", "make the mount's name", || t.resolve_for_writing("/tree"));
    assert_eq!(t.list_top().unwrap(), ["other", "tree", "zeta"]);

    // A file is covered as a directory is.
    let t = Tree::new(vec![base(&b), mount("zeta", &zeta)]).unwrap();
    assert_eq!(t.warnings().len(), 1, "{:?}", t.warnings());
    assert!(t.warnings()[0].contains("zeta"));

    let t = Tree::new(vec![base(&b), mount("elsewhere", &zeta)]).unwrap();
    assert!(t.warnings().is_empty(), "{:?}", t.warnings());
}

/// **Names match exactly**, case and all, as every directory name does:
/// muir's FILE folds no case in a pathname (`DESIGN.md` §6). Only a first
/// component that is a mount's name, exactly, is the mount; anything else
/// is the base's, and with no base it names nothing.
#[test]
fn names_match_exactly() {
    let s = Scratch::new("names");
    let b = s.dir("base");
    let tree = s.dir("tree-src");
    let t = Tree::new(vec![base(&b), mount("tree", &tree)]).unwrap();
    for (pathname, under) in [
        ("/TREE/x", "TREE/x"),
        ("/Tree/x", "Tree/x"),
        ("/tree2/x", "tree2/x"),
        ("/tre/x", "tre/x"),
        ("/tree.text", "tree.text"),
        ("/ tree/x", " tree/x"),
    ] {
        assert_eq!(
            place(&t, pathname),
            Place { path: b.join(under), root: None, readonly: false },
            "{pathname:?}"
        );
    }
    assert_eq!(place(&t, "/tree/x").root.as_deref(), Some("tree"));

    let t = Tree::new(vec![mount("tree", &tree)]).unwrap();
    refused(&s, "FNF", "read /TREE/x", || t.resolve("/TREE/x"));
    refused(&s, "ATF", "write /TREE/x", || t.resolve_for_writing("/TREE/x"));
    assert_eq!(place(&t, "/tree/x").path, tree.join("x"));
}

/// **The listing of `/` names each entry once**: the base's entries and the
/// mounts' names, a mount's name standing for a base entry of the same
/// name; with no base, the mounts alone. `/` itself, however it is spelt,
/// is the top of the tree, and is not written.
#[test]
fn the_listing_of_the_top_names_each_entry_once() {
    let s = Scratch::new("top");
    let b = s.dir("base");
    s.dir("base/a");
    s.file("base/b.text", "b\n");
    s.dir("base/tree");
    let tree = s.dir("tree-src");
    let zeta = s.dir("zeta-src");

    let t = Tree::new(vec![base(&b), mount("tree", &tree), mount("zeta", &zeta)]).unwrap();
    assert_eq!(t.list_top().unwrap(), ["a", "b.text", "tree", "zeta"]);
    for pathname in ["", "/", "///"] {
        assert_eq!(t.resolve(pathname), Ok(Resolved::Top), "{pathname:?}");
        refused(&s, "ATD", &format!("write {pathname:?}"), || t.resolve_for_writing(pathname));
    }

    let t = Tree::new(vec![mount("zeta", &zeta), mount("tree", &tree)]).unwrap();
    assert_eq!(t.list_top().unwrap(), ["tree", "zeta"]);
    assert_eq!(t.resolve("/"), Ok(Resolved::Top));
    refused(&s, "ATF", "write / with no base", || t.resolve_for_writing("/"));

    let t = Tree::new(vec![readonly(base(&b))]).unwrap();
    assert_eq!(t.list_top().unwrap(), ["a", "b.text", "tree"]);
    refused(&s, "ATF", "write / of a read-only base", || t.resolve_for_writing("/"));
}

/// **A root itself is no file to write**, however it is named: FILE may not
/// delete, rename, replace or make it, as muir's `resolve_for_writing`
/// refuses it (`src/chaos/file.rs:405`), and so nothing can be made at `/`
/// under a mount's name. Refused with `ATD`, as muir refuses it. Read, it
/// is a directory like any other.
#[cfg(unix)]
#[test]
fn a_root_itself_is_not_written() {
    let s = Scratch::new("root-itself");
    let b = s.dir("base");
    s.file("base/ok.text", "ok\n");
    let tree = s.dir("tree-src");
    s.link("base/self", &b);
    s.link("tree-src/self", &tree);
    let t = Tree::new(vec![base(&b), mount("tree", &tree)]).unwrap();
    for pathname in ["", "/", "//", "/tree", "/tree/", "//tree//", "/self", "/tree/self"] {
        refused(&s, "ATD", &format!("write {pathname:?}"), || t.resolve_for_writing(pathname));
        refused(&s, "ATD", &format!("rename from {pathname:?}"), || {
            t.resolve_for_renaming(pathname, "/moved")
        });
        refused(&s, "ATD", &format!("rename to {pathname:?}"), || {
            t.resolve_for_renaming("/ok.text", pathname)
        });
    }
    assert_eq!(place(&t, "/tree").path, tree);
    assert_eq!(place(&t, "/self").path, b);
}

/// **At startup the stale temporaries of the writable roots are removed,
/// and nothing else is.** A daemon killed in the middle of a write leaves
/// its temporary behind (`DESIGN.md` §6, §10), and they are found by the
/// one name FILE gives them, in every directory of every writable root,
/// without following a link. Kept: every name that only looks like one; a
/// directory or a link with a temporary's name, since FILE only makes
/// regular files; one behind a link out of the root; and one in a
/// read-only root, which is never written.
#[cfg(unix)]
#[test]
fn stale_temporaries_are_removed_and_nothing_else() {
    let s = Scratch::new("stale");
    let b = s.dir("base");
    s.dir("base/a/b");
    let users = s.dir("users-src");
    let sys = s.dir("sys-src");
    let outside = s.dir("outside");
    s.file("outside/victim.text", "keep\n");

    let stale = [
        b.join(temporary_name(0o3050)),
        b.join("a/b").join(temporary_name(0o3051)),
        users.join(temporary_name(0o3050)),
    ];
    for t in &stale {
        std::fs::write(t, "half a file\n").unwrap();
    }
    for name in LOOKALIKES {
        std::fs::write(b.join(name), "a file of the user's\n").unwrap();
    }
    let dir = b.join(temporary_name(0o3050));
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("inner.text"), "inner\n").unwrap();
    s.link(&format!("base/{}", temporary_name(0o3050)), outside.join("victim.text"));
    s.link("base/out", &outside);
    std::fs::write(outside.join(temporary_name(0o3050)), "not in a root\n").unwrap();
    std::fs::write(sys.join(temporary_name(0o3050)), "in a read-only root\n").unwrap();

    let t =
        Tree::new(vec![base(&b), mount("users", &users), readonly(mount("sys", &sys))]).unwrap();
    let mut expected = snapshot(&s.dir);
    for p in &stale {
        expected.remove(p.strip_prefix(&s.dir).unwrap()).unwrap();
    }
    let removed: BTreeSet<PathBuf> =
        t.remove_temporaries().into_iter().map(|r| r.unwrap()).collect();
    assert_eq!(removed, stale.iter().cloned().collect());
    assert_eq!(snapshot(&s.dir), expected);
    assert!(t.remove_temporaries().is_empty(), "a second pass finds nothing");
}

/// **A temporary is named as muir names its own**: `#muir-`, the client's
/// address in decimal, `-`, a count taken once per write for the whole
/// process, and `#` (`src/chaos/file.rs:865`, and `NEXT_TEMP` at `:70`),
/// so that no two writes, from one client or two, ever share one. What
/// only looks like one is not one.
#[test]
fn a_temporary_is_named_as_muirs_are() {
    let (a, b) = (temporary_name(0o3050), temporary_name(0o3050));
    assert_ne!(a, b);
    for n in [&a, &b] {
        let count = n.strip_prefix("#muir-1576-").and_then(|n| n.strip_suffix('#'));
        assert!(count.is_some_and(|c| !c.is_empty() && c.bytes().all(|d| d.is_ascii_digit())));
        assert!(is_temporary(n), "{n}");
    }
    assert!(is_temporary("#muir-0-0#") && is_temporary("#muir-65535-18446744073709551615#"));
    for name in LOOKALIKES {
        assert!(!is_temporary(name), "{name:?}");
    }
}

/// An entry's resolution for writing, or the test fails.
#[track_caller]
fn entry(t: &Tree, pathname: &str) -> muir_ah::roots::Entry {
    t.resolve_entry_for_writing(pathname).unwrap_or_else(|e| panic!("{pathname:?}: {e:?}"))
}

/// Every use FILE makes of a pathname as an entry, refused with `code`:
/// read, as DIRECTORY describes a name the tree will not follow; written, as
/// DELETE removes one; and either end of a RENAME whose other end is
/// `/ok.text` in the base.
#[track_caller]
fn entry_refused_everywhere(s: &Scratch, t: &Tree, code: &str, pathname: &str) {
    refused(s, code, &format!("entry {pathname:?}"), || t.resolve_entry(pathname));
    refused(s, code, &format!("entry to write {pathname:?}"), || {
        t.resolve_entry_for_writing(pathname)
    });
    refused(s, code, &format!("rename the entry {pathname:?}"), || {
        t.resolve_entries_for_renaming(pathname, "/ok.text")
    });
    refused(s, code, &format!("rename to the entry {pathname:?}"), || {
        t.resolve_entries_for_renaming("/ok.text", pathname)
    });
}

/// **DELETE and RENAME act on a name itself, not on what it leads to**
/// (`DESIGN.md` §6, "FILE's rules"), as Unix does and as muir did: the
/// pathname's directory is resolved as any place is --- canonical, in its
/// root, every link on the way followed and held to that root --- and its
/// last component is joined on as the directory holds it, a link or not,
/// and not followed. So a link at the end is the link itself, wherever it
/// leads: into its own root, into another, out of the tree, to nothing, or
/// round in a circle. Removing one removes the link and never its target,
/// which is how a link that leads nowhere is got rid of; renaming one moves
/// the link. Resolving touches nothing.
#[cfg(unix)]
#[test]
fn an_entry_is_its_directory_resolved_and_its_own_name_not_followed() {
    let s = Scratch::new("entries");
    let b = s.dir("base");
    let real = s.dir("base/real");
    s.file("base/real/file.text", "real\n");
    let tree = s.dir("tree-src");
    s.file("tree-src/x.text", "x\n");
    let outside = s.dir("outside");
    let victim = s.file("outside/victim.text", "keep\n");
    s.link("base/in", "real/file.text");
    s.link("base/dir-link", &real);
    s.link("base/out", &outside);
    s.link("base/out-file", &victim);
    s.link("base/dangling", outside.join("new.text"));
    s.link("base/loop", "loop");
    s.link("base/to-tree", tree.join("x.text"));
    s.link("base/real/deep-out", &outside);
    s.link("tree-src/l", "x.text");
    let t = Tree::new(vec![base(&b), mount("tree", &tree)]).unwrap();

    for (pathname, directory, name) in [
        ("/in", &b, "in"),
        ("/dir-link", &b, "dir-link"),
        ("/out", &b, "out"),
        ("/out-file", &b, "out-file"),
        ("/dangling", &b, "dangling"),
        ("/loop", &b, "loop"),
        ("/to-tree", &b, "to-tree"),
        ("//real//deep-out", &real, "deep-out"),
        ("/dir-link/deep-out", &real, "deep-out"),
        ("/dir-link/file.text", &real, "file.text"),
        ("/real/new.text", &real, "new.text"),
        ("/tree/l", &tree, "l"),
        ("/tree/x.text", &tree, "x.text"),
    ] {
        let before = snapshot(&s.dir);
        let e = entry(&t, pathname);
        assert_eq!((&e.directory.path, e.name.as_str()), (directory, name), "{pathname:?}");
        assert_eq!(e.path(), directory.join(name), "{pathname:?}");
        let root = if directory.starts_with(&tree) { &tree } else { &b };
        assert_inside(&e.directory, root);
        assert_eq!(e.directory.root.as_deref(), (root == &tree).then_some("tree"));
        assert_eq!(t.resolve_entry(pathname).as_ref(), Ok(&e), "{pathname:?}: read as written");
        assert_eq!(snapshot(&s.dir), before, "{pathname:?}: something on disk changed");
    }
    // The same names followed, as a read or a write follows them, are
    // refused wherever the link leads out of its root or nowhere.
    for pathname in ["/out", "/out-file", "/dangling", "/loop", "/to-tree", "/real/deep-out"] {
        refused(&s, "ATD", &format!("follow {pathname:?}"), || t.resolve(pathname));
    }

    // Removed, a link is gone and what it led to is not.
    let mut expected = snapshot(&s.dir);
    for pathname in ["/dangling", "/loop", "/out-file", "/out", "/in"] {
        std::fs::remove_file(entry(&t, pathname).path()).unwrap();
        expected.remove(Path::new("base").join(&pathname[1..]).as_path()).unwrap();
    }
    // Renamed, the link moves, and leads where it did.
    let (from, to) = t.resolve_entries_for_renaming("/to-tree", "/dir-link/moved").unwrap();
    assert_eq!((from.path(), to.path()), (b.join("to-tree"), real.join("moved")));
    std::fs::rename(from.path(), to.path()).unwrap();
    let moved = expected.remove(Path::new("base/to-tree")).unwrap();
    expected.insert(PathBuf::from("base/real/moved"), moved);
    assert_eq!(snapshot(&s.dir), expected);
    assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep\n");
    assert_eq!(std::fs::read_to_string(real.join("file.text")).unwrap(), "real\n");
}

/// **An entry is refused as a place is, except for where its own name
/// leads.** A climb, a temporary's name, or a directory on the way that
/// leaves its root, leads nowhere or loops is `ATD`, for reading, writing
/// and either end of a rename. A read-only root refuses every entry in it
/// for writing with `ATF`, and reads it; so is a rename from one root into
/// another, and with no base `/` itself. `/` and a root itself are no name
/// in a directory, and are refused as [`Tree::resolve_for_writing`] refuses
/// them. Nothing on disk changes in any of it.
#[cfg(unix)]
#[test]
fn an_entry_is_refused_as_a_place_is_but_for_its_own_name() {
    let s = Scratch::new("entries-refused");
    let b = s.dir("base");
    s.dir("base/a");
    s.file("base/ok.text", "ok\n");
    let sys = s.dir("sys-src");
    s.file("sys-src/file.text", "sys\n");
    s.link("sys-src/dangling", "nowhere");
    let tree = s.dir("tree-src");
    s.file("tree-src/x", "x\n");
    let outside = s.dir("outside");
    s.file("outside/victim.text", "keep\n");
    s.link("base/out", &outside);
    s.link("base/dangling-dir", outside.join("new-dir"));
    s.link("base/loop", "loop");
    s.link("base/to-tree", &tree);
    let temp = temporary_name(0o3050);
    std::fs::write(b.join("a").join(&temp), "half a file\n").unwrap();
    let t = Tree::new(vec![base(&b), mount("tree", &tree), readonly(mount("sys", &sys))]).unwrap();

    for pathname in [
        "..",
        "/..",
        "/a/..",
        "/a/../ok.text",
        "/./ok.text",
        "/a/.",
        "/tree/../ok.text",
        "/tree/../../outside/victim.text",
        "/out/victim.text",
        "/out/new.text",
        "/out/a/b",
        "/dangling-dir/new.text",
        "/loop/x",
        "/to-tree/x",
    ] {
        entry_refused_everywhere(&s, &t, "ATD", pathname);
    }
    for pathname in [format!("/a/{temp}"), format!("/{temp}"), format!("/{temp}/x")] {
        entry_refused_everywhere(&s, &t, "ATD", &pathname);
    }

    // A read-only root: every entry in it, and the root itself, refused for
    // writing; and read, a link in it that leads nowhere included.
    for pathname in ["/sys", "/sys/file.text", "/sys/new.text", "/sys/dangling", "/sys/dir/x"] {
        refused(&s, "ATF", &format!("write {pathname:?}"), || {
            t.resolve_entry_for_writing(pathname)
        });
    }
    for (from, to) in [
        ("/sys/file.text", "/sys/moved.text"),
        ("/ok.text", "/sys/ok.text"),
        ("/sys/file.text", "/stolen.text"),
        ("/ok.text", "/tree/ok.text"),
        ("/tree/x", "/x"),
    ] {
        refused(&s, "ATF", &format!("{from} to {to}"), || t.resolve_entries_for_renaming(from, to));
    }
    let e = t.resolve_entry("/sys/dangling").unwrap();
    assert_eq!((e.path(), e.directory.readonly), (sys.join("dangling"), true));
    let (from, to) = t.resolve_entries_for_renaming("/ok.text", "/a/moved.text").unwrap();
    assert_eq!((from.path(), to.path()), (b.join("ok.text"), b.join("a/moved.text")));

    // `/` and a root itself.
    for pathname in ["", "/", "//", "/tree", "/tree/", "//tree//"] {
        entry_refused_everywhere(&s, &t, "ATD", pathname);
    }

    // With no base, `/` is read-only and names only the mounts.
    let t = Tree::new(vec![mount("tree", &tree), readonly(mount("sys", &sys))]).unwrap();
    refused(&s, "FNF", "read a name that is no mount's", || t.resolve_entry("/elsewhere"));
    for pathname in ["", "/", "/elsewhere", "/elsewhere/x"] {
        refused(&s, "ATF", &format!("write {pathname:?}"), || {
            t.resolve_entry_for_writing(pathname)
        });
    }
    assert_eq!(entry(&t, "/tree/new.text").path(), tree.join("new.text"));
}
