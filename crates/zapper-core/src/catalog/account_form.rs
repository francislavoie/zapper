//! Settings → Account's form, wherever it is filled in: an app's own
//! window (`player::api::save_account`) and a browser signed in to the
//! recording service (`recording::account`, `/api/account`). One set of rules for
//! both, so the two cannot disagree about what Save keeps.
//!
//! **Secrets are write-only.** [`view`] says whether a password is on file,
//! never what it is, and shows a playlist or guide address with any login
//! in it redacted. A form sends a blank password (or `null` for a redacted
//! address) to mean "keep the one on file", and the blanks are filled here,
//! from the file, the same way for a save and for a test. Nothing here logs
//! or formats a value it was given.
//!
//! **Errors are scrubbed** of what a URL carries ([`sync_engine::scrub`]),
//! and a test's answer has the typed password itself masked out as well
//! ([`mask_secret`]): it is the one secret that arrives here having never
//! been stored, so nothing knows where else it might turn up.

use crate::catalog::credentials::{self, account_path, on_file, summary, Account, Kind, Source};
use crate::catalog::sync_engine;
use crate::api::types::{AccountCandidateDto, AccountDto};
use crate::m3u::redact::redact_url;

/// The account as a form may see it: every kind's fields, no secret.
pub fn view() -> AccountDto {
    let (creds, source) = summary();
    let f = on_file();
    let m3u = f.m3u.unwrap_or_default();
    let (playlist, playlist_redacted) = redact_url(&m3u.playlist);
    let epg: Vec<(String, bool)> = m3u.epg_urls.iter().map(|u| redact_url(u)).collect();
    let tuner = f.hdhomerun.unwrap_or_default();
    let tuner_epg: Vec<(String, bool)> = tuner.epg_urls.iter().map(|u| redact_url(u)).collect();
    let tvh = f.tvheadend.unwrap_or_default();
    AccountDto {
        hdhomerun_device: tuner.device,
        hdhomerun_epg_redacted: tuner_epg.iter().any(|(_, r)| *r),
        hdhomerun_epg_urls: tuner_epg.into_iter().map(|(u, _)| u).collect(),
        // Checked at save to carry no login; redacted anyway, for a file
        // edited by hand.
        tvheadend_url: redact_url(&tvh.url).0,
        tvheadend_has_password: !tvh.password.is_empty(),
        tvheadend_username: tvh.username,
        kind: f.kind.as_str().to_owned(),
        playlist,
        playlist_redacted,
        epg_redacted: epg.iter().any(|(_, r)| *r),
        epg_urls: epg.into_iter().map(|(u, _)| u).collect(),
        user_agent: m3u.user_agent.unwrap_or_default(),
        // A host typed with a login in it (`http://user:pass@…`) is shown
        // without it, and sent back as shown keeps the one on file.
        host: creds
            .as_ref()
            .map(|c| redact_url(&c.host).0)
            .unwrap_or_default(),
        username: creds
            .as_ref()
            .map(|c| c.username.clone())
            .unwrap_or_default(),
        has_password: creds.as_ref().is_some_and(|c| !c.password.is_empty()),
        source: match source {
            Source::Zapper => "zapper",
            Source::None => "none",
        }
        .to_owned(),
        path: account_path().display().to_string(),
    }
}

/// Writes the form as the account in use, its blanks filled from the file.
/// The kind it names becomes the one in use; the other kinds' fields stay
/// on file as they were.
///
/// What follows is the caller's: a sync, and in the app, dropping the client
/// it keeps.
pub fn save(candidate: AccountCandidateDto) -> Result<Kind, String> {
    let account = resolve_candidate(candidate, on_file())?;
    let kind = account.kind();
    let written = match &account {
        Account::Xtream(c) => credentials::save(c),
        Account::M3u(m) => credentials::save_m3u(m),
        Account::HdHomeRun(h) => credentials::save_hdhomerun(h),
        Account::Tvheadend(t) => credentials::save_tvheadend(t),
    };
    // The path and the system's reason; nothing of what was being written.
    written.map_err(|e| sync_engine::scrub(&format!("{e:#}")))?;
    Ok(kind)
}

