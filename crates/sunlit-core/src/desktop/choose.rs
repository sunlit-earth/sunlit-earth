//! Which setter this session gets, and on what evidence.
//!
//! A pure function over [`Session`], which is every question the choice asks of
//! the machine it runs on. The live answers are in `probe.rs`; the tests answer
//! from a table, which is what lets the whole choice be checked on a machine
//! with no Linux desktop at all.

use super::{BACKENDS, Backend, DESKTOP_ENV};

/// The variables walked, in order, for the name a session gives itself.
///
/// The second and third only when the first is empty: a display manager that
/// starts a bare window manager often exports no `XDG_CURRENT_DESKTOP` and still
/// names the session it started in one of the others.
const NAME_SOURCES: [&str; 3] = [DESKTOP_ENV, "XDG_SESSION_DESKTOP", "DESKTOP_SESSION"];

/// What the choice needs to know about the session it is choosing for.
///
/// Each question is asked only when a rung reaches it, so a session decided by
/// its first token asks for nothing else.
pub trait Session {
    /// An environment variable, blank being the same as unset.
    fn var(&self, name: &str) -> Option<String>;
    /// Whether a program of this name is on `PATH`.
    fn on_path(&self, program: &str) -> bool;
}

/// The setter a session gets, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub backend: Backend,
    /// The facts the choice rests on, in the order they were found.
    pub evidence: Vec<String>,
}

impl Choice {
    /// The evidence as one sentence, for a log line or a report.
    pub fn explanation(&self) -> String {
        format!("{}: {}", self.backend.desktop, self.evidence.join("; "))
    }
}

/// One way of setting the wallpaper that was considered and did not apply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Declined {
    /// What was tried: a desktop row or a rung of the ladder.
    pub rung: String,
    pub reason: String,
}

/// No setter for this session, with everything that was tried.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub declined: Vec<Declined>,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "no way to set the wallpaper was found in this session")?;
        for declined in &self.declined {
            write!(f, "; {}: {}", declined.rung, declined.reason)?;
        }
        let known: Vec<&str> = BACKENDS.iter().map(|(_, b)| b.desktop).collect();
        write!(
            f,
            ". The desktops with a setter here are {}.",
            known.join(", ")
        )
    }
}

/// The setter for `session`, or every reason there is none.
pub fn choose(session: &dyn Session) -> Result<Choice, Refusal> {
    let mut declined = Vec::new();
    if let Some(choice) = from_names(session, &mut declined) {
        return Ok(choice);
    }
    Err(Refusal { declined })
}

/// The first row of the table the session names itself as, whose setter is
/// there.
///
/// A row whose setter is missing does not end the walk: the next token may name
/// a desktop that can do the job, and the refusal lists the row either way.
fn from_names(session: &dyn Session, declined: &mut Vec<Declined>) -> Option<Choice> {
    let mut named_any = false;
    for source in NAME_SOURCES {
        let Some(value) = session.var(source) else {
            continue;
        };
        for token in tokens(&value) {
            let Some(backend) = row_for(&token) else {
                continue;
            };
            named_any = true;
            let mut evidence = vec![format!("{source}={value} names {}", backend.desktop)];
            match backend.program {
                Some(program) if !session.on_path(program) => {
                    declined.push(Declined {
                        rung: backend.desktop.to_owned(),
                        reason: format!(
                            "this is {desktop}, whose wallpaper is set with `{program}`, and \
                             that program is not on PATH",
                            desktop = backend.desktop,
                        ),
                    });
                    continue;
                }
                Some(program) => evidence.push(format!("{program} on PATH")),
                None => {}
            }
            return Some(Choice {
                backend: *backend,
                evidence,
            });
        }
        if source == DESKTOP_ENV {
            break;
        }
    }
    if !named_any {
        declined.push(Declined {
            rung: "desktop table".to_owned(),
            reason: names_nothing(session),
        });
    }
    None
}

/// Why the session's own name picked no row.
fn names_nothing(session: &dyn Session) -> String {
    let set: Vec<String> = NAME_SOURCES
        .iter()
        .filter_map(|name| session.var(name).map(|value| format!("{name}={value}")))
        .collect();
    if set.is_empty() {
        format!("{} are not set", NAME_SOURCES.join(", "))
    } else {
        format!("{} names no desktop with a row here", set.join(", "))
    }
}

