//! The XDG desktop portal's wallpaper call, the last rung of the ladder.
//!
//! With `show-preview` false the portal asks once, stores the answer, and from
//! then on sets the wallpaper without a word or refuses without one. The app
//! registers under its own id first where the portal can take one, so that the
//! answer is this app's rather than one shared by every unidentified program.

use crate::desktop::APP_ID;

/// What a `Response` code means for this publish.
///
/// 2 is also what a stored denial answers with, before any dialog, which is
/// why that sentence says where the permission is reset.
pub(super) fn response(code: u32, registered: bool) -> Result<(), String> {
    match code {
        0 => Ok(()),
        1 => Err(
            "the portal's dialog was dismissed, so the wallpaper was not set; the next \
             publish asks again"
                .to_owned(),
        ),
        2 if registered => Err(format!(
            "the portal refused to set the wallpaper. A \"Deny\" in its dialog is \
             remembered and refused silently from then on; \
             `flatpak permission-remove wallpaper wallpaper {APP_ID}` resets it"
        )),
        2 => Err(
            "the portal refused to set the wallpaper. This portal has no Registry, so the \
             answer is the one every unidentified app shares; `flatpak permissions \
             wallpaper` shows it"
                .to_owned(),
        ),
        other => Err(format!(
            "the portal answered the wallpaper request with code {other}"
        )),
    }
}

/// The request object path the portal answers a call on, from the caller's
/// unique name and the token it chose.
pub(super) fn request_path(unique_name: &str, token: &str) -> String {
    let sender = unique_name.trim_start_matches(':').replace('.', "_");
    format!("/org/freedesktop/portal/desktop/request/{sender}/{token}")
}

#[cfg(target_os = "linux")]
pub(super) use live::publish;

#[cfg(target_os = "linux")]
mod live {
    use std::collections::HashMap;
    use std::path::Path;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::time::Duration;

    use zbus::blocking::{Connection, Proxy};
    use zbus::zvariant::{Fd, OwnedValue, Value};

    use super::{request_path, response};
    use crate::desktop::APP_ID;

    const BUS_NAME: &str = "org.freedesktop.portal.Desktop";
    const PATH: &str = "/org/freedesktop/portal/desktop";

    /// How long a publish waits for the portal before handing the wait to a
    /// thread: long enough for a stored answer, short of a person reading a
    /// dialog.
    const ANSWER: Duration = Duration::from_secs(5);

    /// The one connection the portal knows this app by, and whether it
    /// registered. `Register` must be a connection's first portal call, so the
    /// connection is kept rather than opened per publish.
    static CONNECTION: Mutex<Option<(Connection, bool)>> = Mutex::new(None);

    /// Whether an earlier request is still waiting on its dialog.
    static PENDING: AtomicBool = AtomicBool::new(false);

    static TOKENS: AtomicU64 = AtomicU64::new(0);

    fn connection() -> Result<(Connection, bool), String> {
        let mut held = CONNECTION
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(held) = held.as_ref() {
            return Ok(held.clone());
        }
        let conn = Connection::session()
            .map_err(|e| format!("the session bus could not be reached for the portal: {e}"))?;
        let options: HashMap<&str, Value<'_>> = HashMap::new();
        let registered = match conn.call_method(
            Some(BUS_NAME),
            PATH,
            Some("org.freedesktop.host.portal.Registry"),
            "Register",
            &(APP_ID, options),
        ) {
            Ok(_) => true,
            Err(e) => {
                tracing::info!(
                    "the portal did not take an app id ({e}); its wallpaper permission is \
                     the one every unidentified app shares"
                );
                false
            }
        };
        *held = Some((conn.clone(), registered));
        Ok((conn, registered))
    }

    /// Ask the portal to make `image` the background.
    ///
    /// `Ok` carries a note when the portal is still waiting on its dialog: the
    /// wallpaper changes once that is answered, and the answer is logged.
    pub(in crate::wallpaper::linux) fn publish(image: &Path) -> Result<String, String> {
        if PENDING.load(Ordering::SeqCst) {
            return Err(
                "the portal's permission dialog from an earlier publish has not been \
                 answered yet"
                    .to_owned(),
            );
        }
        let (conn, registered) = connection()?;
        let refused = |e: &dyn std::fmt::Display| format!("the portal's wallpaper call: {e}");
        let token = format!("sunlit_earth_{}", TOKENS.fetch_add(1, Ordering::SeqCst));
        let unique = conn
            .unique_name()
            .ok_or_else(|| refused(&"the bus connection has no name"))?
            .to_string();
        let request = Proxy::new(
            &conn,
            BUS_NAME,
            request_path(&unique, &token),
            "org.freedesktop.portal.Request",
        )
        .map_err(|e| refused(&e))?;
        let mut responses = request
            .receive_signal("Response")
            .map_err(|e| refused(&e))?;

        let file = std::fs::File::open(image)
            .map_err(|e| format!("cannot open {}: {e}", image.display()))?;
        let mut options: HashMap<&str, Value<'_>> = HashMap::new();
        options.insert("handle_token", Value::from(token.as_str()));
        options.insert("show-preview", Value::from(false));
        options.insert("set-on", Value::from("background"));
        conn.call_method(
            Some(BUS_NAME),
            PATH,
            Some("org.freedesktop.portal.Wallpaper"),
            "SetWallpaperFile",
            &("", Fd::from(&file), options),
        )
        .map_err(|e| refused(&e))?;
        drop(file);

        let (tx, rx) = crossbeam_channel::bounded::<u32>(1);
        PENDING.store(true, Ordering::SeqCst);
        std::thread::Builder::new()
            .name("portal-response".to_owned())
            .spawn(move || {
                let code = responses.next().and_then(|message| {
                    message
                        .body()
                        .deserialize::<(u32, HashMap<String, OwnedValue>)>()
                        .ok()
                        .map(|(code, _)| code)
                });
                PENDING.store(false, Ordering::SeqCst);
                if let Some(code) = code
                    && tx.try_send(code).is_err()
                {
                    match response(code, registered) {
                        Ok(()) => tracing::info!("the portal set the wallpaper"),
                        Err(e) => tracing::warn!("{e}"),
                    }
                }
            })
            .map_err(|e| refused(&e))?;

        match rx.recv_timeout(ANSWER) {
            Ok(code) => response(code, registered).map(|()| String::new()),
            Err(_) => Ok(
                "the portal is asking whether Sunlit Earth may set the background; the \
                 wallpaper changes once that is answered"
                    .to_owned(),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn success_is_the_only_code_that_sets_a_wallpaper() {
        assert_eq!(response(0, true), Ok(()));
        for code in [1, 2, 3] {
            assert!(response(code, true).is_err(), "{code}");
        }
    }

    #[test]
    fn a_denial_says_where_the_permission_is_reset() {
        let registered = response(2, true).expect_err("denied");
        assert!(registered.contains("flatpak permission-remove wallpaper wallpaper sunlit-earth"));
        let shared = response(2, false).expect_err("denied");
        assert!(shared.contains("every unidentified app"), "{shared}");
        let dismissed = response(1, true).expect_err("dismissed");
        assert!(dismissed.contains("asks again"), "{dismissed}");
    }

    #[test]
    fn the_request_path_follows_the_senders_name() {
        assert_eq!(
            request_path(":1.42", "sunlit_earth_0"),
            "/org/freedesktop/portal/desktop/request/1_42/sunlit_earth_0"
        );
    }
}
