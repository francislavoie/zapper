//! What an M3U account is: where the playlist lives, where the guide lives,
//! and what to call ourselves when asking for either.

use std::fmt;

#[derive(Clone, Default, PartialEq, Eq)]
pub struct M3uAccount {
    /// An `http(s)://` URL, a `file://` URL, or a plain path on this machine.
    pub playlist: String,
    /// XMLTV locations, URL or path, tried in order. Empty means "use what
    /// the playlist's own `#EXTM3U x-tvg-url` names, if anything".
    pub epg_urls: Vec<String>,
    /// Sent when fetching the playlist and the guide, and to mpv for any
    /// channel whose entry does not name its own. `None` keeps the default,
    /// which is the VLC agent the Xtream side already uses: providers that
    /// filter agents at all tend to let VLC through.
    pub user_agent: Option<String>,
}

impl fmt::Debug for M3uAccount {
    /// Provider playlists carry the account in the URL —
    /// `get.php?username=…&password=…` is the common shape — so the URLs go
    /// through the same redaction as everything else that might reach a log.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("M3uAccount")
            .field("playlist", &super::redact::redact_url(&self.playlist).0)
            .field(
                "epg_urls",
                &self
                    .epg_urls
                    .iter()
                    .map(|u| super::redact::redact_url(u).0)
                    .collect::<Vec<_>>(),
            )
            .field("user_agent", &self.user_agent)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_output_never_carries_the_password_in_the_playlist_url() {
        let a = M3uAccount {
            playlist: "http://p.example/get.php?username=bob&password=hunter2&type=m3u_plus".into(),
            epg_urls: vec!["http://bob:hunter2@epg.example/guide.xml.gz".into()],
            user_agent: None,
        };
        let s = format!("{a:?}");
        assert!(!s.contains("hunter2"), "password leaked: {s}");
        assert!(!s.contains("bob"), "username leaked: {s}");
        assert!(s.contains("p.example"), "host is useful, keep it: {s}");
    }
}
