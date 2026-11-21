//! What a link gave of the native side of the command-line target. See
//! "Native code" in `docs/spec/linking.md`.
//!
//! All of the native side is in groups, and a group is reached only
//! through one function: a [`Group`]'s `link`, which a link names if
//! its program reaches an extern of the group. An executable whose
//! `main` does not name a group holds none of its code, since nothing
//! else refers to it: a program pays for the native code it uses. A
//! process that was not linked for one program, as the compiler's, has
//! them all ([`Linked::whole`]).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc::Sender;
use std::task::Poll;

use super::http::{Handler, Starter};
use super::pending::Sent;
use super::trust::Trust;
use super::waits::Done;
use super::watching::Change;
use super::{Failure, Kind, Listening, PACKAGE, Request, Response, World};
use super::{code, crypto, exchange, files, http, net, network, plain, watching};
use crate::native::Natives;
use crate::value::Value;

/// The handlers of a group that ask a world: those that do not wait,
/// and what begins the work of those that do, which is answered when
/// it is over.
pub(super) struct Offer {
    pub(super) handlers: &'static [(&'static str, Handler)],
    pub(super) starters: &'static [(&'static str, Starter)],
}

impl Offer {
    /// `natives`, with these handlers over `world`.
    pub(super) fn over(&self, natives: Natives, world: &Rc<RefCell<dyn World>>) -> Natives {
        let natives = self
            .handlers
            .iter()
            .fold(natives, |natives, (name, handler)| {
                let world = Rc::clone(world);
                natives.with(PACKAGE, name, move |args| {
                    handler(&mut *world.borrow_mut(), args)
                })
            });
        self.starters
            .iter()
            .fold(natives, |natives, (name, starter)| {
                let world = Rc::clone(world);
                natives.reaching(PACKAGE, name, move |running, args| {
                    let work = starter(args)?;
                    world.borrow_mut().begin(running.later()?, work);
                    Ok(Value::Null)
                })
            })
    }
}

/// HTTP for a real process: what `host/network.rs` is, as the rest of
/// the host reaches it.
pub(super) trait Http {
    /// Whether a thread may yet send something a task waits for.
    fn waits(&self) -> bool;
    fn send(&mut self, ticket: u64, request: Request);
    /// Sends a request for the head of its response, trusting whom it
    /// says. Why it trusts no one, if what it says cannot be read.
    fn open(
        &mut self,
        ticket: u64,
        request: Request,
        trusted: Option<Trust>,
    ) -> Result<(), Failure>;
    /// Whether a response is still waited for, which it then is not.
    fn sent(&mut self, ticket: u64) -> bool;
    fn listen(&mut self, host: &str, port: i64) -> Result<Listening, Failure>;
    /// Takes the next request a server's thread brought, for whatever
    /// waits to accept one.
    fn arrived(&mut self) -> Option<(u64, Done)>;
    /// The next request of a server, if one is there or none will be;
    /// otherwise the ticket waits for one.
    fn accept(&mut self, ticket: u64, server: i64) -> Option<Done>;
    fn respond(&mut self, incoming: i64, response: Response) -> Result<(), Failure>;
    /// Lets go of a request that was not answered.
    fn abandon(&mut self, incoming: i64);
    /// Stops a server, and gives the tickets that waited to accept.
    fn close(&mut self, server: i64) -> Vec<u64>;
    fn forsake(&mut self, ticket: u64);
}

/// Watching files for a real process: what `host/watching.rs` is, as
/// the rest of the host reaches it.
pub(super) trait Watching {
    /// Whether a thread may yet send something a task waits for.
    fn waits(&self) -> bool;
    fn watch(&mut self, path: &str, sender: Sender<Sent>) -> Result<i64, Failure>;
    /// The next change, or nothing if the watcher is closed; pending
    /// if the ticket waits for one.
    fn next(&mut self, ticket: u64, id: i64) -> Poll<Option<Change>>;
    /// Hands a change the system told of to a ticket that waits, or
    /// keeps it. No change is word that one was missed.
    fn came(&mut self, id: i64, change: Option<Change>) -> Option<(u64, Change)>;
    /// Stops watching, and gives the tickets that waited for a change.
    fn close(&mut self, id: i64) -> Vec<u64>;
    fn forsake(&mut self, ticket: u64);
}

/// What makes HTTP for a process, given what its threads send through.
type MakeHttp = fn(Sender<Sent>) -> Box<dyn Http>;

/// The groups a link named: what makes each, which is all that refers
/// to its code.
#[derive(Default)]
pub struct Linked {
    pub(super) http: Option<MakeHttp>,
    pub(super) watching: Option<fn() -> Box<dyn Watching>>,
    /// What adds the handlers of a group that asks nothing of a world.
    pub(super) natives: Vec<fn(Natives) -> Natives>,
    /// The handlers of the groups that ask one.
    pub(super) offers: Vec<&'static Offer>,
}

impl Linked {
    /// Every group: what a process has that was linked for no one
    /// program.
    #[must_use]
    pub fn whole() -> Self {
        let mut linked = Self::default();
        for group in GROUPS {
            (group.link)(&mut linked);
        }
        linked
    }
}

