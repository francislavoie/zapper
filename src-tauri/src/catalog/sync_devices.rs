//! The sync passes for the two account kinds that are machines on the
//! network: an HDHomeRun tuner and a Tvheadend server.
//!
//! Both are playlist-shaped once fetched — a stored stream URL per channel,
//! categories replaced wholesale, unseen channels soft-deleted — so both
//! write through `catalog::playlist`'s writer and reuse the playlist pass's
//! machinery in `sync_engine`: the same three phases, so the toolbar says
//! the same things; the same hand-off from the download to the channels
//! phase; the same streaming bridge into the parsers. What is here is only
//! what each fetches, and from where.
//!
//! Each pass takes the [`Mirror`] it writes to, so the tests below can run a
//! whole pass — fake server, real HTTP, real parsers — against a database of
//! their own.

use anyhow::{anyhow, Context, Result};
use std::io::Read;

use crate::catalog::origin;
use crate::catalog::playlist::{self, PlaylistStats, PlaylistWriter};
use crate::catalog::sync_engine::{self as engine, Landed, Mirror, PassOutcome, StatusSink};
use crate::hdhomerun::device::{self, Discover, LineupEntry};
use crate::hdhomerun::{guide, HdhrSource};
use crate::ipc::types::SyncPhase;
use crate::m3u::fetch;
use crate::tvheadend::client::Tags;
use crate::tvheadend::TvhSource;

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ----------------------------------------------------------------- HDHomeRun

/// The tuner's lineup (categories, then channels, as a playlist's), then
/// the guide: SiliconDust's, or the account's XMLTV if it names one.
pub(crate) async fn run_hdhomerun_pass<S: StatusSink, M: Mirror>(
    sink: &S,
    src: &HdhrSource,
    mirror: M,
) -> PassOutcome {
    let landed: Landed = std::sync::Mutex::new(None);
    engine::run_steps(
        sink,
        vec![
            (SyncPhase::Categories, Box::pin(step_lineup(src, &mirror, &landed))),
            (
                SyncPhase::Channels,
                Box::pin(engine::step_landed_channels(&mirror, &landed)),
            ),
            (SyncPhase::Epg, Box::pin(step_hdhomerun_guide(src, &mirror))),
        ],
    )
    .await
}

async fn step_lineup<M: Mirror>(src: &HdhrSource, mirror: &M, landed: &Landed) -> Result<String> {
    let r = async {
        let d = src.device().discover().await?;
        let lineup = src.device().lineup(&d).await?;
        mirror.write(|conn| land_lineup(conn, &d, &lineup))
    }
    .await;
    engine::hand_off(landed, r.as_ref().map(|(s, _)| s));
    let (s, protected) = r?;
    let mut msg = format!("{} categories", s.categories);
    if protected > 0 {
        msg.push_str(&format!(" · {protected} copy-protected channels not listed"));
    }
    Ok(msg)
}

/// A lineup into the mirror, and the tuner noted as its origin. Returns what
/// the write did and how many copy-protected entries were left out.
pub(crate) fn land_lineup(
    conn: &rusqlite::Connection,
    d: &Discover,
    lineup: &[LineupEntry],
) -> Result<(PlaylistStats, usize)> {
    let l = device::entries(&d.device_id, lineup);
    if l.entries.is_empty() {
        // Said here rather than left to the writer's "the playlist had no
        // channels", which would be the wrong word and no help.
        return Err(if l.protected > 0 {
            anyhow!(
                "every channel on this HDHomeRun ({}) is copy-protected, and none can play here",
                l.protected
            )
        } else {
            anyhow!("the HDHomeRun's lineup is empty: scan for channels in the HDHomeRun app first")
        });
    }
    let mut w = PlaylistWriter::begin(conn)?;
    w.set_origin(origin::of_hdhomerun(&d.device_id));
    for (e, identity) in l.entries {
        w.push_identified(e, &identity)?;
    }
    w.rank_groups(&device::group_order());
    let stats = w.finish(&crate::m3u::Header::default())?;
    origin::record(conn, &origin::of_hdhomerun(&d.device_id))?;
    Ok((stats, l.protected))
}