/// Tests the form as typed, without saving it: the Test button checks the
/// form, so a change can be tried before it replaces the account in use.
///
/// Nothing is written and nothing in use is touched — not the account file,
/// not a cached client, not the catalogue, and no sync is asked for. Blank
/// secret fields are filled from the file exactly as [`save`] fills them,
/// so what is tested is what Save would then keep.
pub async fn test(candidate: AccountCandidateDto) -> Result<String, String> {
    let account = resolve_candidate(candidate, on_file())?;
    test_unsaved(account).await
}

/// The connection test of the account on file, with a client of its own.
pub async fn test_saved() -> Result<String, String> {
    let source =
        crate::source::Source::load().map_err(|e| sync_engine::scrub(&format!("{e:#}")))?;
    probe_source(&source).await
}

/// The connection test for any source, saved or not.
pub async fn probe(source: &crate::source::Source) -> Result<String, String> {
    probe_source(source).await
}

/// The password a candidate carries, if any: what [`mask_secret`] takes
/// out of anything said about it.
pub fn typed_secret(candidate: &AccountCandidateDto) -> &str {
    match candidate {
        AccountCandidateDto::Xtream { password, .. }
        | AccountCandidateDto::Tvheadend { password, .. } => password,
        AccountCandidateDto::M3u { .. } | AccountCandidateDto::Hdhomerun { .. } => "",
    }
}

/// The credentials an Xtream form saves. Pure, so the keep-the-password
/// rule is shared with the connection test and checked without a file.
fn resolve_xtream_form(
    existing: Option<crate::xtream::Credentials>,
    host: String,
    username: String,
    password: String,
) -> Result<crate::xtream::Credentials, String> {
    let mut host = host.trim().to_owned();
    let username = username.trim().to_owned();
    if host.is_empty() {
        return Err("a host is required".to_owned());
    }
    if username.is_empty() {
        return Err("a username is required".to_owned());
    }
    // The host as [`view`] showed it, sent back untouched: the one on file,
    // login and all.
    if let Some(on_file) = existing.as_ref().map(|c| &c.host) {
        if host.contains("<redacted>") && host == redact_url(on_file).0 {
            host = on_file.clone();
        }
    }
    if host.contains("<redacted>") {
        return Err("type the full address, including the parts shown as <redacted>".to_owned());
    }
    let password = if password.is_empty() {
        match existing.map(|c| c.password).filter(|p| !p.is_empty()) {
            Some(p) => p,
            None => return Err("a password is required".to_owned()),
        }
    } else {
        password
    };
    Ok(crate::xtream::Credentials {
        host,
        username,
        password,
    })
}

fn resolve_hdhomerun_form(
    existing: Option<crate::hdhomerun::HdhrAccount>,
    device: String,
    epg_urls: Option<Vec<String>>,
) -> Result<crate::hdhomerun::HdhrAccount, String> {
    let device = device.trim().to_owned();
    crate::hdhomerun::device::base_url(&device).map_err(|e| e.to_string())?;
    let epg_urls = match epg_urls {
        Some(list) => list
            .into_iter()
            .map(|u| u.trim().to_owned())
            .filter(|u| !u.is_empty())
            .collect(),
        None => existing.unwrap_or_default().epg_urls,
    };
    if epg_urls.iter().any(|u| u.contains("<redacted>")) {
        return Err("type the full address, including the parts shown as <redacted>".to_owned());
    }
    Ok(crate::hdhomerun::HdhrAccount { device, epg_urls })
}