/// The tokens of one variable, lower case, in the session's own order.
///
/// `DESKTOP_SESSION` is sometimes a path to the session file rather than its
/// name, so each token is reduced to its last path component.
fn tokens(value: &str) -> impl Iterator<Item = String> + '_ {
    value
        .split(':')
        .map(|token| {
            token
                .trim()
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .to_ascii_lowercase()
        })
        .filter(|token| !token.is_empty())
}

fn row_for(token: &str) -> Option<&'static Backend> {
    BACKENDS
        .iter()
        .find(|(names, _)| names.contains(&token))
        .map(|(_, backend)| backend)
}

#[cfg(test)]
pub(crate) mod fake {
    use std::collections::{HashMap, HashSet};

    use super::Session;

    /// A session made of fabricated answers.
    #[derive(Default)]
    pub(crate) struct FakeSession {
        pub vars: HashMap<String, String>,
        pub programs: HashSet<String>,
        /// Every program is on `PATH`, whatever `programs` holds.
        pub every_program: bool,
    }

    impl FakeSession {
        pub(crate) fn named(desktop: &str) -> Self {
            Self::default().var(super::DESKTOP_ENV, desktop)
        }

        /// A session named `desktop` in which everything anything asks for is
        /// there, which is what the table's own tests assume.
        pub(crate) fn complete(desktop: &str) -> Self {
            Self {
                every_program: true,
                ..Self::named(desktop)
            }
        }

        pub(crate) fn var(mut self, name: &str, value: &str) -> Self {
            self.vars.insert(name.to_owned(), value.to_owned());
            self
        }

        pub(crate) fn program(mut self, name: &str) -> Self {
            self.programs.insert(name.to_owned());
            self
        }
    }

    impl Session for FakeSession {
        fn var(&self, name: &str) -> Option<String> {
            self.vars
                .get(name)
                .filter(|value| !value.trim().is_empty())
                .cloned()
        }

        fn on_path(&self, program: &str) -> bool {
            self.every_program || self.programs.contains(program)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::FakeSession;
    use super::*;

    fn chosen(session: &FakeSession) -> Option<&'static str> {
        choose(session).ok().map(|choice| choice.backend.desktop)
    }

    #[test]
    fn a_named_desktop_with_its_setter_is_chosen_with_the_evidence() {
        let session = FakeSession::named("ubuntu:GNOME").program("gsettings");
        let choice = choose(&session).expect("GNOME with gsettings");
        assert_eq!(choice.backend.desktop, "GNOME");
        let explanation = choice.explanation();
        assert!(explanation.contains("ubuntu:GNOME"), "{explanation}");
        assert!(explanation.contains("gsettings on PATH"), "{explanation}");
    }

    #[test]
    fn a_missing_setter_moves_on_to_the_next_token_and_is_listed_if_nothing_else_works() {
        let session = FakeSession::named("KDE:XFCE").program("xfconf-query");
        assert_eq!(chosen(&session), Some("XFCE"));

        let refusal = choose(&FakeSession::named("GNOME")).expect_err("no gsettings");
        let text = refusal.to_string();
        assert!(text.contains("`gsettings`"), "{text}");
        assert!(text.contains("not on PATH"), "{text}");
    }

    #[test]
    fn the_secondary_variables_are_read_only_when_the_first_is_empty() {
        let session = FakeSession::default()
            .var("XDG_SESSION_DESKTOP", "xfce")
            .program("xfconf-query");
        assert_eq!(chosen(&session), Some("XFCE"));

        let path = FakeSession::default()
            .var("DESKTOP_SESSION", "/usr/share/xsessions/plasma")
            .program("dbus-send");
        assert_eq!(chosen(&path), Some("KDE Plasma"));

        let named = FakeSession::named("Enlightenment")
            .var("XDG_SESSION_DESKTOP", "xfce")
            .program("xfconf-query");
        assert_eq!(chosen(&named), None);
    }

    #[test]
    fn a_session_that_names_nothing_known_says_what_it_did_name() {
        let refusal = choose(&FakeSession::named("Enlightenment")).expect_err("no row");
        let text = refusal.to_string();
        assert!(text.contains("XDG_CURRENT_DESKTOP=Enlightenment"), "{text}");
        assert!(text.contains("KDE Plasma"), "{text}");

        let unset = choose(&FakeSession::default())
            .expect_err("no name")
            .to_string();
        for name in NAME_SOURCES {
            assert!(unset.contains(name), "{unset}");
        }
    }
}
