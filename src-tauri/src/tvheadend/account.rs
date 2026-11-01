//! What a Tvheadend account is: the server, and who to log in as.

use std::fmt;

#[derive(Clone, Default, PartialEq, Eq)]
pub struct TvhAccount {
    /// `http://host:9981`, as typed. Read by [`super::client::base_url`].
    pub url: String,
    /// Empty for a server that lets anyone stream (an access entry for `*`),
    /// in which case the password is ignored and none is sent.
    pub username: String,
    pub password: String,
}

impl TvhAccount {
    pub fn has_login(&self) -> bool {
        !self.username.trim().is_empty()
    }
}

impl fmt::Debug for TvhAccount {
    /// The same rule as the Xtream credentials: the address is useful in a
    /// log, the login is not.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TvhAccount")
            .field("url", &crate::m3u::redact::redact_url(&self.url).0)
            .field("username", &if self.has_login() { "<redacted>" } else { "" })
            .field("password", &"<redacted>")
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_output_never_carries_the_login() {
        let a = TvhAccount {
            url: "http://tvh.lan:9981".into(),
            username: "bob".into(),
            password: "hunter2".into(),
        };
        let s = format!("{a:?}");
        assert!(!s.contains("bob") && !s.contains("hunter2"), "{s}");
        assert!(s.contains("tvh.lan"), "{s}");
    }
}