async fn step_hdhomerun_guide<M: Mirror>(src: &HdhrSource, mirror: &M) -> Result<String> {
    let xmltv = &src.account().epg_urls;
    if !xmltv.is_empty() {
        // A grabber's guide can be tens of megabytes and slow; the tuner's
        // own client gives up after thirty seconds, so this gets the
        // playlist client, which does not.
        let http = fetch::http_client(None)?;
        return engine::step_xmltv_guides(mirror, &http, xmltv).await;
    }
    let d = src.device().discover().await?;
    if d.device_auth.trim().is_empty() {
        return Err(anyhow!(
            "this HDHomeRun offers no SiliconDust guide token; add an XMLTV guide in Settings → Account"
        ));
    }
    let http = reqwest::Client::builder()
        .user_agent(concat!("zapper/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(std::time::Duration::from_secs(15))
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .context("building http client")?;
    let until = now() + guide::AHEAD_SECS;
    let mut start = None;
    let mut reached = now();
    let mut programmes = 0;
    for window in 0..guide::MAX_WINDOWS {
        let answer = match guide_window(&http, src.guide_api(), &d.device_auth, start).await {
            Ok(a) => a,
            // The first window is the guide; without it there is nothing to
            // show. A later one failing leaves a shorter guide, which is
            // still a guide.
            Err(e) if window == 0 => return Err(e),
            Err(e) => {
                tracing::warn!("a later guide window failed: {}", engine::scrub(&format!("{e:#}")));
                break;
            }
        };
        programmes += mirror.write(|conn| guide::land(conn, &answer))?;
        match guide::latest_end(&answer) {
            Some(end) if end > reached => {
                reached = end;
                start = Some(end);
            }
            _ => break,
        }
        if reached >= until {
            break;
        }
    }
    mirror.write(guide::own_guide_channels)?;
    engine::prune_guide(mirror);
    Ok(format!("{programmes} programmes from SiliconDust"))
}

/// One window of SiliconDust's guide. A refusal is said in words rather
/// than passed on as a status: it is about the guide service, not the
/// tuner, and the lineup it follows has already landed.
async fn guide_window(
    http: &reqwest::Client,
    api: &str,
    device_auth: &str,
    start: Option<i64>,
) -> Result<Vec<guide::GuideChannel>> {
    let resp = http
        .get(guide::url(api, device_auth, start))
        .send()
        .await
        .map_err(reqwest::Error::without_url)
        .context("could not reach SiliconDust's guide service")?;
    let status = resp.status();
    if !status.is_success() {
        return Err(anyhow!(
            "SiliconDust's guide service refused this tuner (HTTP {}); an XMLTV guide can be set instead",
            status.as_u16()
        ));
    }
    let body = resp
        .bytes()
        .await
        .map_err(reqwest::Error::without_url)
        .context("reading SiliconDust's guide")?;
    guide::parse(&body)
}

// ----------------------------------------------------------------- Tvheadend

/// The server's playlist (categories from its tags), then its XMLTV guide.
pub(crate) async fn run_tvheadend_pass<S: StatusSink, M: Mirror>(
    sink: &S,
    src: &TvhSource,
    mirror: M,
) -> PassOutcome {
    let landed: Landed = std::sync::Mutex::new(None);
    engine::run_steps(
        sink,
        vec![
            (
                SyncPhase::Categories,
                Box::pin(step_tvheadend_playlist(src, &mirror, &landed)),
            ),
            (
                SyncPhase::Channels,
                Box::pin(engine::step_landed_channels(&mirror, &landed)),
            ),
            (SyncPhase::Epg, Box::pin(step_tvheadend_guide(src, &mirror))),
        ],
    )
    .await
}

async fn step_tvheadend_playlist<M: Mirror>(
    src: &TvhSource,
    mirror: &M,
    landed: &Landed,
) -> Result<String> {
    let r = load_tvheadend_playlist(src, mirror).await;
    engine::hand_off(landed, r.as_ref());
    let s = r?;
    Ok(format!("{} categories", s.categories))
}

async fn load_tvheadend_playlist<M: Mirror>(src: &TvhSource, mirror: &M) -> Result<PlaylistStats> {
    let client = src.client();
    // Tags first: the playlist is written as it streams in, and each entry's
    // category is settled as it passes.
    let tags = client.tags().await.unwrap_or_default();
    let resp = client.playlist().await?;
    let from = origin::of_tvheadend(client.base(), &src.account().username);
    let m = mirror.clone();
    engine::pump_into(resp, "the channel playlist", None, move |r| {
        m.write(|conn| {
            let s = land_tvheadend_playlist(conn, r, &tags, &from)?;
            origin::record(conn, &from)?;
            Ok(s)
        })
    })
    .await
}

/// A Tvheadend playlist into the mirror.
///
/// Each channel is filed under its first tag ([`Tags`]); a playlist that
/// carries `group-title` (a proxy in front of the server, say) is honoured
/// where the tags say nothing; and the rest go under one category — `Other`
/// beside the tags, `Channels` when there are no tags at all.
pub(crate) fn land_tvheadend_playlist(
    conn: &rusqlite::Connection,
    src: Box<dyn Read + Send>,
    tags: &Tags,
    from: &str,
) -> Result<PlaylistStats> {
    let r = fetch::decompressing(src)?;
    let fallback = if tags.by_channel.is_empty() {
        "Channels"
    } else {
        "Other"
    };
    let mut w = PlaylistWriter::begin(conn)?;
    w.set_origin(from.to_owned());
    let header = crate::m3u::parse::parse(r, |mut e| {
        let uuid = e.tvg_id.clone();
        let tag = uuid.as_deref().and_then(|u| tags.by_channel.get(u)).cloned();
        e.group = tag.or(e.group.take()).or_else(|| Some(fallback.to_owned()));
        match uuid {
            Some(u) => w.push_identified(e, &crate::tvheadend::identity(&u)),
            None => {
                let identity = crate::m3u::ids::identity(&e);
                w.push_identified(e, &identity)
            }
        }
    })?;
    w.rank_groups(&tags.order);
    w.finish(&header)
}

async fn step_tvheadend_guide<M: Mirror>(src: &TvhSource, mirror: &M) -> Result<String> {
    let resp = src.client().get("/xmltv/channels", "the guide").await?;
    let programmes =
        engine::pump_into(resp, "the guide", None, engine::guide_parser(mirror.clone(), true))
            .await?;
    let (matched, total) = mirror.write(playlist::match_epg)?;
    engine::prune_guide(mirror);
    Ok(format!(
        "{programmes} programmes, guide for {matched} of {total} channels"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{db, status};
    use crate::hdhomerun::HdhrAccount;
    use crate::ipc::types::SyncStatusDto;
    use crate::test_http::{serve, Reply, Request, Server};
    use crate::tvheadend::client::{NEEDS_LOGIN, NEEDS_PERSISTENT_AUTH};
    use crate::tvheadend::digest;
    use crate::tvheadend::TvhAccount;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    /// A database of the test's own; see the module notes.
    #[derive(Clone)]
    struct TestMirror(Arc<Mutex<rusqlite::Connection>>);

    impl TestMirror {
        fn new() -> Self {
            Self(Arc::new(Mutex::new(crate::catalog::open_in_memory().unwrap())))
        }

        fn rows<T: rusqlite::types::FromSql>(&self, sql: &str) -> Vec<T> {
            let c = self.0.lock().unwrap();
            let mut s = c.prepare(sql).unwrap();
            let v = s.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
            v
        }
    }

    impl Mirror for TestMirror {
        fn write<R>(&self, f: impl FnOnce(&rusqlite::Connection) -> Result<R>) -> Result<R> {
            let c = self.0.lock().unwrap_or_else(|e| e.into_inner());
            f(&c)
        }

        // The search index is process-wide; rebuilding it from this database
        // would pull it out from under the search tests. The count is what
        // the phase reports.
        fn reindex(&self) -> Result<usize> {
            self.write(|c| {
                Ok(c.query_row(
                    "SELECT COUNT(*) FROM channels WHERE deleted_at IS NULL",
                    [],
                    |r| r.get::<_, i64>(0),
                )? as usize)
            })
        }
    }

    #[derive(Default)]
    struct Recorder(Mutex<Vec<SyncStatusDto>>);

    impl StatusSink for Recorder {
        fn publish(&self, s: SyncStatusDto) {
            self.0.lock().unwrap().push(s);
        }
    }

    impl Recorder {
        fn messages(&self) -> Vec<String> {
            self.0.lock().unwrap().iter().map(|s| s.message.clone()).collect()
        }
        fn phases(&self) -> Vec<SyncPhase> {
            self.0.lock().unwrap().iter().map(|s| s.phase).collect()
        }
    }

    // -------------------------------------------------- the fake HDHomeRun

    const DISCOVER: &str = include_str!("../../tests/fixtures/hdhomerun/discover.json");
    const LINEUP: &str = include_str!("../../tests/fixtures/hdhomerun/lineup.json");
    const TOKEN: &str = "fixtureDeviceAuthTOKEN0123456789";

    /// The tuner and SiliconDust's guide service on one port. The guide
    /// answers in twelve-hour windows from `Start` (or now), so a sync has to
    /// ask more than once to fill a day; `/auto/v4.1` is out of tuners.
    fn hdhomerun(guide_status: u16) -> Server {
        serve(move |req: &Request, base| match req.route() {
            "/discover.json" => Reply::ok(DISCOVER.replace(
                "http://192.168.1.50/lineup.json",
                &format!("{base}/lineup.json"),
            )),
            "/lineup.json" => Reply::ok(LINEUP),
            "/api/guide" => {
                if guide_status != 200 {
                    return Reply::new(guide_status, "{}");
                }
                assert_eq!(req.query("DeviceAuth"), Some(TOKEN));
                let start = req
                    .query("Start")
                    .map(|s| s.parse::<i64>().unwrap())
                    .unwrap_or_else(|| now() - 600);
                let end = start + 12 * 3600;
                Reply::ok(
                    serde_json::json!([
                        {"GuideNumber": "4.1", "GuideName": "KXYZ-HD", "Guide": [
                            {"StartTime": start, "EndTime": start + 3600, "Title": "News", "Synopsis": "Local."},
                            {"StartTime": start + 3600, "EndTime": end, "Title": "Movie"}
                        ]},
                        {"GuideNumber": "5.1", "GuideName": "KWVU-HD", "Guide": [
                            {"StartTime": start, "EndTime": end, "Title": "Sport"}
                        ]}
                    ])
                    .to_string(),
                )
            }
            "/auto/v4.1" => {
                Reply::new(503, "").header("X-HDHomeRun-Error", "805 All Tuners In Use")
            }
            "/guide.xml" => Reply::ok(
                r#"<tv><channel id="KXYZ.us"><display-name>KXYZ-HD</display-name></channel>
                   <programme start="20251101180000 +0000" stop="20251101190000 +0000" channel="KXYZ.us"><title>From XMLTV</title></programme></tv>"#,
            ),
            _ => Reply::new(404, ""),
        })
    }

    fn tuner_at(server: &Server, epg_urls: Vec<String>) -> HdhrSource {
        HdhrSource::new(HdhrAccount {
            device: server.base.trim_start_matches("http://").to_owned(),
            epg_urls,
        })
        .unwrap()
        .with_guide_api(&server.base)
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn an_hdhomerun_syncs_end_to_end() {
        let _guard = status::test_lock();
        db::install_test_db();
        let server = hdhomerun(200);
        let src = tuner_at(&server, vec![]);
        let mirror = TestMirror::new();
        let rec = Recorder::default();

        let outcome = run_hdhomerun_pass(&rec, &src, mirror.clone()).await;
        assert_eq!(outcome, PassOutcome::Refreshed, "{:?}", rec.messages());
        assert_eq!(rec.phases().last(), Some(&SyncPhase::Idle), "{:?}", rec.messages());
        let msgs = rec.messages();
        assert!(msgs.contains(&"3 categories · 1 copy-protected channels not listed".to_owned()), "{msgs:?}");
        assert!(msgs.contains(&"6 channels (6 new, 0 gone)".to_owned()), "{msgs:?}");
        assert!(msgs.iter().any(|m| m.ends_with("programmes from SiliconDust")), "{msgs:?}");
        // The token is in every guide request and in no message.
        assert!(msgs.iter().all(|m| !m.contains(TOKEN)));

        assert_eq!(
            mirror.rows::<String>("SELECT name FROM categories ORDER BY sort"),
            ["Favourites", "HD", "SD"]
        );
        assert_eq!(
            mirror.rows::<String>(
                "SELECT num_label FROM channels WHERE deleted_at IS NULL ORDER BY num LIMIT 3"
            ),
            ["4.1", "4.2", "4.10"]
        );
        assert!(mirror
            .rows::<i64>("SELECT stream_id FROM channels")
            .iter()
            .all(|id| *id >= crate::m3u::ids::ID_FLOOR));
        // Programmes meet their channels without matching.
        let titles = mirror.rows::<String>(
            "SELECT e.title FROM epg e JOIN channels c ON c.epg_channel_id = e.channel_ref
             WHERE c.name = 'KXYZ-HD' ORDER BY e.start LIMIT 2",
        );
        assert_eq!(titles, ["News", "Movie"]);
        // A day ahead takes more than one twelve-hour window, and each
        // window after the first starts where the last one ended.
        let windows: Vec<_> = server
            .requests()
            .into_iter()
            .filter(|r| r.route() == "/api/guide")
            .collect();
        assert!(windows.len() >= 2, "{} windows", windows.len());
        assert!(windows[0].query("Start").is_none());
        assert!(windows[1].query("Start").is_some());
        assert!(windows.len() <= guide::MAX_WINDOWS);

        // Again: the same ids, nothing new.
        let rec = Recorder::default();
        run_hdhomerun_pass(&rec, &src, mirror.clone()).await;
        assert!(rec.messages().contains(&"6 channels (0 new, 0 gone)".to_owned()), "{:?}", rec.messages());

        assert_eq!(
            src.probe().await.unwrap(),
            "HDHomeRun FLEX 4K (HDFX-4K): 6 channels, 4 tuners · 1 copy-protected, not listed"
        );
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn a_refused_guide_leaves_the_lineup_in_place() {
        let _guard = status::test_lock();
        db::install_test_db();
        let server = hdhomerun(403);
        let mirror = TestMirror::new();
        let rec = Recorder::default();
        let outcome = run_hdhomerun_pass(&rec, &tuner_at(&server, vec![]), mirror.clone()).await;
        assert_eq!(outcome, PassOutcome::Refreshed);
        assert!(
            rec.messages().iter().any(|m| m.contains("SiliconDust's guide service refused")),
            "{:?}",
            rec.messages()
        );
        assert_eq!(mirror.rows::<i64>("SELECT COUNT(*) FROM channels"), [6]);
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn an_xmltv_guide_replaces_silicondusts_and_is_matched_by_name() {
        let _guard = status::test_lock();
        db::install_test_db();
        let server = hdhomerun(200);
        let src = tuner_at(&server, vec![format!("{}/guide.xml", server.base)]);
        let mirror = TestMirror::new();
        let rec = Recorder::default();
        run_hdhomerun_pass(&rec, &src, mirror.clone()).await;
        assert!(
            rec.messages().contains(&"1 programmes, guide for 1 of 6 channels".to_owned()),
            "{:?}",
            rec.messages()
        );
        assert!(!server.requests().iter().any(|r| r.route() == "/api/guide"));
        assert_eq!(
            mirror.rows::<String>("SELECT epg_channel_id FROM channels WHERE name = 'KXYZ-HD'"),
            ["KXYZ.us"]
        );
        // Back to SiliconDust: the channel points at its own guide again.
        run_hdhomerun_pass(&Recorder::default(), &tuner_at(&server, vec![]), mirror.clone()).await;
        assert_eq!(
            mirror.rows::<String>("SELECT epg_channel_id FROM channels WHERE name = 'KXYZ-HD'"),
            ["hdhr:4.1"]
        );
    }

    #[tokio::test]
    async fn a_busy_tuner_is_named_as_one() {
        let server = hdhomerun(200);
        let src = tuner_at(&server, vec![]);
        let msg = src
            .explain_url(&format!("{}/auto/v4.1", server.base))
            .await
            .unwrap();
        assert_eq!(
            msg,
            "All 4 tuners on HDHomeRun FLEX 4K (HDFX-4K) are in use. Stop something that is watching or recording on another device, then try again."
        );
        // A URL that answers with something else is nothing to explain.
        assert_eq!(src.explain_url(&format!("{}/lineup.json", server.base)).await, None);
    }

    #[tokio::test]
    async fn an_unreachable_tuner_says_where_it_looked() {
        // A port nothing listens on: bound, then closed.
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let src = HdhrSource::new(HdhrAccount {
            device: format!("127.0.0.1:{port}"),
            epg_urls: vec![],
        })
        .unwrap();
        let err = format!("{:#}", src.probe().await.unwrap_err());
        assert!(err.contains(&format!("could not reach the HDHomeRun at http://127.0.0.1:{port}")), "{err}");
    }

    // -------------------------------------------------- the fake Tvheadend

    const M3U: &str = include_str!("../../tests/fixtures/tvheadend/channels.m3u");
    const XMLTV: &str = include_str!("../../tests/fixtures/tvheadend/channels.xml");
    const AUTH_CODE: &str = "tvhAUTHcode42";

    #[derive(Clone, Copy)]
    struct Tvh {
        /// Whether anything needs a login.
        login: bool,
        /// Whether the user has persistent authentication enabled.
        persistent: bool,
        /// Whether the first Digest answer is told its nonce went stale.
        stale_once: bool,
    }

    fn challenge(nonce: &str, stale: bool) -> Reply {
        let mut v = format!(r#"Digest realm="tvheadend", qop=auth, nonce="{nonce}", opaque="op""#);
        if stale {
            v.push_str(", stale=true");
        }
        Reply::new(401, "").header("WWW-Authenticate", &v)
    }

    /// A Tvheadend that checks Digest the way the real one does: realm,
    /// nonce, the URI as requested, and the response over all of it.
    fn tvheadend(cfg: Tvh) -> Server {
        let staled = AtomicBool::new(false);
        serve(move |req: &Request, base| {
            // The Digest response is over the method too; only GET is used.
            assert_eq!(req.method, "GET");
            if cfg.login {
                let Some(h) = req.header("authorization").and_then(|h| h.strip_prefix("Digest ")) else {
                    return challenge("nonce-1", false);
                };
                let p = digest::parse_params(h);
                let get = |k: &str| {
                    p.iter()
                        .find(|(n, _)| n == k)
                        .map(|(_, v)| v.clone())
                        .unwrap_or_default()
                };
                if cfg.stale_once && !staled.swap(true, Ordering::SeqCst) {
                    return challenge("nonce-2", true);
                }
                let ch = digest::parse_challenge(&format!(
                    r#"realm="tvheadend", qop=auth, nonce="{}", opaque="op""#,
                    get("nonce")
                ))
                .unwrap();
                let nc = u32::from_str_radix(&get("nc"), 16).unwrap_or(0);
                let want = digest::response(
                    &ch,
                    &get("username"),
                    "tvh-secret",
                    "GET",
                    &get("uri"),
                    nc,
                    &get("cnonce"),
                );
                if get("username") != "viewer" || get("response") != want || get("uri") != req.path {
                    return challenge("nonce-1", false);
                }
            }
            let playlist = |query: &str| M3U.replace("{base}", base).replace("{query}", query);
            match req.route() {
                "/api/serverinfo" => Reply::ok(
                    r#"{"sw_version":"4.3-2183~g1b5b5ca","api_version":19,"name":"Tvheadend"}"#,
                ),
                "/playlist/auth/channels" if cfg.persistent => {
                    Reply::ok(playlist(&format!("?auth={AUTH_CODE}&")))
                }
                "/playlist/auth/channels" => Reply::new(403, ""),
                "/playlist/channels" => Reply::ok(playlist("?")),
                "/api/channeltag/grid" => Reply::ok(
                    serde_json::json!({"entries": [
                        {"uuid": "t-hd", "name": "HDTV", "index": 2, "enabled": true},
                        {"uuid": "t-tv", "name": "TV channels", "index": 1, "enabled": true},
                        {"uuid": "t-radio", "name": "Radio channels", "index": 3, "enabled": true}
                    ]})
                    .to_string(),
                ),
                "/api/channel/grid" => Reply::ok(
                    serde_json::json!({"entries": [
                        {"uuid": "c0ffee0000000000000000000000c001", "tags": ["t-hd", "t-tv"]},
                        {"uuid": "c0ffee0000000000000000000000c002", "tags": ["t-tv"]},
                        {"uuid": "c0ffee0000000000000000000000c003", "tags": []},
                        {"uuid": "c0ffee0000000000000000000000c004", "tags": ["t-radio"]}
                    ]})
                    .to_string(),
                ),
                "/xmltv/channels" => Reply::ok(XMLTV),
                _ => Reply::new(404, ""),
            }
        })
    }

    fn server_at(server: &Server, username: &str, password: &str) -> TvhSource {
        TvhSource::new(TvhAccount {
            url: server.base.clone(),
            username: username.into(),
            password: password.into(),
        })
        .unwrap()
    }

    const ON: Tvh = Tvh {
        login: true,
        persistent: true,
        stale_once: false,
    };

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn a_tvheadend_server_syncs_end_to_end_with_persistent_auth() {
        let _guard = status::test_lock();
        db::install_test_db();
        let server = tvheadend(ON);
        let src = server_at(&server, "viewer", "tvh-secret");
        let mirror = TestMirror::new();
        let rec = Recorder::default();

        let outcome = run_tvheadend_pass(&rec, &src, mirror.clone()).await;
        assert_eq!(outcome, PassOutcome::Refreshed, "{:?}", rec.messages());
        let msgs = rec.messages();
        assert!(msgs.contains(&"3 categories".to_owned()), "{msgs:?}");
        assert!(msgs.contains(&"4 channels (4 new, 0 gone)".to_owned()), "{msgs:?}");
        // Channel 3 is in no guide.
        assert!(msgs.contains(&"4 programmes, guide for 3 of 4 channels".to_owned()), "{msgs:?}");
        assert_eq!(rec.phases().last(), Some(&SyncPhase::Idle));

        // Categories in the server's tag order, the untagged last.
        assert_eq!(
            mirror.rows::<String>("SELECT name FROM categories ORDER BY sort"),
            ["TV channels", "Radio channels", "Other"]
        );
        // Stream URLs carry the persistent code, so mpv needs no login.
        let urls = mirror.rows::<String>("SELECT stream_url FROM channels ORDER BY num");
        assert_eq!(urls.len(), 4);
        assert!(urls.iter().all(|u| u.contains(&format!("?auth={AUTH_CODE}&profile=pass"))), "{urls:?}");
        assert_eq!(
            mirror.rows::<String>("SELECT num_label FROM channels WHERE name = 'KXYZ-HD'"),
            ["5.1"]
        );
        // Guide channels are the UUIDs, matched directly.
        assert_eq!(
            mirror.rows::<String>("SELECT epg_channel_id FROM channels WHERE name = 'Northwind Radio'"),
            ["c0ffee0000000000000000000000c004"]
        );
        // Every request after a challenge answered it.
        let reqs = server.requests();
        assert!(reqs.iter().any(|r| r.header("authorization").is_some_and(|h| h.starts_with("Digest "))));
        assert!(!reqs.iter().any(|r| r.header("authorization").is_some_and(|h| h.starts_with("Basic "))));

        assert_eq!(src.probe().await.unwrap(), "Tvheadend 4.3-2183~g1b5b5ca: 4 channels");
    }

    #[test]
    fn retagging_or_renaming_a_tvheadend_channel_keeps_its_id() {
        let c = crate::catalog::open_in_memory().unwrap();
        let body = |m3u: String| -> Box<dyn Read + Send> { Box::new(std::io::Cursor::new(m3u.into_bytes())) };
        let base = M3U.replace("{base}", "http://tvh").replace("{query}", "?");
        land_tvheadend_playlist(&c, body(base.clone()), &Tags::default(), "tvh:test").unwrap();
        let ids = |c: &rusqlite::Connection| -> Vec<i64> {
            let mut s = c.prepare("SELECT stream_id FROM channels ORDER BY stream_id").unwrap();
            let v = s.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
            v
        };
        let before = ids(&c);
        let mut tags = Tags::default();
        tags.by_channel.insert("c0ffee0000000000000000000000c001".into(), "News".into());
        tags.order.push("News".into());
        let s = land_tvheadend_playlist(&c, body(base.replace("Northwind One HD", "Northwind One")), &tags, "tvh:test").unwrap();
        assert_eq!((s.channels.inserted, s.channels.deleted), (0, 0));
        assert_eq!(ids(&c), before);
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn without_persistent_auth_the_message_says_what_to_enable() {
        let _guard = status::test_lock();
        db::install_test_db();
        let server = tvheadend(Tvh {
            persistent: false,
            ..ON
        });
        let src = server_at(&server, "viewer", "tvh-secret");
        let rec = Recorder::default();
        let outcome = run_tvheadend_pass(&rec, &src, TestMirror::new()).await;
        // A refusal: the pass ends there and backs off.
        assert_eq!(outcome, PassOutcome::Refused);
        assert_eq!(rec.messages().last().map(String::as_str), Some(NEEDS_PERSISTENT_AUTH));
        let err = src.probe().await.unwrap_err().to_string();
        assert!(err.contains("“Persistent authentication” to Enable"), "{err}");
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn a_wrong_password_is_told_apart_from_a_missing_setting() {
        let _guard = status::test_lock();
        db::install_test_db();
        let server = tvheadend(ON);
        let rec = Recorder::default();
        let outcome =
            run_tvheadend_pass(&rec, &server_at(&server, "viewer", "wrong"), TestMirror::new()).await;
        assert_eq!(outcome, PassOutcome::Refused);
        let last = rec.messages().last().cloned().unwrap();
        assert!(last.contains("refused this username and password"), "{last}");
        assert!(!last.contains("wrong") && !last.contains("viewer"), "{last}");
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn an_open_server_needs_no_login_and_a_closed_one_says_so() {
        let _guard = status::test_lock();
        db::install_test_db();
        let open = tvheadend(Tvh {
            login: false,
            persistent: false,
            stale_once: false,
        });
        let src = server_at(&open, "", "");
        let mirror = TestMirror::new();
        assert_eq!(
            run_tvheadend_pass(&Recorder::default(), &src, mirror.clone()).await,
            PassOutcome::Refreshed
        );
        let urls = mirror.rows::<String>("SELECT stream_url FROM channels");
        assert!(urls.iter().all(|u| u.ends_with("?profile=pass")), "{urls:?}");
        assert!(open.requests().iter().all(|r| r.header("authorization").is_none()));
        assert_eq!(src.probe().await.unwrap(), "Tvheadend 4.3-2183~g1b5b5ca: 4 channels · no login");

        let closed = tvheadend(ON);
        let err = server_at(&closed, "", "").probe().await.unwrap_err().to_string();
        assert_eq!(err, NEEDS_LOGIN);
    }

    #[tokio::test]
    async fn a_stale_nonce_is_answered_again_rather_than_refused() {
        let server = tvheadend(Tvh {
            stale_once: true,
            ..ON
        });
        let src = server_at(&server, "viewer", "tvh-secret");
        assert_eq!(src.client().server_info().await.unwrap(), "Tvheadend 4.3-2183~g1b5b5ca");
        // Bare, challenged, stale, answered.
        assert_eq!(server.requests().len(), 3);
    }
}