fn resolve_tvheadend_form(
    existing: Option<crate::tvheadend::TvhAccount>,
    url: String,
    username: String,
    password: String,
) -> Result<crate::tvheadend::TvhAccount, String> {
    let url = url.trim().to_owned();
    crate::tvheadend::client::base_url(&url).map_err(|e| e.to_string())?;
    let username = username.trim().to_owned();
    let password = if username.is_empty() {
        String::new()
    } else if password.is_empty() {
        match existing.map(|a| a.password).filter(|p| !p.is_empty()) {
            Some(p) => p,
            None => {
                return Err(
                    "a password is required with a username (leave both empty if the server needs no login)"
                        .to_owned(),
                )
            }
        }
    } else {
        password
    };
    Ok(crate::tvheadend::TvhAccount {
        url,
        username,
        password,
    })
}

/// The account `set_m3u_account` would save. Pure, so the keep-or-replace
/// rules can be tested without an account file.
fn resolve_m3u_form(
    existing: Option<crate::m3u::M3uAccount>,
    playlist: Option<String>,
    epg_urls: Option<Vec<String>>,
    user_agent: String,
) -> Result<crate::m3u::M3uAccount, String> {
    let existing = existing.unwrap_or_default();
    let playlist = match playlist
        .map(|p| p.trim().to_owned())
        .filter(|p| !p.is_empty())
    {
        Some(p) => p,
        None if !existing.playlist.is_empty() => existing.playlist,
        None => return Err("a playlist URL or file is required".to_owned()),
    };
    let epg_urls = match epg_urls {
        Some(list) => list
            .into_iter()
            .map(|u| u.trim().to_owned())
            .filter(|u| !u.is_empty())
            .collect(),
        None => existing.epg_urls,
    };
    // A redacted value sent back verbatim would replace the real one with
    // `<redacted>`; refuse it rather than save something that cannot work.
    if playlist.contains("<redacted>") || epg_urls.iter().any(|u| u.contains("<redacted>")) {
        return Err("type the full address, including the parts shown as <redacted>".to_owned());
    }
    let user_agent = Some(user_agent.trim().to_owned()).filter(|u| !u.is_empty());
    Ok(crate::m3u::M3uAccount {
        playlist,
        epg_urls,
        user_agent,
    })
}

/// What a candidate means once its blanks are filled from the file.
fn resolve_candidate(
    candidate: AccountCandidateDto,
    on_file: credentials::OnFile,
) -> Result<Account, String> {
    use AccountCandidateDto as C;
    Ok(match candidate {
        C::Xtream {
            host,
            username,
            password,
        } => Account::Xtream(resolve_xtream_form(
            on_file.xtream,
            host,
            username,
            password,
        )?),
        C::M3u {
            playlist,
            epg_urls,
            user_agent,
        } => Account::M3u(resolve_m3u_form(
            on_file.m3u,
            playlist,
            epg_urls,
            user_agent,
        )?),
        C::Hdhomerun { device, epg_urls } => {
            Account::HdHomeRun(resolve_hdhomerun_form(on_file.hdhomerun, device, epg_urls)?)
        }
        C::Tvheadend {
            url,
            username,
            password,
        } => Account::Tvheadend(resolve_tvheadend_form(
            on_file.tvheadend,
            url,
            username,
            password,
        )?),
    })
}

/// Probes an account that is not the one in use, with a client of its own
/// that is dropped afterwards.
///
/// Its answer is masked as well as scrubbed. The scrub finds secrets by
/// where they sit — a query parameter, a URL's login — and a candidate is
/// the one place a password arrives that the app has never stored, so its
/// literal value is taken out of whatever comes back too.
async fn test_unsaved(account: Account) -> Result<String, String> {
    let secret = match &account {
        Account::Xtream(c) => c.password.clone(),
        Account::Tvheadend(t) => t.password.clone(),
        Account::M3u(_) | Account::HdHomeRun(_) => String::new(),
    };
    let mask = |text: String| mask_secret(&text, &secret);
    let source = crate::source::Source::from_account(account)
        .map_err(|e| mask(crate::catalog::sync_engine::scrub(&format!("{e:#}"))))?;
    probe_source(&source).await.map(mask).map_err(mask)
}