/// Why a process refuses what a group it was linked without would do.
/// No build makes one that is asked: a link names every group its
/// program reaches.
pub(super) fn unlinked() -> Failure {
    Failure {
        kind: Kind::Failed,
        message: "this program was linked without what that takes".to_owned(),
    }
}

/// A part of the native side, which a link leaves out of a program
/// that reaches none of it.
pub struct Group {
    /// The name of [`Group::link`], as a link's `main` refers to it.
    pub symbol: &'static str,
    /// What begins the path of each extern of the group.
    externs: &'static [&'static str],
    link: extern "C" fn(&mut Linked),
}

/// Declares each group once: the function a link names, and its place
/// in [`GROUPS`] under that function's name.
macro_rules! groups {
    ($($(#[$doc:meta])* $symbol:ident for $externs:expr => |$linked:ident| $link:expr;)*) => {
        $(
            $(#[$doc])*
            #[unsafe(no_mangle)]
            pub extern "C" fn $symbol($linked: &mut Linked) {
                $link;
            }
        )*

        /// Every group of the command-line target's native side.
        pub const GROUPS: &[Group] = &[$(Group {
            symbol: stringify!($symbol),
            externs: $externs,
            link: $symbol,
        }),*];
    };
}

/// What begins the work of `fs.change`.
const CHANGING: Offer = Offer {
    handlers: &files::WATCHING,
    starters: &[("fs_change", |args| {
        Ok(super::Work::Change(files::watcher(args)?))
    })],
};

const FILES: Offer = Offer {
    handlers: &files::HANDLERS,
    starters: &[],
};

const HTTP: Offer = Offer {
    handlers: &http::HANDLERS,
    starters: &http::STARTERS,
};

const NET: Offer = Offer {
    handlers: &net::HANDLERS,
    starters: &net::STARTERS,
};

// A group is found by what begins an extern's path, the first that
// does: one whose externs begin as another's do is listed before it.
groups! {
    /// Links `system.console`.
    amb_system_console for &["console_"] => |linked| linked.offers.push(&plain::CONSOLE);
    /// Links `system.env`.
    amb_system_env for &["env_"] => |linked| linked.offers.push(&plain::ENV);
    /// Links `system.random`.
    amb_system_random for &["random_"] => |linked| linked.offers.push(&plain::RANDOM);
    /// Links `system.clock`.
    amb_system_clock for &["clock_"] => |linked| linked.offers.push(&plain::CLOCK);
    /// Links `system.process`.
    amb_system_process for &["process_"] => |linked| linked.offers.push(&plain::PROCESS);
    /// Links the watching of files of `system.fs`.
    amb_system_watching for &["fs_watch", "fs_unwatch", "fs_change"] => |linked| {
        linked.watching = Some(|| Box::<watching::Watches>::default());
        linked.offers.push(&CHANGING);
    };
    /// Links the rest of `system.fs`.
    amb_system_fs for &["fs_"] => |linked| linked.offers.push(&FILES);
    /// Links `system.http`: a client, its TLS, and a server.
    amb_system_http for &["http_"] => |linked| {
        linked.http = Some(|sender| Box::new(network::Network::new(sender)));
        linked.offers.push(&HTTP);
    };
    /// Links `system.net` and `system.stream`.
    amb_system_net for &["net_", "stream_"] => |linked| linked.offers.push(&NET);
    /// Links `system.crypto`.
    amb_system_crypto for &["crypto."] => |linked| linked.natives.push(crypto::offer);
    /// Links the loading of `system.code`.
    amb_system_code for &["code_"] => |linked| linked.natives.push(code::offer);
    /// Links `system.exchange`.
    amb_system_exchange for &["exchange."] => |linked| linked.natives.push(exchange::offer);
}

impl Group {
    /// What a link that named this group alone gave.
    #[must_use]
    pub fn linked(&self) -> Linked {
        let mut linked = Linked::default();
        (self.link)(&mut linked);
        linked
    }
}

/// The group the extern of the `system` package at `path` is of. One
/// that is of none is what starts a program's to supply
/// (`launch::started`), or the compiler's.
#[must_use]
pub fn group(path: &str) -> Option<&'static Group> {
    let holds = |group: &&Group| group.externs.iter().any(|begins| path.starts_with(begins));
    GROUPS.iter().find(holds)
}

#[cfg(test)]
mod tests {
    use super::{GROUPS, Linked, group};

    #[test]
    fn an_extern_is_of_the_group_its_module_is() {
        let symbol = |path| group(path).map(|group| group.symbol);

        assert_eq!(symbol("http_send"), Some("amb_system_http"));
        assert_eq!(symbol("fs_change"), Some("amb_system_watching"));
        assert_eq!(symbol("fs_write"), Some("amb_system_fs"));
        assert_eq!(symbol("stream_read"), Some("amb_system_net"));
        assert_eq!(symbol("crypto.hmac"), Some("amb_system_crypto"));
        assert_eq!(symbol("thrown"), None);
    }

    #[test]
    fn a_process_linked_for_no_one_program_has_every_group() {
        let (none, whole) = (Linked::default(), Linked::whole());
        let count = |linked: &Linked| linked.natives.len() + linked.offers.len();

        assert!(none.http.is_none() && none.watching.is_none());
        assert!(whole.http.is_some() && whole.watching.is_some());
        assert_eq!((count(&none), count(&whole)), (0, GROUPS.len()));
    }
}