/// `text` with every occurrence of `secret` replaced. Too short a secret is
/// left alone: masking every "ab" in a sentence would garble the message and
/// protect nothing a guess would not find.
pub fn mask_secret(text: &str, secret: &str) -> String {
    if secret.chars().count() < 4 {
        return text.to_owned();
    }
    text.replace(secret, "<redacted>")
}

/// The connection test for any source, saved or not.
async fn probe_source(source: &crate::source::Source) -> Result<String, String> {
    use crate::source::Source;
    let scrubbed = |e: anyhow::Error| crate::catalog::sync_engine::scrub(&format!("{e:#}"));
    let server = match source {
        Source::Xtream(server) => server,
        // A playlist has nobody to ask "who am I"; the honest test is
        // whether it can be fetched and parsed, and whether its guide answers.
        Source::M3u(m) => return m.probe().await.map_err(scrubbed),
        // A tuner says what it is and what it can tune.
        Source::HdHomeRun(h) => return h.probe().await.map_err(scrubbed),
        // A server is logged in to, and asked for the playlist this login
        // gets, which is also the persistent-authentication check. Its
        // refusals already say what to do; the chain behind them would only
        // repeat it.
        Source::Tvheadend(t) => {
            return t.probe().await.map_err(|e| {
                match e
                    .chain()
                    .find_map(|c| c.downcast_ref::<crate::source::Refused>())
                {
                    Some(r) => crate::catalog::sync_engine::scrub(&r.why),
                    None => scrubbed(e),
                }
            })
        }
    };
    let info = server
        .login()
        .await
        .map_err(|e| crate::catalog::sync_engine::scrub(&format!("{e:#}")))?;
    if !info.auth {
        // The server answered and said no, which is a different thing from
        // not answering and worth saying differently.
        let status = if info.status.is_empty() {
            "rejected".to_owned()
        } else {
            info.status.clone()
        };
        return Err(format!("the server rejected these credentials ({status})"));
    }
    let conns = match (info.active_cons, info.max_connections) {
        (Some(a), Some(m)) => format!(" · {a} of {m} connections in use"),
        _ => String::new(),
    };
    let status = if info.status.is_empty() {
        "Active".to_owned()
    } else {
        info.status
    };
    Ok(format!("{status}{conns}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on_file_playlist() -> crate::m3u::M3uAccount {
        crate::m3u::M3uAccount {
            playlist: "http://p.example/get.php?username=u&password=pw".into(),
            epg_urls: vec!["http://p.example/xmltv.php?username=u&password=pw".into()],
            user_agent: Some("Old/1".into()),
        }
    }

    #[test]
    fn an_untouched_redacted_playlist_and_guide_are_kept() {
        let got = resolve_m3u_form(Some(on_file_playlist()), None, None, "".into()).unwrap();
        assert_eq!(got.playlist, on_file_playlist().playlist);
        assert_eq!(got.epg_urls, on_file_playlist().epg_urls);
        // The agent is not secret, so it is always sent, and empty clears it.
        assert_eq!(got.user_agent, None);
    }

    #[test]
    fn typed_values_replace_and_an_empty_guide_list_clears() {
        let got = resolve_m3u_form(
            Some(on_file_playlist()),
            Some("  /home/me/tv.m3u ".into()),
            Some(vec![" ".into()]),
            " Kodi/21 ".into(),
        )
        .unwrap();
        assert_eq!(got.playlist, "/home/me/tv.m3u");
        assert!(got.epg_urls.is_empty());
        assert_eq!(got.user_agent.as_deref(), Some("Kodi/21"));
    }

    #[test]
    fn a_tuner_needs_an_address_and_keeps_a_redacted_guide_list() {
        assert!(resolve_hdhomerun_form(None, "  ".into(), None).is_err());
        let on_file = crate::hdhomerun::HdhrAccount {
            device: "10.0.0.7".into(),
            epg_urls: vec!["http://g.example/x.xml?key=k".into()],
        };
        let kept =
            resolve_hdhomerun_form(Some(on_file.clone()), " 10.0.0.8 ".into(), None).unwrap();
        assert_eq!(kept.device, "10.0.0.8");
        assert_eq!(kept.epg_urls, on_file.epg_urls);
        let cleared =
            resolve_hdhomerun_form(Some(on_file), "10.0.0.8".into(), Some(vec![])).unwrap();
        assert!(
            cleared.epg_urls.is_empty(),
            "empty means SiliconDust's guide"
        );
    }

    #[test]
    fn a_tvheadend_password_is_kept_when_left_empty_and_dropped_without_a_user() {
        let on_file = crate::tvheadend::TvhAccount {
            url: "http://tvh.lan:9981".into(),
            username: "viewer".into(),
            password: "pw".into(),
        };
        let kept = resolve_tvheadend_form(
            Some(on_file.clone()),
            "http://tvh.lan:9981".into(),
            "viewer".into(),
            "".into(),
        )
        .unwrap();
        assert_eq!(kept.password, "pw");
        let anon =
            resolve_tvheadend_form(Some(on_file), "tvh.lan:9981".into(), " ".into(), "x".into())
                .unwrap();
        assert_eq!((anon.username.as_str(), anon.password.as_str()), ("", ""));
        let err =
            resolve_tvheadend_form(None, "tvh.lan".into(), "viewer".into(), "".into()).unwrap_err();
        assert!(err.contains("password is required"), "{err}");
        let err = resolve_tvheadend_form(None, "http://a:b@tvh.lan".into(), "".into(), "".into())
            .unwrap_err();
        assert!(err.contains("own fields"), "{err}");
    }

    #[test]
    fn a_playlist_is_required_and_redacted_text_is_refused() {
        assert!(resolve_m3u_form(None, None, None, "".into()).is_err());
        let err = resolve_m3u_form(
            None,
            Some("http://p.example/get.php?username=<redacted>".into()),
            None,
            "".into(),
        )
        .unwrap_err();
        assert!(err.contains("redacted"), "{err}");
    }

    #[test]
    fn an_xtream_host_shown_redacted_and_sent_back_keeps_the_one_on_file() {
        let on_file = crate::xtream::Credentials {
            host: "http://me:hunter2@x.example:8080".into(),
            username: "u".into(),
            password: "pw".into(),
        };
        let shown = redact_url(&on_file.host).0;
        assert!(!shown.contains("hunter2"), "{shown}");
        let kept =
            resolve_xtream_form(Some(on_file.clone()), shown, "u".into(), "".into()).unwrap();
        assert_eq!(kept.host, on_file.host);
        assert_eq!(kept.password, "pw");
        // Redacted text that is not what was shown is not a value.
        let err = resolve_xtream_form(
            Some(on_file),
            "http://<redacted>@y.example".into(),
            "u".into(),
            "".into(),
        )
        .unwrap_err();
        assert!(err.contains("redacted"), "{err}");
        // A host with nothing to hide is shown and kept as typed.
        let plain = "http://x.example:8080";
        assert_eq!(redact_url(plain).0, plain);
    }
}

#[cfg(test)]
mod candidate_tests {
    //! The connection test of an account that has not been saved.
    //!
    //! Each test runs in a config directory of its own (`paths::with_test_root`)
    //! on a current-thread runtime, so the thread-local root holds for every
    //! await, and talks to a fake server on 127.0.0.1.

    use super::*;
    use crate::catalog::credentials::{self, Account, Kind};
    use crate::test_http::{serve, Reply, Request, Server};
    use AccountCandidateDto as C;

    fn in_own_config<T>(name: &str, f: impl FnOnce() -> T) -> T {
        let dir = std::env::temp_dir().join(format!(
            "zapper-candidate-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let out = crate::paths::with_test_root(&dir, f);
        let _ = std::fs::remove_dir_all(&dir);
        out
    }

    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(f)
    }

    /// An Xtream server that knows one user, with either of two passwords.
    fn xtream_server(status: u16) -> Server {
        serve(move |req: &Request, _| {
            if req.route() != "/player_api.php" {
                return Reply::new(404, "");
            }
            if status != 200 {
                return Reply::new(status, "");
            }
            let known = req.query("username") == Some("viewer")
                && matches!(
                    req.query("password"),
                    Some("typed-secret") | Some("saved-secret")
                );
            let body = if known {
                r#"{"user_info":{"auth":1,"status":"Active","active_cons":"1","max_connections":"2"}}"#
            } else {
                r#"{"user_info":{"auth":0}}"#
            };
            Reply::ok(body).header("Content-Type", "application/json")
        })
    }

    fn save_saved_xtream() {
        credentials::save(&crate::xtream::Credentials {
            host: "http://saved.invalid".into(),
            username: "viewer".into(),
            password: "saved-secret".into(),
        })
        .unwrap();
    }

    fn xtream(host: &str, password: &str) -> C {
        C::Xtream {
            host: host.into(),
            username: " viewer ".into(),
            password: password.into(),
        }
    }

    #[test]
    fn what_is_typed_is_tested_and_nothing_is_saved() {
        in_own_config("typed", || {
            save_saved_xtream();
            let server = xtream_server(200);
            let got = block_on(test(xtream(&server.base, "typed-secret")));
            assert_eq!(got.unwrap(), "Active · 1 of 2 connections in use");
            // The typed host and password were the ones asked, not the saved.
            let asked = server.requests();
            assert_eq!(asked.len(), 1);
            assert_eq!(asked[0].query("password"), Some("typed-secret"));
            // And the file is as it was.
            let f = credentials::on_file();
            assert_eq!(f.kind, Kind::Xtream);
            let saved = f.xtream.unwrap();
            assert_eq!(saved.host, "http://saved.invalid");
            assert_eq!(saved.password, "saved-secret");
        });
    }

    #[test]
    fn a_blank_password_tests_the_one_on_file() {
        in_own_config("blank", || {
            save_saved_xtream();
            let server = xtream_server(200);
            let got = block_on(test(xtream(&server.base, "")));
            assert!(got.is_ok(), "{got:?}");
            assert_eq!(server.requests()[0].query("password"), Some("saved-secret"));
        });
    }

    #[test]
    fn a_blank_password_with_none_on_file_is_refused_before_asking() {
        in_own_config("none", || {
            let server = xtream_server(200);
            let err = block_on(test(xtream(&server.base, ""))).unwrap_err();
            assert!(err.contains("password is required"), "{err}");
            assert!(server.requests().is_empty());
        });
    }

    #[test]
    fn a_refusal_is_reported_without_the_password() {
        in_own_config("refused", || {
            let server = xtream_server(200);
            let err = block_on(test(xtream(&server.base, "wrong-secret"))).unwrap_err();
            assert!(err.contains("rejected"), "{err}");
            assert!(!err.contains("wrong-secret"), "{err}");
        });
    }

    #[test]
    fn a_failing_server_is_reported_without_the_password() {
        in_own_config("failing", || {
            let server = xtream_server(500);
            let err = block_on(test(xtream(&server.base, "typed-secret"))).unwrap_err();
            // reqwest quotes the URL it asked, and that URL carries the login.
            assert!(err.contains("500"), "{err}");
            assert!(!err.contains("typed-secret"), "{err}");
        });
    }

    #[test]
    fn another_kind_is_tested_without_switching_to_it() {
        in_own_config("kind", || {
            save_saved_xtream();
            let server = serve(|req, base| match req.route() {
                "/tv.m3u" => Reply::ok(format!(
                    "#EXTM3U\n#EXTINF:-1 group-title=\"News\",One\n{base}/1.ts\n\
                     #EXTINF:-1 group-title=\"News\",Two\n{base}/2.ts\n"
                )),
                _ => Reply::new(404, ""),
            });
            let got = block_on(test(C::M3u {
                playlist: Some(format!("{}/tv.m3u", server.base)),
                epg_urls: Some(vec![]),
                user_agent: String::new(),
            }))
            .unwrap();
            assert!(got.starts_with("Playlist OK: 2 channels"), "{got}");
            let f = credentials::on_file();
            assert_eq!(
                f.kind,
                Kind::Xtream,
                "testing a playlist made it the account"
            );
            assert!(f.m3u.is_none());
        });
    }

    #[test]
    fn blanks_are_filled_from_the_file_as_a_save_would() {
        let on_file = || credentials::OnFile {
            kind: Kind::Xtream,
            xtream: Some(crate::xtream::Credentials {
                host: "http://a.invalid".into(),
                username: "u".into(),
                password: "kept-secret".into(),
            }),
            m3u: Some(crate::m3u::M3uAccount {
                playlist: "http://p.invalid/get.php?username=u&password=pw".into(),
                epg_urls: vec!["http://p.invalid/xmltv.php?password=pw".into()],
                user_agent: None,
            }),
            hdhomerun: None,
            tvheadend: Some(crate::tvheadend::TvhAccount {
                url: "http://tvh.invalid:9981".into(),
                username: "viewer".into(),
                password: "tvh-secret".into(),
            }),
        };
        match resolve_candidate(xtream("http://b.invalid", ""), on_file()).unwrap() {
            Account::Xtream(c) => {
                assert_eq!(
                    (c.host.as_str(), c.username.as_str()),
                    ("http://b.invalid", "viewer")
                );
                assert_eq!(c.password, "kept-secret");
            }
            _ => panic!("not an Xtream account"),
        }
        match resolve_candidate(
            C::M3u {
                playlist: None,
                epg_urls: None,
                user_agent: "".into(),
            },
            on_file(),
        )
        .unwrap()
        {
            Account::M3u(m) => {
                assert!(m.playlist.ends_with("password=pw"));
                assert_eq!(m.epg_urls.len(), 1);
            }
            _ => panic!("not a playlist"),
        }
        match resolve_candidate(
            C::Tvheadend {
                url: "http://tvh.invalid:9981".into(),
                username: "viewer".into(),
                password: "".into(),
            },
            on_file(),
        )
        .unwrap()
        {
            Account::Tvheadend(t) => assert_eq!(t.password, "tvh-secret"),
            _ => panic!("not a Tvheadend server"),
        }
        let err = resolve_candidate(
            C::Hdhomerun {
                device: " ".into(),
                epg_urls: None,
            },
            on_file(),
        )
        .unwrap_err();
        assert!(!err.is_empty());
    }

    #[test]
    fn the_candidate_arrives_in_the_shape_the_pane_sends() {
        let c: C =
            serde_json::from_str(r#"{"kind":"hdhomerun","device":"10.0.0.7","epgUrls":null}"#)
                .unwrap();
        assert!(matches!(c, C::Hdhomerun { ref device, epg_urls: None } if device == "10.0.0.7"));
        let c: C = serde_json::from_str(
            r#"{"kind":"m3u","playlist":null,"epgUrls":["a"],"userAgent":"UA/1"}"#,
        )
        .unwrap();
        assert!(matches!(c, C::M3u { playlist: None, .. }));
        let c: C = serde_json::from_str(
            r#"{"kind":"tvheadend","url":"http://t","username":"","password":""}"#,
        )
        .unwrap();
        assert!(matches!(c, C::Tvheadend { .. }));
    }

    #[test]
    fn a_secret_is_masked_wherever_it_appears() {
        assert_eq!(
            mask_secret(
                "could not log in as viewer:hunter22 at /x/hunter22",
                "hunter22"
            ),
            "could not log in as viewer:<redacted> at /x/<redacted>"
        );
        // Too short to mask without garbling the message.
        assert_eq!(mask_secret("a bad answer", "a"), "a bad answer");
        assert_eq!(mask_secret("nothing to hide", ""), "nothing to hide");
    }
}
