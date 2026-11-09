//! Subtitles from OpenSubtitles for a movie or an episode: found, ranked,
//! downloaded with the user's own sign-in, and kept.
//!
//! - **Search** ([`search`]) goes by the title's TMDB id (the show's, with
//!   the season and episode, for an episode), and by its name when there is
//!   no id or the id finds nothing: two searches at most ([`search_plan`]).
//!   What is found is [`rank`]ed: other episodes, other features, other
//!   languages and machine translations dropped, the first language first.
//!   An answer is kept [`SEARCH_KEEP_SECS`] per account, title and
//!   languages, a failure [`FAILED_KEEP_SECS`].
//! - **Download** ([`download`]) needs the user's sign-in, kept in
//!   `online-services.json` with the password so an expired token is renewed
//!   without asking again ([`TOKEN_RENEW_SECS`], at most one sign-in per
//!   [`LOGIN_MIN_GAP`]). A refused sign-in is not tried again by itself.
//!   A file is kept once, as [`file_path`], and never downloaded again,
//!   whichever title asks for it; each title that offers it has a row in
//!   `subtitle_downloads` ([`subtitle_store`]), under the current account.
//!
//! Every public function has a `…_with` twin taking a [`Context`]: the
//! client and its pacer, the search cache, the settings file and the files'
//! directory. Only [`Context::shared`] uses the process's own; each test
//! builds one of its own.
//!
//! Nothing here logs the username, the password, the token or a download
//! link.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::config::{self, OpenSubtitlesConfig};
use super::http::OnlineError;
use super::opensubtitles::{Client, Found, Link, Login, Lookup, SearchParams};
use super::srt;
use crate::catalog::db;
use crate::catalog::subtitle_store::{self, Subject};
use crate::catalog::sync_engine::scrub;
use crate::catalog::vod_store::{self, ItemKind, VodItem};
use crate::api::types::{
    KeptSubtitleDto, MatchedBy, OnlineServicesDto, SubtitleDownloadDto, SubtitleResultDto,
    SubtitleSearchDto,
};

/// How long a search's answer is kept, per account, title and languages.
pub const SEARCH_KEEP_SECS: u64 = 6 * 3600;
/// How long a failed search is answered from memory.
pub const FAILED_KEEP_SECS: u64 = 60;
/// The most results a search shows.
pub const RESULTS_MAX: usize = 50;
/// A token older than this is renewed before it is used.
pub const TOKEN_RENEW_SECS: i64 = 12 * 3600;
// The fixed sentences a failure may be, beside an [`OnlineError`]'s: they
// hold nothing but themselves.
pub const NOT_IN_CATALOGUE: &str = "That title is not in the catalogue any more.";
pub const NOT_A_FILE: &str = "That is not an OpenSubtitles file.";
pub const DOWNLOAD_STOPPED: &str = "The download stopped before it ended.";
pub const NOT_SUBTITLES: &str = "OpenSubtitles sent something that is not a subtitle file.";
pub const ENTER_SIGN_IN: &str = "Enter your OpenSubtitles username and password.";
/// The least time between two sign-ins: OpenSubtitles allows about one a
/// second, and none should be needed more often than this.
pub const LOGIN_MIN_GAP: Duration = Duration::from_secs(10);
/// How long a link paid for and not used up is tried again rather than
/// paying for another; OpenSubtitles keeps its links a few hours.
const LINK_KEEP: Duration = Duration::from_secs(3600);
/// The most languages searched for at once.
const LANGUAGES_MAX: usize = 2;
/// The most searches kept; past it, the stale ones go, then all.
const CACHE_MAX: usize = 500;
/// The most files described from searches.
const DESCRIBED_MAX: usize = 5000;

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// A catalogue or file error, as the interface shows it.
fn failed(e: impl std::fmt::Display) -> String {
    scrub(&format!("{e:#}"))
}

fn file_in(dir: &Path, file_id: i64) -> PathBuf {
    dir.join(format!("{file_id}.srt"))
}

/// Where a downloaded file is kept: `data_dir()/subtitles/<file_id>.srt`.
pub fn file_path(file_id: i64) -> PathBuf {
    file_in(&crate::paths::data_dir().join("subtitles"), file_id)
}

// ------------------------------------------------------------- searching

/// Language codes as OpenSubtitles takes them: lower-case, `xx` or `xx-yy`,
/// each once, the first [`LANGUAGES_MAX`] kept in their order.
pub fn clean_languages(given: &[String]) -> Vec<String> {
    let letters = |s: &[u8]| s.len() == 2 && s.iter().all(u8::is_ascii_lowercase);
    let mut out: Vec<String> = Vec::new();
    for l in given {
        let l = l.trim().to_ascii_lowercase();
        let b = l.as_bytes();
        let code = match b.len() {
            2 => letters(b),
            5 => letters(&b[..2]) && b[2] == b'-' && letters(&b[3..]),
            _ => false,
        };
        if code && !out.contains(&l) {
            out.push(l);
            if out.len() == LANGUAGES_MAX {
                break;
            }
        }
    }
    out
}

/// A name's last word when it is a year (a provider may name a film
/// "Film 0001 2021").
fn year_word(w: &str) -> Option<i32> {
    let y: i32 =
        (w.len() == 4 && w.bytes().all(|b| b.is_ascii_digit())).then(|| w.parse().ok())??;
    (1900..=2100).contains(&y).then_some(y)
}

/// Whether `code` is a provider's language label: upper-case codes
/// (`EN`, `PT-BR`), alone or joined with `/` (`FR/EN`). Three letters
/// (`ENG`) only when `three` is allowed.
fn language_label(code: &str, three: bool) -> bool {
    let upper = |s: &str| s.bytes().all(|b| b.is_ascii_uppercase());
    let longest = if three { 3 } else { 2 };
    !code.is_empty()
        && code.split('/').all(|part| match part.split_once('-') {
            None => (2..=longest).contains(&part.len()) && upper(part),
            Some((lang, region)) => {
                lang.len() == 2 && region.len() == 2 && upper(lang) && upper(region)
            }
        })
}

/// `name` without the language label a provider puts before it
/// (`EN - Film 0001`, `FR/EN: Film 0001`), which [`crate::catalog::clean_name`]
/// keeps; unchanged when there is no label, or nothing after it. Before a
/// colon only two-letter codes count: a title's own short first word is
/// often followed by one (`ABC: …`), and seldom by a spaced dash.
fn without_language_label(name: &str) -> &str {
    let name = name.trim_start();
    let end = name
        .find(|c: char| !(c.is_ascii_alphabetic() || c == '/' || c == '-'))
        .unwrap_or(name.len());
    let (code, rest) = name.split_at(end);
    if !language_label(code, true) {
        return name;
    }
    let after = if let Some(r) = rest.strip_prefix(':') {
        if !language_label(code, false) {
            return name;
        }
        r
    } else {
        let r = rest.trim_start();
        let dashed = r.strip_prefix('-').or_else(|| r.strip_prefix('–'));
        match dashed {
            Some(d)
                if rest.starts_with(char::is_whitespace) && d.starts_with(char::is_whitespace) =>
            {
                d
            }
            _ => return name,
        }
    };
    if after.chars().any(char::is_alphanumeric) {
        after.trim_start()
    } else {
        name
    }
}

/// The name searched for, lower-case, without a provider's language label
/// before it and a trailing year (which becomes the year when none is
/// known); `None` when nothing is left.
fn searchable_name(name: &str, year: Option<i32>) -> Option<(String, Option<i32>)> {
    let mut words: Vec<&str> = without_language_label(name).split_whitespace().collect();
    let mut year = year;
    if words.len() > 1 {
        if let Some(y) = words.last().and_then(|w| year_word(w)) {
            if year.is_none_or(|known| known == y) {
                year = Some(y);
                words.pop();
            }
        }
    }
    while words
        .last()
        .is_some_and(|w| w.chars().all(|c| matches!(c, '-' | '–' | '—' | ':' | '|')))
    {
        words.pop();
    }
    let query = words.join(" ").to_lowercase();
    query
        .chars()
        .any(char::is_alphanumeric)
        .then_some((query, year))
}

/// The searches to try, in turn, until one finds something: the TMDB id
/// when there is one, then the name (with the film's year; an episode's
/// show by its name alone). Empty when there is nothing to search by.
pub fn search_plan(s: &Subject, languages: &[String]) -> Vec<(SearchParams, MatchedBy)> {
    let episodic = s.item.kind == ItemKind::Episode;
    let (season, episode) = if episodic {
        (s.season, s.episode)
    } else {
        (None, None)
    };
    let params = |lookup| SearchParams {
        lookup,
        season,
        episode,
        languages: languages.to_vec(),
    };
    let mut plan = Vec::new();
    if let Some(id) = s.tmdb_id.filter(|id| *id > 0) {
        plan.push((params(Lookup::Tmdb(id)), MatchedBy::Id));
    }
    if let Some((query, year)) = searchable_name(&s.name, s.year) {
        let year = if episodic { None } else { year };
        plan.push((params(Lookup::Name { query, year }), MatchedBy::Name));
    }
    plan
}

/// Whether a row is for this title, as far as OpenSubtitles says: a film's
/// for a film, an episode's for the same season and episode. What it does
/// not say is not held against it.
fn fits(f: &Found, s: &Subject) -> bool {
    let episodic = s.item.kind == ItemKind::Episode;
    let kind_ok = match f.feature.kind.as_deref() {
        None => true,
        Some(k) if episodic => k == "episode",
        Some(k) => k == "movie",
    };
    let same = |theirs: Option<i32>, ours: Option<i32>| {
        theirs.is_none() || ours.is_none() || theirs == ours
    };
    kind_ok
        && (!episodic || (same(f.feature.season, s.season) && same(f.feature.episode, s.episode)))
}

/// What a search found, as the interface lists it: other features and
/// episodes, other languages and machine translations dropped; the first
/// language first, then full subtitles before forced ones, trusted
/// uploaders, and the most downloaded; at most [`RESULTS_MAX`]. `kept` are
/// the files already on this computer.
pub fn rank(
    found: Vec<Found>,
    s: &Subject,
    languages: &[String],
    kept: &HashSet<i64>,
) -> Vec<SubtitleResultDto> {
    let wanted: Vec<String> = languages.iter().map(|l| l.to_ascii_lowercase()).collect();
    let mut seen = HashSet::new();
    let mut rows: Vec<(usize, Found)> = found
        .into_iter()
        .filter(|f| !f.translated && fits(f, s) && seen.insert(f.file_id))
        .filter_map(|f| {
            let language = f.language.trim().to_ascii_lowercase();
            // A language asked for without a region takes every region's.
            let primary = language.split('-').next().unwrap_or_default();
            let place = if wanted.is_empty() {
                Some(0)
            } else {
                wanted
                    .iter()
                    .position(|w| *w == language || (!w.contains('-') && w == primary))
            };
            place.map(|p| (p, Found { language, ..f }))
        })
        .collect();
    rows.sort_by(|(pa, a), (pb, b)| {
        pa.cmp(pb)
            .then(a.forced.cmp(&b.forced))
            .then(b.trusted.cmp(&a.trusted))
            .then(b.downloads.cmp(&a.downloads))
            .then(a.file_id.cmp(&b.file_id))
    });
    rows.truncate(RESULTS_MAX);
    rows.into_iter()
        .map(|(_, f)| SubtitleResultDto {
            kept: kept.contains(&f.file_id),
            file_id: f.file_id,
            language: f.language,
            release: f.release,
            hearing_impaired: f.hearing_impaired,
            forced: f.forced,
            trusted: f.trusted,
            downloads: f.downloads,
        })
        .collect()
}

/// A search, as it is kept: the account's origin (another account's movie 7
/// is another film), the title and the languages.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct SearchKey {
    origin: String,
    item: VodItem,
    languages: Vec<String>,
}

/// What a search found (before ranking) and how, or why it failed.
type Searched = Result<(Vec<Found>, Option<MatchedBy>), String>;

/// A file as a search described it, for the row kept when it is downloaded.
#[derive(Clone)]
struct Described {
    language: String,
    release: String,
    hearing_impaired: bool,
}

/// Searches answered lately, and the files they described.
#[derive(Default)]
struct SearchCache {
    entries: Mutex<HashMap<SearchKey, (Instant, Searched)>>,
    files: Mutex<HashMap<i64, Described>>,
}

fn keeps_for(r: &Searched) -> Duration {
    Duration::from_secs(if r.is_ok() {
        SEARCH_KEEP_SECS
    } else {
        FAILED_KEEP_SECS
    })
}

impl SearchCache {
    fn get(&self, key: &SearchKey, now: Instant) -> Option<Searched> {
        lock(&self.entries)
            .get(key)
            .filter(|(at, r)| now.saturating_duration_since(*at) < keeps_for(r))
            .map(|(_, r)| r.clone())
    }

    fn put(&self, key: SearchKey, r: Searched, now: Instant) {
        if let Ok((found, _)) = &r {
            let mut files = lock(&self.files);
            if files.len() + found.len() > DESCRIBED_MAX {
                files.clear();
            }
            for f in found {
                files.insert(
                    f.file_id,
                    Described {
                        language: f.language.trim().to_ascii_lowercase(),
                        release: f.release.clone(),
                        hearing_impaired: f.hearing_impaired,
                    },
                );
            }
        }
        let mut entries = lock(&self.entries);
        if entries.len() >= CACHE_MAX {
            entries.retain(|_, (at, r)| now.saturating_duration_since(*at) < keeps_for(r));
            if entries.len() >= CACHE_MAX {
                entries.clear();
            }
        }
        entries.insert(key, (now, r));
    }

    fn described(&self, file_id: i64) -> Option<Described> {
        lock(&self.files).get(&file_id).cloned()
    }
}

// ------------------------------------------------------------ the context

/// What the account's downloads have shown, for as long as the process runs.
#[derive(Default)]
struct Account {
    /// Downloads left and when they come back (Unix seconds), when known.
    quota: Mutex<Option<(u32, Option<i64>)>>,
    /// When a sign-in was last tried.
    last_login: Mutex<Option<Instant>>,
    /// The files being downloaded, each with the lock its download holds.
    in_flight: Mutex<HashMap<i64, Arc<tokio::sync::Mutex<()>>>>,
    /// Links had from `/download` whose file is not kept yet: when, and for
    /// which OpenSubtitles user. Each cost one of that user's downloads, so
    /// a failed fetch tries the link again rather than paying for another
    /// (for [`LINK_KEEP`]); a sign-in, a sign-out or turning the service off
    /// forgets them. A download counted whose answer was lost is kept
    /// without a link, so that it is not paid for again meanwhile either.
    links: Mutex<HashMap<i64, PaidLink>>,
    /// Held while the token is renewed, so that downloads waiting on a
    /// renewal share it rather than each signing in.
    renewing: tokio::sync::Mutex<()>,
}

impl Account {
    fn quota(&self) -> Option<(u32, Option<i64>)> {
        let mut q = lock(&self.quota);
        // Past its reset, a spent allowance is not known any more.
        if q.is_some_and(|(_, at)| at.is_some_and(|at| at <= unix_now())) {
            *q = None;
        }
        *q
    }

    fn set_quota(&self, q: Option<(u32, Option<i64>)>) {
        *lock(&self.quota) = q;
    }

    fn file_lock(&self, file_id: i64) -> Arc<tokio::sync::Mutex<()>> {
        lock(&self.in_flight).entry(file_id).or_default().clone()
    }

    /// Forgets `file_id`'s lock once nobody else waits on it.
    fn release(&self, file_id: i64, held: Arc<tokio::sync::Mutex<()>>) {
        let mut m = lock(&self.in_flight);
        // The map's and this one.
        if Arc::strong_count(&held) <= 2 {
            m.remove(&file_id);
        }
    }

    /// The link `username` paid for `file_id` and did not use up, while it
    /// is fresh at `now`. Another user's is forgotten, never handed out.
    fn take_link(&self, file_id: i64, username: &str, now: Instant) -> Option<PaidLink> {
        let mut links = lock(&self.links);
        links.retain(|_, p| now.saturating_duration_since(p.at) < LINK_KEEP);
        links.remove(&file_id).filter(|p| p.username == username)
    }

    fn put_link(&self, file_id: i64, paid: PaidLink) {
        lock(&self.links).insert(file_id, paid);
    }

    fn forget_links(&self) {
        lock(&self.links).clear();
    }
}

/// A download paid for, when and by whom, and its link unless the answer
/// that held it was lost.
struct PaidLink {
    at: Instant,
    username: String,
    link: Option<Link>,
}

/// Everything a search or a download uses: OpenSubtitles' client (with its
/// pacer), the search cache, the settings file, the files' directory and
/// the account's state.
pub struct Context {
    client: Client,
    cache: SearchCache,
    config_path: PathBuf,
    files_dir: PathBuf,
    account: Account,
}

impl Context {
    pub fn new(client: Client, config_path: PathBuf, files_dir: PathBuf) -> Self {
        Self {
            client,
            cache: SearchCache::default(),
            config_path,
            files_dir,
            account: Account::default(),
        }
    }

    /// The process's own: the shared pacer, the process-wide search cache,
    /// `config_dir()` and `data_dir()/subtitles`.
    pub fn shared() -> &'static Arc<Context> {
        static SHARED: OnceLock<Arc<Context>> = OnceLock::new();
        SHARED.get_or_init(|| {
            Arc::new(Context::new(
                Client::new(),
                crate::paths::config_dir().join(config::FILE),
                crate::paths::data_dir().join("subtitles"),
            ))
        })
    }

    fn settings(&self) -> OpenSubtitlesConfig {
        config::load_from(&self.config_path).opensubtitles
    }

    fn update<R>(&self, f: impl FnOnce(&mut OpenSubtitlesConfig) -> R) -> Result<R, String> {
        config::update_in(&self.config_path, |c| f(&mut c.opensubtitles))
            .map(|(_, r)| r)
            .map_err(failed)
    }
}

/// The files of `found` already on this computer.
fn on_disk(dir: &Path, found: &[Found]) -> HashSet<i64> {
    found
        .iter()
        .map(|f| f.file_id)
        .filter(|id| file_in(dir, *id).is_file())
        .collect()
}

/// Tries `plan` in turn: the first search whose rows rank to something is
/// the answer; when none does, the last one's rows.
async fn run_plan(
    ctx: &Context,
    s: &Subject,
    languages: &[String],
    plan: Vec<(SearchParams, MatchedBy)>,
) -> Result<(Vec<Found>, Option<MatchedBy>), OnlineError> {
    let mut last = Vec::new();
    for (params, by) in plan {
        let found = ctx.client.search(&params).await?;
        if !rank(found.clone(), s, languages, &HashSet::new()).is_empty() {
            return Ok((found, Some(by)));
        }
        last = found;
    }
    Ok((last, None))
}

/// Finds subtitles for `item` in `languages` (cleaned; none means the
/// settings' languages, else English).
pub async fn search(item: VodItem, languages: Vec<String>) -> Result<SubtitleSearchDto, String> {
    search_with(Context::shared(), item, languages).await
}

pub async fn search_with(
    ctx: &Context,
    item: VodItem,
    languages: Vec<String>,
) -> Result<SubtitleSearchDto, String> {
    let settings = ctx.settings();
    if !settings.enabled {
        return Ok(SubtitleSearchDto::Off);
    }
    let mut langs = clean_languages(&languages);
    if langs.is_empty() {
        langs = clean_languages(&settings.languages);
    }
    if langs.is_empty() {
        langs = vec!["en".to_owned()];
    }
    let (subject, origin) =
        db::with_db(|c| Ok((subtitle_store::subject(c, item)?, vod_store::origin(c)?)))
            .map_err(failed)?;
    let Some(subject) = subject else {
        return Err(NOT_IN_CATALOGUE.to_owned());
    };
    let key = SearchKey {
        origin,
        item,
        languages: langs.clone(),
    };
    let searched = match ctx.cache.get(&key, Instant::now()) {
        Some(r) => r,
        None => {
            let plan = search_plan(&subject, &langs);
            if plan.is_empty() {
                return Ok(SubtitleSearchDto::UnknownTitle);
            }
            let r = match run_plan(ctx, &subject, &langs, plan).await {
                Ok(found) => Ok(found),
                Err(OnlineError::Busy { retry_at }) => {
                    return Ok(SubtitleSearchDto::Busy { retry_at })
                }
                Err(OnlineError::Off) => return Ok(SubtitleSearchDto::Off),
                Err(e) => Err(e.to_string()),
            };
            ctx.cache.put(key, r.clone(), Instant::now());
            r
        }
    };
    let (found, matched_by) = searched?;
    let here = on_disk(&ctx.files_dir, &found);
    let results = rank(found, &subject, &langs, &here);
    tracing::info!(
        "OpenSubtitles: {} results for {} {}",
        results.len(),
        item.kind.as_str(),
        item.id
    );
    Ok(SubtitleSearchDto::Results {
        matched_by: matched_by.filter(|_| !results.is_empty()),
        results,
        languages: langs,
    })
}

// ----------------------------------------------------------- downloading

/// How a download that did not happen is answered.
fn answer(ctx: &Context, e: OnlineError) -> Result<SubtitleDownloadDto, String> {
    match e {
        OnlineError::Off => Ok(SubtitleDownloadDto::Off),
        OnlineError::SignInNeeded => Ok(SubtitleDownloadDto::SignIn {
            refused: false,
            email: false,
        }),
        OnlineError::SignInRefused { email } => Ok(SubtitleDownloadDto::SignIn {
            refused: true,
            email,
        }),
        OnlineError::Quota { reset_at } => {
            ctx.account.set_quota(Some((0, reset_at)));
            Ok(SubtitleDownloadDto::Quota { reset_at })
        }
        OnlineError::Busy { retry_at } => Ok(SubtitleDownloadDto::Busy { retry_at }),
        e => Err(e.to_string()),
    }
}

/// Signs in. A renewal (`paced`) is refused when the last sign-in was tried
/// under [`LOGIN_MIN_GAP`] ago; the user's own sign-in is not, so a
/// password typed again is never "busy".
async fn login_with(
    ctx: &Context,
    username: &str,
    password: &str,
    paced: bool,
) -> Result<Login, OnlineError> {
    {
        let now = Instant::now();
        let mut last = lock(&ctx.account.last_login);
        let next = last.map(|at| at + LOGIN_MIN_GAP).filter(|next| *next > now);
        if let Some(next) = next.filter(|_| paced) {
            let wait = next.saturating_duration_since(now).as_secs_f64().ceil() as i64;
            return Err(OnlineError::Busy {
                retry_at: unix_now() + wait,
            });
        }
        *last = Some(now);
    }
    ctx.client.login(username, password).await
}

/// A new token for the sign-in `seen`, saved with it. One renewal at a
/// time: one that waited on another uses the token it had. A refusal is
/// saved too, so that it is not tried again until the user signs in. What
/// a sign-out or another account's sign-in wrote meanwhile is left alone.
async fn renew(
    ctx: &Context,
    seen: &OpenSubtitlesConfig,
) -> Result<OpenSubtitlesConfig, OnlineError> {
    let _one_at_a_time = ctx.account.renewing.lock().await;
    let os = ctx.settings();
    // Turned off while this waited its turn.
    if !os.enabled {
        return Err(OnlineError::Off);
    }
    if os.username.trim().is_empty() || os.username != seen.username {
        return Err(OnlineError::SignInNeeded);
    }
    let renewed_meanwhile = os.token.as_deref().is_some_and(|t| !t.trim().is_empty())
        && os.token != seen.token
        && os.token_at >= seen.token_at;
    if renewed_meanwhile {
        return Ok(os);
    }
    if os.refused {
        return Err(OnlineError::SignInRefused {
            email: os.username.contains('@'),
        });
    }
    if os.password.is_empty() {
        return Err(OnlineError::SignInNeeded);
    }
    // The sign-in this renewal started from, unchanged: a sign-in made
    // meanwhile (even the same user's, with a new password) is not marked
    // refused or given a stale token.
    let same_sign_in = |c: &OpenSubtitlesConfig| {
        c.username == os.username
            && c.signed_in()
            && c.token == os.token
            && c.password == os.password
    };
    match login_with(ctx, &os.username, &os.password, true).await {
        Ok(login) => {
            let saved = ctx
                .update(|c| {
                    if !same_sign_in(c) {
                        return None;
                    }
                    c.token = Some(login.token);
                    c.token_at = Some(unix_now());
                    c.api_base = Some(login.api_base);
                    c.refused = false;
                    Some(c.clone())
                })
                .map_err(OnlineError::invalid)?;
            let Some(saved) = saved else {
                tracing::info!("OpenSubtitles: the sign-in changed while it was renewed");
                return Err(OnlineError::SignInNeeded);
            };
            tracing::info!("OpenSubtitles: signed in");
            Ok(saved)
        }
        Err(e @ OnlineError::SignInRefused { .. }) => {
            tracing::info!("OpenSubtitles refused the sign-in kept");
            ctx.update(|c| {
                if same_sign_in(c) {
                    c.refused = true;
                }
            })
            .map_err(OnlineError::invalid)?;
            Err(e)
        }
        Err(e) => Err(e),
    }
}

/// The temporary link to `file_id`, on the token kept (renewed first when
/// old, and once more on a 401).
async fn link_for(
    ctx: &Context,
    mut os: OpenSubtitlesConfig,
    file_id: i64,
) -> Result<Link, OnlineError> {
    let now = unix_now();
    let fresh = os.token.as_deref().is_some_and(|t| !t.trim().is_empty())
        && os.token_at.is_some_and(|at| now - at < TOKEN_RENEW_SECS);
    let mut renewed = false;
    if !fresh {
        os = renew(ctx, &os).await?;
        renewed = true;
    }
    loop {
        let base = os
            .api_base
            .clone()
            .unwrap_or_else(|| ctx.client.base().to_owned());
        let token = os.token.clone().unwrap_or_default();
        match ctx.client.download_link(&base, &token, file_id).await {
            Err(OnlineError::SignInNeeded) if !renewed => {
                os = renew(ctx, &os).await?;
                renewed = true;
            }
            Err(OnlineError::SignInNeeded) => {
                // A token just had and refused: the sign-in is asked for.
                // A token written meanwhile (a new sign-in) stays.
                ctx.update(|c| {
                    if c.token == os.token {
                        c.token = None;
                        c.token_at = None;
                    }
                })
                .map_err(OnlineError::invalid)?;
                return Err(OnlineError::SignInNeeded);
            }
            other => return other,
        }
    }
}

/// How `file_id` is described in the row kept for it: as a search found
/// it, else as another title kept it.
fn describe(ctx: &Context, file_id: i64) -> Result<KeptSubtitleDto, String> {
    let at = unix_now();
    if let Some(d) = ctx.cache.described(file_id) {
        return Ok(KeptSubtitleDto {
            file_id,
            language: d.language,
            release: d.release,
            hearing_impaired: d.hearing_impaired,
            downloaded_at: at,
        });
    }
    let known = db::with_db(|c| subtitle_store::described(c, file_id)).map_err(failed)?;
    Ok(match known {
        Some(k) => KeptSubtitleDto {
            downloaded_at: at,
            ..k
        },
        None => KeptSubtitleDto {
            file_id,
            language: String::new(),
            release: String::new(),
            hearing_impaired: false,
            downloaded_at: at,
        },
    })
}

/// Keeps `file_id` for `item` under the catalogue `origin`.
fn keep(
    ctx: &Context,
    origin: &str,
    item: VodItem,
    file_id: i64,
) -> Result<KeptSubtitleDto, String> {
    let s = describe(ctx, file_id)?;
    db::with_db(|c| subtitle_store::keep_under(c, origin, item, &s)).map_err(failed)?;
    Ok(s)
}

fn tmp_file(dir: &Path, file_id: i64) -> PathBuf {
    dir.join(format!("{file_id}.srt.{}.tmp", std::process::id()))
}

/// Writes the file through a `.tmp` beside it, so that a file that is there
/// is whole.
///
/// The app and the recording service on one computer share the directory,
/// and each locks a file only within itself: the `.tmp` is named for the
/// process, so two writing the same file at once each write their own and
/// the last rename wins, whole. Both may then have paid for that file; that
/// takes the same file chosen in both at the same moment, and is accepted.
fn write_file(dir: &Path, file_id: i64, text: &str) -> Result<(), String> {
    let saving =
        |e: std::io::Error| format!("The subtitle file could not be saved ({:?}).", e.kind());
    std::fs::create_dir_all(dir).map_err(saving)?;
    let tmp = tmp_file(dir, file_id);
    std::fs::write(&tmp, text).map_err(saving)?;
    std::fs::rename(&tmp, file_in(dir, file_id)).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        saving(e)
    })
}

/// Downloads `file_id` for `item` and keeps it; a file already on this
/// computer is kept for `item` with no request.
pub async fn download(item: VodItem, file_id: i64) -> Result<SubtitleDownloadDto, String> {
    download_with(Context::shared(), item, file_id).await
}

pub async fn download_with(
    ctx: &Arc<Context>,
    item: VodItem,
    file_id: i64,
) -> Result<SubtitleDownloadDto, String> {
    if !ctx.settings().enabled {
        // Turned off, perhaps by the other process sharing the settings.
        ctx.account.forget_links();
        return Ok(SubtitleDownloadDto::Off);
    }
    if file_id <= 0 {
        return Err(NOT_A_FILE.to_owned());
    }
    // The catalogue it is asked in: the row is kept under it, even if
    // another account's catalogue replaces it before the file arrives.
    let origin = db::with_db(vod_store::origin).map_err(failed)?;
    let ctx = Arc::clone(ctx);
    // A task of its own, so that a caller that goes away (a page closed, a
    // command cancelled) does not abandon a download already paid for.
    let task = tokio::spawn(async move {
        // One download per file: a second waits for the first, then finds
        // the file on disk (or the link it paid for).
        let held = ctx.account.file_lock(file_id);
        let r = {
            let _turn = held.lock().await;
            download_locked(&ctx, &origin, item, file_id).await
        };
        ctx.account.release(file_id, held);
        r
    });
    task.await.unwrap_or_else(|e| {
        tracing::warn!("a subtitle download stopped: {e}");
        Err(DOWNLOAD_STOPPED.to_owned())
    })
}

/// Why a link gave no kept file: what to answer, and whether the link
/// itself is at fault (refused, or a page or a bad file in place of the
/// subtitles) rather than the way to it or the disk.
struct NotKept {
    answer: Result<SubtitleDownloadDto, String>,
    link_failed: bool,
}

/// The file behind `url`, asked for again once when it could not be
/// reached or the host failed (a 502 or 503 is already asked again by
/// [`super::http::send`]).
async fn fetch(ctx: &Context, url: &str) -> Result<Vec<u8>, OnlineError> {
    match ctx.client.fetch_file(url).await {
        Err(OnlineError::Unreachable(_) | OnlineError::Refused(500 | 504)) => {
            ctx.client.fetch_file(url).await
        }
        r => r,
    }
}

/// Fetches `url`, checks it, writes it and keeps it for `item` under the
/// catalogue `origin`.
async fn fetch_and_keep(
    ctx: &Context,
    origin: &str,
    item: VodItem,
    file_id: i64,
    url: &str,
) -> Result<KeptSubtitleDto, NotKept> {
    let bytes = fetch(ctx, url).await.map_err(|e| NotKept {
        link_failed: matches!(e, OnlineError::Refused(400..=499) | OnlineError::Invalid(_)),
        answer: answer(ctx, e),
    })?;
    let text = srt::decode(&bytes);
    if !srt::looks_like_srt(&text) {
        tracing::info!("OpenSubtitles: file {file_id} is not a subtitle file");
        return Err(NotKept {
            answer: Err(NOT_SUBTITLES.to_owned()),
            link_failed: true,
        });
    }
    let on_disk = |e: String| NotKept {
        answer: Err(e),
        link_failed: false,
    };
    write_file(&ctx.files_dir, file_id, &text).map_err(on_disk)?;
    keep(ctx, origin, item, file_id).map_err(on_disk)
}

async fn download_locked(
    ctx: &Context,
    origin: &str,
    item: VodItem,
    file_id: i64,
) -> Result<SubtitleDownloadDto, String> {
    if file_in(&ctx.files_dir, file_id).is_file() {
        let kept = db::with_db(|c| subtitle_store::kept_file_under(c, origin, item, file_id))
            .map_err(failed)?;
        let subtitle = match kept {
            Some(k) => k,
            None => keep(ctx, origin, item, file_id)?,
        };
        return Ok(SubtitleDownloadDto::Downloaded {
            subtitle,
            downloads_left: ctx.account.quota().map(|(left, _)| left),
        });
    }
    let os = ctx.settings();
    // Turned off while this waited for another download of the file.
    if !os.enabled {
        ctx.account.forget_links();
        return Ok(SubtitleDownloadDto::Off);
    }
    if os.username.trim().is_empty() {
        return Ok(SubtitleDownloadDto::SignIn {
            refused: false,
            email: false,
        });
    }
    if os.refused {
        return Ok(SubtitleDownloadDto::SignIn {
            refused: true,
            email: os.username.contains('@'),
        });
    }
    if !os.signed_in() {
        return Ok(SubtitleDownloadDto::SignIn {
            refused: false,
            email: false,
        });
    }
    // A link this user already paid for is used before another is asked for.
    let username = os.username.clone();
    let (paid, reused) = match ctx.account.take_link(file_id, &username, Instant::now()) {
        Some(paid) => (paid, true),
        None => {
            let link = match link_for(ctx, os, file_id).await {
                Ok(link) => Some(link),
                Err(OnlineError::AnswerLost) => None,
                Err(e) => return answer(ctx, e),
            };
            if let Some(link) = &link {
                if let Some(left) = link.remaining {
                    ctx.account.set_quota(Some((left, link.reset_at)));
                }
            }
            let paid = PaidLink {
                at: Instant::now(),
                username,
                link,
            };
            (paid, false)
        }
    };
    let Some(link) = &paid.link else {
        // Counted, with no link to show for it: kept as it is, so that the
        // file is not paid for again until it would be anyway.
        ctx.account.put_link(file_id, paid);
        return Err(OnlineError::AnswerLost.to_string());
    };
    match fetch_and_keep(ctx, origin, item, file_id, &link.url).await {
        Ok(subtitle) => {
            tracing::info!("OpenSubtitles: downloaded file {file_id}");
            Ok(SubtitleDownloadDto::Downloaded {
                subtitle,
                downloads_left: link.remaining,
            })
        }
        Err(not) => {
            // Tried again next time, unless it has failed twice itself.
            if !(reused && not.link_failed) {
                ctx.account.put_link(file_id, paid);
            }
            not.answer
        }
    }
}

// ------------------------------------------------------------ kept files

/// The files `item` offers under the current account, newest first.
pub fn kept(item: VodItem) -> Result<Vec<KeptSubtitleDto>, String> {
    db::with_db(|c| subtitle_store::kept(c, item)).map_err(failed)
}

fn kept_path_in(dir: &Path, item: VodItem, file_id: i64) -> Result<Option<PathBuf>, String> {
    let row = db::with_db(|c| subtitle_store::kept_file(c, item, file_id)).map_err(failed)?;
    let path = file_in(dir, file_id);
    Ok(row.filter(|_| path.is_file()).map(|_| path))
}

/// The file, when `item` offers it under the current account and it is on
/// this computer.
pub fn kept_path(item: VodItem, file_id: i64) -> Result<Option<PathBuf>, String> {
    kept_path_in(&Context::shared().files_dir, item, file_id)
}

fn vtt_in(dir: &Path, item: VodItem, file_id: i64) -> Result<Option<String>, String> {
    let Some(path) = kept_path_in(dir, item, file_id)? else {
        return Ok(None);
    };
    match std::fs::read(&path) {
        Ok(bytes) => Ok(Some(srt::to_vtt(&srt::decode(&bytes)))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!(
            "The subtitle file could not be read ({:?}).",
            e.kind()
        )),
    }
}

/// A kept file as WebVTT, when `item` offers it under the current account.
pub fn vtt(item: VodItem, file_id: i64) -> Result<Option<String>, String> {
    vtt_with(Context::shared(), item, file_id)
}

pub fn vtt_with(
    ctx: &Context,
    item: VodItem,
    file_id: i64,
) -> Result<Option<String>, String> {
    vtt_in(&ctx.files_dir, item, file_id)
}

// --------------------------------------------------- the account, settings

pub fn status_with(ctx: &Context) -> OnlineServicesDto {
    config::status(&config::load_from(&ctx.config_path), ctx.account.quota())
}

/// What Settings › Online services shows.
pub fn status() -> OnlineServicesDto {
    status_with(Context::shared())
}

/// Signs in to OpenSubtitles, keeping the username, password and token.
/// A refused sign-in leaves the one kept as it was.
pub async fn sign_in(username: String, password: String) -> Result<OnlineServicesDto, String> {
    sign_in_with(Context::shared(), username, password).await
}

pub async fn sign_in_with(
    ctx: &Context,
    username: String,
    password: String,
) -> Result<OnlineServicesDto, String> {
    let username = username.trim().to_owned();
    if username.is_empty() || password.is_empty() {
        return Err(ENTER_SIGN_IN.to_owned());
    }
    if !ctx.settings().enabled {
        return Err(OnlineError::Off.to_string());
    }
    let login = login_with(ctx, &username, &password, false)
        .await
        .map_err(|e| e.to_string())?;
    ctx.update(|c| {
        c.username = username;
        c.password = password;
        c.token = Some(login.token);
        c.token_at = Some(unix_now());
        c.api_base = Some(login.api_base);
        c.refused = false;
    })?;
    ctx.account.set_quota(None);
    // Links paid for by the sign-in before are its own.
    ctx.account.forget_links();
    tracing::info!("OpenSubtitles: signed in");
    Ok(status_with(ctx))
}

/// Ends the sign-in (OpenSubtitles is told, when it is on) and forgets it.
pub async fn sign_out() -> Result<OnlineServicesDto, String> {
    sign_out_with(Context::shared()).await
}

pub async fn sign_out_with(ctx: &Context) -> Result<OnlineServicesDto, String> {
    let os = ctx.settings();
    if let (true, Some(token)) = (os.enabled, os.token.as_deref()) {
        let base = os.api_base.as_deref().unwrap_or(ctx.client.base());
        ctx.client.logout(base, token).await;
    }
    ctx.update(|c| {
        c.username.clear();
        c.password.clear();
        c.token = None;
        c.token_at = None;
        c.api_base = None;
        c.refused = false;
    })?;
    ctx.account.set_quota(None);
    ctx.account.forget_links();
    tracing::info!("OpenSubtitles: signed out");
    Ok(status_with(ctx))
}

/// Turns OpenSubtitles on or off, and sets the languages searched for
/// (cleaned as [`clean_languages`]; empty is the interface's own).
pub fn set_opensubtitles(
    enabled: Option<bool>,
    languages: Option<Vec<String>>,
) -> Result<OnlineServicesDto, String> {
    set_opensubtitles_with(Context::shared(), enabled, languages)
}

pub fn set_opensubtitles_with(
    ctx: &Context,
    enabled: Option<bool>,
    languages: Option<Vec<String>>,
) -> Result<OnlineServicesDto, String> {
    ctx.update(|c| {
        if let Some(on) = enabled {
            c.enabled = on;
        }
        if let Some(l) = languages {
            c.languages = clean_languages(&l);
        }
    })?;
    if enabled == Some(false) {
        ctx.account.forget_links();
    }
    Ok(status_with(ctx))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{db, origin};
    use crate::online::config;
    use crate::online::http::Pacer;
    use crate::online::opensubtitles::{Feature, Found, Lookup};
    use crate::test_http::{serve, Reply, Request, Server};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    const ORIGIN_A: &str = "xtream:subs-a";
    const ORIGIN_B: &str = "xtream:subs-b";
    const FILM_1: i64 = 940_001;
    const FILM_2: i64 = 940_002;
    /// "Film 0002" again, under a TMDB id OpenSubtitles knows nothing of.
    const FILM_WRONG_ID: i64 = 940_003;
    const NOWHERE: i64 = 940_004;
    const SHOW_1: i64 = 94_001;
    const S1E2: i64 = 9_400_101;
    const S1E3: i64 = 9_400_102;
    const SPECIAL: i64 = 9_400_103;
    const S2E1_E2: i64 = 9_400_104;
    /// 2036-11-08T12:00:00Z: when the stand-in's downloads come back.
    const RESET_AT: i64 = 2_109_758_400;

    /// The test catalogue's titles, under origin A.
    fn seed() {
        db::install_test_db();
        db::with_db(|c| {
            c.execute_batch(&format!(
                "INSERT OR REPLACE INTO movies (movie_id, name, clean_name, year, added, tmdb_id)
                   VALUES ({FILM_1}, 'Film 0001', 'Film 0001', 2021, 0, 900001),
                          ({FILM_2}, 'Film 0002', 'Film 0002', 2019, 0, NULL),
                          ({FILM_WRONG_ID}, 'Film 0002', 'Film 0002', 2019, 0, 900999),
                          ({NOWHERE}, 'Film 0009', 'Film 0009', 2018, 0, 900009);
                 INSERT OR REPLACE INTO series (series_id, name, clean_name, year, added, tmdb_id)
                   VALUES ({SHOW_1}, 'Show 0001', 'Show 0001', 2020, 0, 800001);
                 INSERT OR REPLACE INTO episodes (episode_id, series_id, season, episode, episode_end)
                   VALUES ({S1E2}, {SHOW_1}, 1, 2, NULL),
                          ({S1E3}, {SHOW_1}, 1, 3, NULL),
                          ({SPECIAL}, {SHOW_1}, 0, 1, NULL),
                          ({S2E1_E2}, {SHOW_1}, 2, 1, 2);
                 DELETE FROM subtitle_downloads
                   WHERE item_id IN ({FILM_1}, {FILM_2}, {FILM_WRONG_ID}, {NOWHERE}, {S1E2});"
            ))?;
            origin::record(c, ORIGIN_A)
        })
        .unwrap();
    }

    fn subject_of(item: VodItem) -> Subject {
        db::with_db(|c| subtitle_store::subject(c, item))
            .unwrap()
            .unwrap()
    }

    fn row(file_id: i64, lang: &str, release: &str, feature: &str, extra: &str) -> String {
        format!(
            r#"{{"id":"{file_id}","type":"subtitle","attributes":{{"language":"{lang}","download_count":10,"hearing_impaired":false,"foreign_parts_only":false,"from_trusted":false,"ai_translated":false,"machine_translated":false,"release":"{release}","feature_details":{feature},"files":[{{"file_id":{file_id},"cd_number":1}}]{extra}}}}}"#
        )
    }

    const FILM_1_FEATURE: &str =
        r#"{"feature_type":"Movie","tmdb_id":900001,"season_number":null,"episode_number":null}"#;

    /// The stand-in's rows for Film 0001 (`tools/fake_opensubtitles.py`).
    fn film_1_rows() -> Vec<String> {
        let f = FILM_1_FEATURE;
        vec![
            row(7000001, "en", "Film.0001.2021.1080p.TEST", f, "")
                .replace(r#""download_count":10"#, r#""download_count":1200"#)
                .replace(r#""from_trusted":false"#, r#""from_trusted":true"#),
            row(7000002, "en", "Film.0001.2021.1080p.TEST.SDH", f, "")
                .replace(r#""hearing_impaired":false"#, r#""hearing_impaired":true"#),
            row(7000003, "en", "Film.0001.2021.AI", f, "")
                .replace(r#""ai_translated":false"#, r#""ai_translated":true"#),
            row(7000004, "fr", "Film.0001.2021.1080p.TEST", f, ""),
        ]
    }

    fn page(rows: &[String]) -> String {
        format!(
            r#"{{"total_pages":1,"page":1,"data":[{}]}}"#,
            rows.join(",")
        )
    }

    /// The stand-in's search: Film 0001 by its id, Show 0001's S1E2 by the
    /// show's, Film 0002 by its name; nothing else.
    fn search_answer(req: &Request) -> Reply {
        let q = |k| req.query(k);
        if q("tmdb_id") == Some("900001") {
            return Reply::ok(page(&film_1_rows()));
        }
        if q("parent_tmdb_id") == Some("800001")
            && q("season_number") == Some("1")
            && q("episode_number") == Some("2")
        {
            let ep = |e| {
                format!(
                    r#"{{"feature_type":"Episode","parent_tmdb_id":800001,"season_number":1,"episode_number":{e}}}"#
                )
            };
            return Reply::ok(page(&[
                row(7000011, "en", "Show.0001.S01E02.TEST", &ep(2), ""),
                row(7000012, "en", "Show.0001.S01E03.TEST", &ep(3), ""),
            ]));
        }
        if q("query") == Some("film+0002") {
            return Reply::ok(page(&[row(
                7000021,
                "en",
                "Film.0002.2019.720p.TEST",
                r#"{"feature_type":"Movie","tmdb_id":900002}"#,
                "",
            )]));
        }
        Reply::ok(page(&[]))
    }

    fn srt(file_id: i64, cues: u32) -> String {
        (1..=cues)
            .map(|k| {
                let start = k * 2 - 1;
                format!(
                    "{k}\n00:00:{start:02},000 --> 00:00:{:02},000\nTest line {k} of file {file_id}\n",
                    start + 2
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn login_ok(base: &str) -> Reply {
        Reply::ok(format!(
            r#"{{"token":"fake-os-token-2","base_url":"{}","user":{{"allowed_downloads":20}},"status":200}}"#,
            base.trim_start_matches("http://")
        ))
    }

    fn link_to(base: &str, file: &str) -> Reply {
        Reply::ok(format!(
            r#"{{"link":"{base}/files/{file}","remaining":4,"reset_time_utc":"2036-11-08T12:00:00.000Z"}}"#
        ))
    }

    /// A stand-in that searches, signs in `tester`, and hands out file
    /// 7000001 for every download.
    fn stand_in() -> Server {
        serve(|req, base| match req.route() {
            "/api/v1/subtitles" => search_answer(req),
            "/api/v1/login" => login_ok(base),
            "/api/v1/download" => link_to(base, "7000001.srt"),
            "/files/7000001.srt" => Reply::ok(srt(7000001, 2)),
            _ => Reply::new(404, ""),
        })
    }

    /// A test's scratch directory, removed when the test ends.
    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A context of the test's own: its own pacer, cache, settings file and
    /// files, at `server`.
    fn context(server: &Server, name: &str) -> (Arc<Context>, Scratch) {
        let dir =
            std::env::temp_dir().join(format!("zapper-subtitles-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let ctx = Context::new(
            Client::at_with(
                &format!("{}/api/v1", server.base),
                Pacer::new(4, 30, Duration::from_secs(10)),
            ),
            dir.join(config::FILE),
            dir.join("subtitles"),
        );
        (Arc::new(ctx), Scratch(dir))
    }

    fn count(server: &Server, route: &str) -> usize {
        routes(server).iter().filter(|r| *r == route).count()
    }

    fn signed_in(ctx: &Context, token_at: i64) {
        config::update_in(&ctx.config_path, |c| {
            let os = &mut c.opensubtitles;
            os.username = "tester".into();
            os.password = "test-pass".into();
            os.token = Some("fake-os-token-1".into());
            os.token_at = Some(token_at);
        })
        .unwrap();
    }

    fn routes(server: &Server) -> Vec<String> {
        server
            .requests()
            .iter()
            .map(|r| r.route().to_owned())
            .collect()
    }

    fn found(file_id: i64, lang: &str, feature: Feature) -> Found {
        Found {
            file_id,
            language: lang.into(),
            release: format!("Release.{file_id}"),
            hearing_impaired: false,
            forced: false,
            trusted: false,
            downloads: 10,
            translated: false,
            feature,
        }
    }

    fn en() -> Vec<String> {
        vec!["en".into()]
    }

    #[test]
    fn search_plan_tries_the_id_then_the_name() {
        let _catalogue = db::movie_catalogue();
        seed();
        let plan = search_plan(&subject_of(VodItem::movie(FILM_1)), &en());
        let lookups: Vec<(Lookup, MatchedBy)> =
            plan.iter().map(|(p, by)| (p.lookup.clone(), *by)).collect();
        assert_eq!(
            lookups,
            [
                (Lookup::Tmdb(900001), MatchedBy::Id),
                (
                    Lookup::Name {
                        query: "film 0001".into(),
                        year: Some(2021)
                    },
                    MatchedBy::Name
                ),
            ]
        );
        for (p, _) in &plan {
            assert_eq!((p.season, p.episode), (None, None));
            assert_eq!(p.languages, en());
        }
        let plan = search_plan(&subject_of(VodItem::movie(FILM_2)), &en());
        assert_eq!(plan.len(), 1, "no TMDB id: the name alone");
        assert_eq!(
            plan[0].0.lookup,
            Lookup::Name {
                query: "film 0002".into(),
                year: Some(2019)
            }
        );
        assert_eq!(plan[0].1, MatchedBy::Name);

        // A name that carries its year searches without it.
        let mut s = subject_of(VodItem::movie(FILM_2));
        s.name = "Film 0002 2019".into();
        s.year = None;
        assert_eq!(
            search_plan(&s, &en())[0].0.lookup,
            Lookup::Name {
                query: "film 0002".into(),
                year: Some(2019)
            }
        );
        // A provider's language label is not part of the name.
        let query = |name: &str| {
            let mut s = subject_of(VodItem::movie(FILM_2));
            s.name = name.into();
            match search_plan(&s, &en()).pop().map(|(p, _)| p.lookup) {
                Some(Lookup::Name { query, .. }) => query,
                other => panic!("{other:?}"),
            }
        };
        assert_eq!(query("EN - Film 0001"), "film 0001");
        assert_eq!(query("FR/EN - Film 0001"), "film 0001");
        assert_eq!(query("PT-BR - Film 0001"), "film 0001");
        assert_eq!(query("EN: Film 0001"), "film 0001");
        assert_eq!(query("UK: Film 0001"), "film 0001");
        assert_eq!(query("ENG - Film 0001"), "film 0001");
        // Before a colon, three letters are a title's own word.
        assert_eq!(query("ABC: Show 0001"), "abc: show 0001");
        assert_eq!(query("ENG: Film 0001"), "eng: film 0001");
        assert_eq!(query("Garden - Lantern"), "garden - lantern");
        assert_eq!(
            query("Up - Film 0001"),
            "up - film 0001",
            "a word, not a label"
        );
        assert_eq!(query("EN-Film 0001"), "en-film 0001");
        assert_eq!(query("EN - "), "en", "nothing after the label");

        // Nothing to go by.
        s.name = " - ".into();
        assert!(search_plan(&s, &en()).is_empty());
    }

    #[test]
    fn search_plan_for_an_episode_uses_the_show() {
        let _catalogue = db::movie_catalogue();
        seed();
        let plan = search_plan(&subject_of(VodItem::episode(S1E2)), &en());
        assert_eq!(plan.len(), 2);
        assert_eq!(plan[0].0.lookup, Lookup::Tmdb(800001));
        assert_eq!(
            plan[1].0.lookup,
            Lookup::Name {
                query: "show 0001".into(),
                year: None
            }
        );
        for (p, _) in &plan {
            assert_eq!((p.season, p.episode), (Some(1), Some(2)));
        }
        let range = search_plan(&subject_of(VodItem::episode(S2E1_E2)), &en());
        assert_eq!((range[0].0.season, range[0].0.episode), (Some(2), Some(1)));
        let special = search_plan(&subject_of(VodItem::episode(SPECIAL)), &en());
        assert_eq!(
            (special[0].0.season, special[0].0.episode),
            (Some(0), Some(1))
        );
    }

    #[test]
    fn languages_are_cleaned() {
        let given: Vec<String> = ["EN", "en", "pt-BR", "fr"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(clean_languages(&given), ["en", "pt-br"]);
        let odd: Vec<String> = ["eng", "", "f", "fr;rm", "zh-hant", " de ", "es"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(clean_languages(&odd), ["de", "es"]);
    }

    #[test]
    fn rank_drops_other_episodes_and_translations() {
        let _catalogue = db::movie_catalogue();
        seed();
        let s = subject_of(VodItem::episode(S1E2));
        let ep = |season, episode| Feature {
            kind: Some("episode".into()),
            tmdb_id: None,
            parent_tmdb_id: Some(800001),
            season: Some(season),
            episode: Some(episode),
        };
        let mut rows = vec![
            found(1, "en", ep(1, 2)),
            found(2, "en", ep(1, 3)),
            Found {
                translated: true,
                ..found(3, "en", ep(1, 2))
            },
            Found {
                downloads: 500,
                ..found(4, "fr", ep(1, 2))
            },
            Found {
                forced: true,
                downloads: 900,
                ..found(5, "en", ep(1, 2))
            },
            Found {
                trusted: true,
                downloads: 5,
                ..found(6, "en", ep(1, 2))
            },
            Found {
                downloads: 100,
                ..found(7, "en", ep(1, 2))
            },
            found(8, "de", ep(1, 2)),
            found(
                9,
                "en",
                Feature {
                    kind: Some("movie".into()),
                    tmdb_id: Some(900001),
                    ..Feature::default()
                },
            ),
            found(10, "en", ep(2, 2)),
            // What OpenSubtitles does not say is not held against a row.
            found(11, "EN", Feature::default()),
        ];
        rows.reverse();
        let langs: Vec<String> = vec!["en".into(), "fr".into()];
        let ids: Vec<i64> = rank(rows, &s, &langs, &HashSet::new())
            .iter()
            .map(|r| r.file_id)
            .collect();
        assert_eq!(ids, [6, 7, 1, 11, 5, 4]);

        // At most RESULTS_MAX.
        let many: Vec<Found> = (0..80).map(|n| found(n, "en", ep(1, 2))).collect();
        assert_eq!(rank(many, &s, &en(), &HashSet::new()).len(), RESULTS_MAX);

        // A language without a region takes its regions; one with, only it.
        let pt = vec![found(1, "pt-BR", ep(1, 2)), found(2, "pt-PT", ep(1, 2))];
        assert_eq!(
            rank(pt.clone(), &s, &["pt".into()], &HashSet::new()).len(),
            2
        );
        let ids: Vec<i64> = rank(pt, &s, &["pt-br".into()], &HashSet::new())
            .iter()
            .map(|r| r.file_id)
            .collect();
        assert_eq!(ids, [1]);
    }

    #[test]
    fn rank_marks_what_is_on_this_computer() {
        let _catalogue = db::movie_catalogue();
        seed();
        let s = subject_of(VodItem::movie(FILM_1));
        let rows = crate::online::opensubtitles::parse_search(&page(&film_1_rows())).unwrap();
        let langs: Vec<String> = vec!["en".into(), "fr".into()];
        let ranked = rank(rows, &s, &langs, &HashSet::from([7000002]));
        let kept: Vec<(i64, bool)> = ranked.iter().map(|r| (r.file_id, r.kept)).collect();
        assert_eq!(kept, [(7000001, false), (7000002, true), (7000004, false)]);
        assert_eq!(ranked[0].release, "Film.0001.2021.1080p.TEST");
        assert!(ranked[0].trusted && ranked[1].hearing_impaired);
        assert_eq!(ranked[0].downloads, 1200);
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn an_empty_id_search_falls_back_to_the_name_once() {
        let _catalogue = db::movie_catalogue();
        seed();
        let server = serve(|req, _| search_answer(req));
        let (ctx, _scratch) = context(&server, "fallback");
        let answer = search_with(&ctx, VodItem::movie(FILM_WRONG_ID), en())
            .await
            .unwrap();
        let SubtitleSearchDto::Results {
            results,
            languages,
            matched_by,
        } = answer
        else {
            panic!("{answer:?}")
        };
        assert_eq!(matched_by, Some(MatchedBy::Name));
        assert_eq!(languages, en());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].file_id, 7000021);
        let seen = server.requests();
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0].query("tmdb_id"), Some("900999"));
        assert_eq!(seen[1].query("query"), Some("film+0002"));
        assert_eq!(seen[1].query("year"), Some("2019"));

        // Found by neither: two requests, and nothing matched.
        let answer = search_with(&ctx, VodItem::movie(NOWHERE), en())
            .await
            .unwrap();
        assert_eq!(
            answer,
            SubtitleSearchDto::Results {
                results: vec![],
                languages: en(),
                matched_by: None
            }
        );
        assert_eq!(server.requests().len(), 4);

        // Found by its id: one request.
        let answer = search_with(&ctx, VodItem::movie(FILM_1), en())
            .await
            .unwrap();
        let SubtitleSearchDto::Results {
            results,
            matched_by,
            ..
        } = answer
        else {
            panic!()
        };
        assert_eq!(matched_by, Some(MatchedBy::Id));
        let ids: Vec<i64> = results.iter().map(|r| r.file_id).collect();
        assert_eq!(ids, [7000001, 7000002], "English only, the AI one dropped");
        assert_eq!(server.requests().len(), 5);
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn searches_are_kept_for_six_hours() {
        let _catalogue = db::movie_catalogue();
        seed();
        let server = serve(|req, _| search_answer(req));
        let (ctx, _scratch) = context(&server, "kept-searches");
        let film = VodItem::movie(FILM_1);
        let first = search_with(&ctx, film, en()).await.unwrap();
        let again = search_with(&ctx, film, vec!["EN".into()]).await.unwrap();
        assert_eq!(first, again);
        assert_eq!(server.requests().len(), 1);
        // Other languages are another search.
        search_with(&ctx, film, vec!["fr".into()]).await.unwrap();
        assert_eq!(server.requests().len(), 2);

        // Another account's movie of that id is another film.
        db::with_db(|c| origin::record(c, ORIGIN_B)).unwrap();
        search_with(&ctx, film, en()).await.unwrap();
        assert_eq!(server.requests().len(), 3);
        db::with_db(|c| origin::record(c, ORIGIN_A)).unwrap();
        search_with(&ctx, film, en()).await.unwrap();
        assert_eq!(server.requests().len(), 3);

        // Six hours on, it is asked again; a failure is kept a minute.
        let key = SearchKey {
            origin: ORIGIN_A.into(),
            item: film,
            languages: en(),
        };
        let now = Instant::now();
        assert!(ctx.cache.get(&key, now).is_some());
        let later = now + Duration::from_secs(SEARCH_KEEP_SECS + 1);
        assert!(ctx.cache.get(&key, later).is_none());
        ctx.cache.put(key.clone(), Err("refused".into()), now);
        assert!(ctx.cache.get(&key, now + Duration::from_secs(59)).is_some());
        assert!(ctx
            .cache
            .get(&key, now + Duration::from_secs(FAILED_KEEP_SECS + 1))
            .is_none());
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn off_asks_nothing() {
        let _catalogue = db::movie_catalogue();
        seed();
        let server = stand_in();
        let (ctx, _scratch) = context(&server, "off");
        signed_in(&ctx, unix_now());
        config::update_in(&ctx.config_path, |c| c.opensubtitles.enabled = false).unwrap();
        let film = VodItem::movie(FILM_1);
        assert_eq!(
            search_with(&ctx, film, en()).await.unwrap(),
            SubtitleSearchDto::Off
        );
        assert_eq!(
            download_with(&ctx, film, 7000001).await.unwrap(),
            SubtitleDownloadDto::Off
        );
        assert!(sign_in_with(&ctx, "tester".into(), "test-pass".into())
            .await
            .is_err());
        assert_eq!(server.requests().len(), 0);
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn a_download_without_a_sign_in_never_asks_for_a_link() {
        let _catalogue = db::movie_catalogue();
        seed();
        let server = stand_in();
        let (ctx, _scratch) = context(&server, "no-sign-in");
        let film = VodItem::movie(FILM_1);
        assert_eq!(
            download_with(&ctx, film, 7000001).await.unwrap(),
            SubtitleDownloadDto::SignIn {
                refused: false,
                email: false
            }
        );
        // A username alone is no sign-in either.
        config::update_in(&ctx.config_path, |c| {
            c.opensubtitles.username = "tester".into()
        })
        .unwrap();
        assert!(matches!(
            download_with(&ctx, film, 7000001).await.unwrap(),
            SubtitleDownloadDto::SignIn { refused: false, .. }
        ));
        assert_eq!(server.requests().len(), 0);
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn a_download_is_kept_and_never_downloaded_again() {
        let _catalogue = db::movie_catalogue();
        seed();
        let server = stand_in();
        let (ctx, _scratch) = context(&server, "kept");
        signed_in(&ctx, unix_now());
        let film = VodItem::movie(FILM_1);
        search_with(&ctx, film, en()).await.unwrap();
        let got = download_with(&ctx, film, 7000001).await.unwrap();
        let SubtitleDownloadDto::Downloaded {
            subtitle,
            downloads_left,
        } = got
        else {
            panic!("{got:?}")
        };
        assert_eq!(subtitle.file_id, 7000001);
        assert_eq!(subtitle.language, "en");
        assert_eq!(subtitle.release, "Film.0001.2021.1080p.TEST");
        assert_eq!(downloads_left, Some(4));
        assert_eq!(
            routes(&server),
            [
                "/api/v1/subtitles",
                "/api/v1/download",
                "/files/7000001.srt"
            ]
        );
        let seen = server.requests();
        assert_eq!(
            seen[1].header("authorization"),
            Some("Bearer fake-os-token-1")
        );
        assert_eq!(seen[2].header("authorization"), None, "the file host");
        let path = ctx.files_dir.join("7000001.srt");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), srt(7000001, 2));
        assert!(!tmp_file(&ctx.files_dir, 7000001).exists());
        assert!(tmp_file(&ctx.files_dir, 7000001)
            .to_string_lossy()
            .ends_with(&format!("7000001.srt.{}.tmp", std::process::id())));
        let kept = db::with_db(|c| subtitle_store::kept(c, film)).unwrap();
        assert_eq!(kept, [subtitle.clone()]);
        assert_eq!(status_with(&ctx).open_subtitles.downloads_left, Some(4));

        // Chosen again, for this title or another: no request.
        assert!(matches!(
            download_with(&ctx, film, 7000001).await.unwrap(),
            SubtitleDownloadDto::Downloaded { .. }
        ));
        let other = VodItem::movie(FILM_2);
        let got = download_with(&ctx, other, 7000001).await.unwrap();
        let SubtitleDownloadDto::Downloaded { subtitle: s2, .. } = got else {
            panic!("{got:?}")
        };
        assert_eq!(
            (s2.language.as_str(), s2.release.as_str()),
            ("en", "Film.0001.2021.1080p.TEST")
        );
        assert_eq!(server.requests().len(), 3);
        let other_kept = db::with_db(|c| subtitle_store::kept(c, other)).unwrap();
        assert_eq!(other_kept.len(), 1);

        // And the search now says it is on this computer.
        let SubtitleSearchDto::Results { results, .. } =
            search_with(&ctx, film, en()).await.unwrap()
        else {
            panic!()
        };
        assert!(results.iter().find(|r| r.file_id == 7000001).unwrap().kept);
        assert!(!results.iter().find(|r| r.file_id == 7000002).unwrap().kept);
        assert_eq!(server.requests().len(), 3);
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn an_expired_token_is_renewed_once() {
        let _catalogue = db::movie_catalogue();
        seed();
        let film = VodItem::movie(FILM_1);

        // A 401 on the link: one sign-in, one retry.
        let downloads = Arc::new(AtomicUsize::new(0));
        let n = downloads.clone();
        let server = serve(move |req, base| match req.route() {
            "/api/v1/login" => login_ok(base),
            "/api/v1/download" if n.fetch_add(1, Ordering::SeqCst) == 0 => {
                Reply::new(401, r#"{"message":"invalid token"}"#)
            }
            "/api/v1/download" => link_to(base, "7000001.srt"),
            "/files/7000001.srt" => Reply::ok(srt(7000001, 2)),
            _ => Reply::new(404, ""),
        });
        let (ctx, _scratch) = context(&server, "renewed");
        signed_in(&ctx, unix_now());
        assert!(matches!(
            download_with(&ctx, film, 7000001).await.unwrap(),
            SubtitleDownloadDto::Downloaded { .. }
        ));
        assert_eq!(
            routes(&server),
            [
                "/api/v1/download",
                "/api/v1/login",
                "/api/v1/download",
                "/files/7000001.srt"
            ]
        );
        assert_eq!(
            server.requests()[2].header("authorization"),
            Some("Bearer fake-os-token-2")
        );
        let saved = config::load_from(&ctx.config_path).opensubtitles;
        assert_eq!(saved.token.as_deref(), Some("fake-os-token-2"));

        // A token past TOKEN_RENEW_SECS is renewed before it is used.
        let server = stand_in();
        let (ctx, _scratch) = context(&server, "renewed-old");
        signed_in(&ctx, unix_now() - TOKEN_RENEW_SECS - 1);
        assert!(matches!(
            download_with(&ctx, film, 7000001).await.unwrap(),
            SubtitleDownloadDto::Downloaded { .. }
        ));
        assert_eq!(
            routes(&server),
            ["/api/v1/login", "/api/v1/download", "/files/7000001.srt"]
        );

        // A second 401: the sign-in is asked for again.
        let server = serve(|req, base| match req.route() {
            "/api/v1/login" => login_ok(base),
            _ => Reply::new(401, r#"{"message":"invalid token"}"#),
        });
        let (ctx, _scratch) = context(&server, "renewed-twice");
        signed_in(&ctx, unix_now());
        assert_eq!(
            download_with(&ctx, film, 7000001).await.unwrap(),
            SubtitleDownloadDto::SignIn {
                refused: false,
                email: false
            }
        );
        assert_eq!(
            routes(&server),
            ["/api/v1/download", "/api/v1/login", "/api/v1/download"]
        );
        assert!(!ctx.files_dir.join("7000001.srt").exists());
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn logins_are_spaced() {
        let _catalogue = db::movie_catalogue();
        seed();
        let server = serve(|_, _| Reply::new(500, ""));
        let (ctx, _scratch) = context(&server, "spaced");
        signed_in(&ctx, unix_now() - TOKEN_RENEW_SECS - 1);
        let film = VodItem::movie(FILM_1);
        assert!(download_with(&ctx, film, 7000001).await.is_err());
        let before = unix_now();
        match download_with(&ctx, film, 7000001).await.unwrap() {
            SubtitleDownloadDto::Busy { retry_at } => {
                assert!(
                    retry_at > before && retry_at <= before + LOGIN_MIN_GAP.as_secs() as i64 + 1
                )
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(routes(&server), ["/api/v1/login"]);
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn a_refused_login_is_not_tried_again() {
        let _catalogue = db::movie_catalogue();
        seed();
        let server = serve(|_, _| Reply::new(401, r#"{"message":"Invalid username/password"}"#));
        let (ctx, _scratch) = context(&server, "refused");
        config::update_in(&ctx.config_path, |c| {
            c.opensubtitles.username = "tester".into();
            c.opensubtitles.password = "wrong-pass".into();
        })
        .unwrap();
        let film = VodItem::movie(FILM_1);
        let refused = SubtitleDownloadDto::SignIn {
            refused: true,
            email: false,
        };
        assert_eq!(download_with(&ctx, film, 7000001).await.unwrap(), refused);
        assert!(config::load_from(&ctx.config_path).opensubtitles.refused);
        assert!(status_with(&ctx).open_subtitles.sign_in_refused);
        assert_eq!(download_with(&ctx, film, 7000001).await.unwrap(), refused);
        assert_eq!(routes(&server), ["/api/v1/login"]);
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn an_email_as_username_says_so() {
        let _catalogue = db::movie_catalogue();
        seed();
        let server = serve(|_, _| Reply::new(400, r#"{"message":"Bad request"}"#));
        let (ctx, _scratch) = context(&server, "email");
        config::update_in(&ctx.config_path, |c| {
            c.opensubtitles.username = "tester@example.com".into();
            c.opensubtitles.password = "test-pass".into();
        })
        .unwrap();
        let film = VodItem::movie(FILM_1);
        let email = SubtitleDownloadDto::SignIn {
            refused: true,
            email: true,
        };
        assert_eq!(download_with(&ctx, film, 7000001).await.unwrap(), email);
        assert!(config::load_from(&ctx.config_path).opensubtitles.refused);
        assert_eq!(download_with(&ctx, film, 7000001).await.unwrap(), email);
        assert_eq!(routes(&server), ["/api/v1/login"]);
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn quota_spent_says_when_it_resets() {
        let _catalogue = db::movie_catalogue();
        seed();
        let server = serve(|_, _| {
            Reply::new(
                406,
                r#"{"message":"You have downloaded your allowed 20 subtitles for 24h","remaining":0,"reset_time_utc":"2036-11-08T12:00:00.000Z"}"#,
            )
        });
        let (ctx, _scratch) = context(&server, "quota");
        signed_in(&ctx, unix_now());
        assert_eq!(
            download_with(&ctx, VodItem::movie(FILM_1), 7000001)
                .await
                .unwrap(),
            SubtitleDownloadDto::Quota {
                reset_at: Some(RESET_AT)
            }
        );
        let s = status_with(&ctx).open_subtitles;
        assert_eq!(s.downloads_left, Some(0));
        assert_eq!(s.downloads_reset_at, Some(RESET_AT));
        assert_eq!(routes(&server), ["/api/v1/download"]);
        // Past its reset, a spent allowance is not known any more.
        ctx.account.set_quota(Some((0, Some(unix_now() - 1))));
        assert_eq!(status_with(&ctx).open_subtitles.downloads_left, None);
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn a_bad_file_is_not_kept() {
        let _catalogue = db::movie_catalogue();
        seed();
        let n = Arc::new(AtomicUsize::new(0));
        let server = serve(move |req, base| match req.route() {
            "/api/v1/download" => {
                let file = ["page", "empty", "huge"][n.fetch_add(1, Ordering::SeqCst) % 3];
                link_to(base, file)
            }
            "/files/page" => Reply::ok("<html><body>This link has expired</body></html>"),
            "/files/empty" => Reply::ok(""),
            "/files/huge" => Reply::ok(srt(7000003, 1).repeat(3 << 20 >> 6).into_bytes()),
            _ => Reply::new(404, ""),
        });
        let (ctx, _scratch) = context(&server, "bad-file");
        signed_in(&ctx, unix_now());
        let film = VodItem::movie(FILM_1);
        for file_id in [7000001, 7000002, 7000003] {
            let e = download_with(&ctx, film, file_id).await.unwrap_err();
            assert!(!e.contains(&server.base), "{e}");
            assert!(!ctx.files_dir.join(format!("{file_id}.srt")).exists());
        }
        assert!(db::with_db(|c| subtitle_store::kept(c, film))
            .unwrap()
            .is_empty());
        assert_eq!(server.requests().len(), 6);
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn one_file_is_asked_for_once_however_often_it_is_chosen() {
        let _catalogue = db::movie_catalogue();
        seed();
        let server = stand_in();
        let (ctx, _scratch) = context(&server, "once");
        signed_in(&ctx, unix_now());
        let (a, b) = tokio::join!(
            download_with(&ctx, VodItem::movie(FILM_1), 7000001),
            download_with(&ctx, VodItem::movie(FILM_2), 7000001),
        );
        assert!(matches!(a.unwrap(), SubtitleDownloadDto::Downloaded { .. }));
        assert!(matches!(b.unwrap(), SubtitleDownloadDto::Downloaded { .. }));
        assert_eq!(routes(&server), ["/api/v1/download", "/files/7000001.srt"]);
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn signing_in_and_out() {
        let _catalogue = db::movie_catalogue();
        seed();
        let server = stand_in();
        let (ctx, _scratch) = context(&server, "sign-in");
        assert!(sign_in_with(&ctx, " ".into(), "test-pass".into())
            .await
            .is_err());
        let s = sign_in_with(&ctx, " tester ".into(), "test-pass".into())
            .await
            .unwrap();
        assert_eq!(s.open_subtitles.signed_in_as.as_deref(), Some("tester"));
        let saved = config::load_from(&ctx.config_path).opensubtitles;
        assert_eq!(saved.token.as_deref(), Some("fake-os-token-2"));
        assert_eq!(saved.api_base.as_deref(), Some(ctx.client.base()));
        let wire = serde_json::to_string(&s).unwrap();
        assert!(!wire.contains("test-pass") && !wire.contains("fake-os-token"));
        // Signing in again at once is the user's own doing: never "busy".
        sign_in_with(&ctx, "tester".into(), "test-pass".into())
            .await
            .unwrap();

        let s = sign_out_with(&ctx).await.unwrap();
        assert_eq!(s.open_subtitles.signed_in_as, None);
        let saved = config::load_from(&ctx.config_path).opensubtitles;
        assert!(saved.username.is_empty() && saved.password.is_empty());
        assert_eq!(saved.token, None);
        assert_eq!(
            routes(&server),
            ["/api/v1/login", "/api/v1/login", "/api/v1/logout"]
        );
        assert_eq!(
            server.requests()[2].header("authorization"),
            Some("Bearer fake-os-token-2")
        );

        let s = set_opensubtitles_with(&ctx, Some(false), Some(vec!["FR".into(), "xx-yyy".into()]))
            .unwrap();
        assert!(!s.open_subtitles.enabled);
        assert_eq!(s.open_subtitles.languages, ["fr"]);
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn a_paid_link_is_used_again() {
        let _catalogue = db::movie_catalogue();
        seed();
        // The first link's host fails twice, the second's once.
        let links = Arc::new(AtomicUsize::new(0));
        let failures: Arc<Mutex<HashMap<String, usize>>> = Arc::default();
        let server = serve(move |req, base| {
            let route = req.route().to_owned();
            if route == "/api/v1/download" {
                let n = links.fetch_add(1, Ordering::SeqCst);
                return link_to(base, ["a.srt", "b.srt"][n.min(1)]);
            }
            let mut failed = failures.lock().unwrap();
            let times = failed.entry(route.clone()).or_default();
            let fails = match route.as_str() {
                "/files/a.srt" => 2,
                "/files/b.srt" => 1,
                _ => return Reply::new(404, ""),
            };
            *times += 1;
            if *times <= fails {
                Reply::new(500, "")
            } else {
                Reply::ok(srt(7000001, 2))
            }
        });
        let (ctx, _scratch) = context(&server, "paid-link");
        signed_in(&ctx, unix_now());
        let film = VodItem::movie(FILM_1);

        // Failed twice (asked again once): the link is kept, not paid again.
        assert!(download_with(&ctx, film, 7000001).await.is_err());
        assert!(!ctx.files_dir.join("7000001.srt").exists());
        assert!(matches!(
            download_with(&ctx, film, 7000001).await.unwrap(),
            SubtitleDownloadDto::Downloaded { .. }
        ));
        assert_eq!(count(&server, "/api/v1/download"), 1);
        assert_eq!(count(&server, "/files/a.srt"), 3);

        // Failed once: asked again in the same download.
        assert!(matches!(
            download_with(&ctx, film, 7000002).await.unwrap(),
            SubtitleDownloadDto::Downloaded { .. }
        ));
        assert_eq!(count(&server, "/api/v1/download"), 2);
        assert_eq!(count(&server, "/files/b.srt"), 2);
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn a_dropped_caller_still_keeps_the_file() {
        let _catalogue = db::movie_catalogue();
        seed();
        let server = serve(|req, base| match req.route() {
            "/api/v1/download" => link_to(base, "7000001.srt"),
            "/files/7000001.srt" => {
                std::thread::sleep(Duration::from_millis(300));
                Reply::ok(srt(7000001, 2))
            }
            _ => Reply::new(404, ""),
        });
        let (ctx, _scratch) = context(&server, "dropped");
        signed_in(&ctx, unix_now());
        let film = VodItem::movie(FILM_1);
        let gone = tokio::time::timeout(
            Duration::from_millis(100),
            download_with(&ctx, film, 7000001),
        )
        .await;
        assert!(gone.is_err(), "the caller gave up first");
        assert_eq!(
            count(&server, "/files/7000001.srt"),
            1,
            "the file was on its way"
        );
        let path = ctx.files_dir.join("7000001.srt");
        for _ in 0..100 {
            if path.exists() && !kept(film).unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
        assert!(path.exists(), "the paid-for file was kept");
        assert_eq!(kept(film).unwrap().len(), 1);
        assert_eq!(count(&server, "/api/v1/download"), 1);
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn renewals_are_shared() {
        let _catalogue = db::movie_catalogue();
        seed();
        let server = stand_in();
        let (ctx, _scratch) = context(&server, "shared-renewal");
        signed_in(&ctx, unix_now() - TOKEN_RENEW_SECS - 1);
        let (a, b) = tokio::join!(
            download_with(&ctx, VodItem::movie(FILM_1), 7000001),
            download_with(&ctx, VodItem::movie(FILM_1), 7000002),
        );
        assert!(matches!(a.unwrap(), SubtitleDownloadDto::Downloaded { .. }));
        assert!(matches!(b.unwrap(), SubtitleDownloadDto::Downloaded { .. }));
        assert_eq!(count(&server, "/api/v1/login"), 1, "one sign-in for both");
        assert_eq!(count(&server, "/api/v1/download"), 2);
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn a_renewal_leaves_a_sign_out_or_another_account_alone() {
        let _catalogue = db::movie_catalogue();
        seed();
        let film = VodItem::movie(FILM_1);
        for (case, other) in [("signed-out", ""), ("other-account", "someone-else")] {
            let dir = std::env::temp_dir().join(format!(
                "zapper-subtitles-renewal-{case}-{}",
                std::process::id()
            ));
            let path = dir.join(config::FILE);
            let during = path.clone();
            let other = other.to_owned();
            // While the renewal signs in, the user signs out, or in as
            // someone else.
            let server = serve(move |req, base| {
                if req.route() == "/api/v1/login" {
                    config::update_in(&during, |c| {
                        let os = &mut c.opensubtitles;
                        os.username = other.clone();
                        os.password = if other.is_empty() {
                            String::new()
                        } else {
                            "other-pass".into()
                        };
                        os.token = None;
                        os.token_at = None;
                    })
                    .unwrap();
                    return login_ok(base);
                }
                link_to(base, "7000001.srt")
            });
            let ctx = Arc::new(Context::new(
                Client::at_with(
                    &format!("{}/api/v1", server.base),
                    Pacer::new(4, 30, Duration::from_secs(10)),
                ),
                path.clone(),
                dir.join("subtitles"),
            ));
            let _scratch = Scratch(dir);
            signed_in(&ctx, unix_now() - TOKEN_RENEW_SECS - 1);
            assert_eq!(
                download_with(&ctx, film, 7000001).await.unwrap(),
                SubtitleDownloadDto::SignIn {
                    refused: false,
                    email: false
                },
                "{case}"
            );
            let saved = config::load_from(&path).opensubtitles;
            assert_eq!(saved.token, None, "{case}: the new token was not written");
            assert_eq!(saved.username, ctx.settings().username);
            assert_eq!(routes(&server), ["/api/v1/login"], "{case}");
        }
    }

    /// A stand-in whose every link is `/files/a.srt`, which answers
    /// `statuses` in turn and then the file.
    fn file_host(statuses: &'static [u16]) -> Server {
        let n = AtomicUsize::new(0);
        serve(move |req, base| match req.route() {
            "/api/v1/login" => login_ok(base),
            "/api/v1/logout" => Reply::ok("{}"),
            "/api/v1/download" => link_to(base, "a.srt"),
            "/files/a.srt" => match statuses.get(n.fetch_add(1, Ordering::SeqCst)) {
                Some(status) => Reply::new(*status, ""),
                None => Reply::ok(srt(7000001, 2)),
            },
            _ => Reply::new(404, ""),
        })
    }

    #[test]
    fn paid_links_expire_and_belong_to_their_user() {
        let paid = |at| PaidLink {
            at,
            username: "tester".into(),
            link: Some(Link {
                url: "http://127.0.0.1:9/files/a.srt".into(),
                remaining: Some(4),
                reset_at: None,
            }),
        };
        let a = Account::default();
        let t0 = Instant::now();
        a.put_link(1, paid(t0));
        assert!(
            a.take_link(1, "tester", t0 + LINK_KEEP).is_none(),
            "expired"
        );
        a.put_link(1, paid(t0));
        assert!(a.take_link(1, "someone-else", t0).is_none());
        assert!(
            a.take_link(1, "tester", t0).is_none(),
            "another's is forgotten"
        );
        a.put_link(1, paid(t0));
        a.put_link(2, paid(t0));
        assert!(a.take_link(1, "tester", t0).is_some());
        a.forget_links();
        assert!(a.take_link(2, "tester", t0).is_none());
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn a_kept_link_is_not_used_after_a_sign_out() {
        let _catalogue = db::movie_catalogue();
        seed();
        let server = file_host(&[500, 500, 500, 500]);
        let (ctx, _scratch) = context(&server, "link-sign-out");
        signed_in(&ctx, unix_now());
        let film = VodItem::movie(FILM_1);
        assert!(download_with(&ctx, film, 7000001).await.is_err());
        sign_out_with(&ctx).await.unwrap();
        assert_eq!(
            download_with(&ctx, film, 7000001).await.unwrap(),
            SubtitleDownloadDto::SignIn {
                refused: false,
                email: false
            }
        );
        assert_eq!(count(&server, "/api/v1/download"), 1);
        assert_eq!(count(&server, "/files/a.srt"), 2, "the link was not used");
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn a_kept_link_is_not_used_by_another_user() {
        let _catalogue = db::movie_catalogue();
        seed();
        let server = file_host(&[500, 500]);
        let (ctx, _scratch) = context(&server, "link-other-user");
        signed_in(&ctx, unix_now());
        let film = VodItem::movie(FILM_1);
        assert!(download_with(&ctx, film, 7000001).await.is_err());
        sign_in_with(&ctx, "someone-else".into(), "other-pass".into())
            .await
            .unwrap();
        assert!(matches!(
            download_with(&ctx, film, 7000001).await.unwrap(),
            SubtitleDownloadDto::Downloaded { .. }
        ));
        assert_eq!(count(&server, "/api/v1/download"), 2, "paid for again");
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn a_reused_link_refused_is_dropped() {
        let _catalogue = db::movie_catalogue();
        seed();
        // Failed twice, then refused when used again, then fine.
        let server = file_host(&[500, 500, 410]);
        let (ctx, _scratch) = context(&server, "link-refused");
        signed_in(&ctx, unix_now());
        let film = VodItem::movie(FILM_1);
        assert!(download_with(&ctx, film, 7000001).await.is_err());
        assert!(download_with(&ctx, film, 7000001).await.is_err());
        assert_eq!(count(&server, "/api/v1/download"), 1, "used again once");
        assert!(matches!(
            download_with(&ctx, film, 7000001).await.unwrap(),
            SubtitleDownloadDto::Downloaded { .. }
        ));
        assert_eq!(count(&server, "/api/v1/download"), 2, "then paid for again");
        assert_eq!(count(&server, "/files/a.srt"), 4);
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn a_counted_download_whose_answer_was_lost_is_not_paid_for_again() {
        let _catalogue = db::movie_catalogue();
        seed();
        let server = serve(|req, _| match req.route() {
            "/api/v1/download" => Reply::ok("<html>not json</html>"),
            _ => Reply::new(404, ""),
        });
        let (ctx, _scratch) = context(&server, "answer-lost");
        signed_in(&ctx, unix_now());
        let film = VodItem::movie(FILM_1);
        let lost = Err(OnlineError::AnswerLost.to_string());
        assert_eq!(download_with(&ctx, film, 7000001).await, lost);
        assert_eq!(download_with(&ctx, film, 7000001).await, lost);
        assert_eq!(count(&server, "/api/v1/download"), 1, "counted once");
        // Another file is asked for as usual.
        assert_eq!(download_with(&ctx, film, 7000002).await, lost);
        assert_eq!(count(&server, "/api/v1/download"), 2);
        // Another user (signed in by the other process sharing the
        // settings) pays for their own.
        config::update_in(&ctx.config_path, |c| {
            c.opensubtitles.username = "someone-else".into()
        })
        .unwrap();
        assert_eq!(download_with(&ctx, film, 7000001).await, lost);
        assert_eq!(count(&server, "/api/v1/download"), 3);
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn turned_off_while_waiting_asks_nothing() {
        let _catalogue = db::movie_catalogue();
        seed();
        let server = stand_in();
        let (ctx, _scratch) = context(&server, "off-after-wait");
        signed_in(&ctx, unix_now() - TOKEN_RENEW_SECS - 1);
        let film = VodItem::movie(FILM_1);
        // Another download of the file holds it while the service is
        // turned off.
        let held = ctx.account.file_lock(7000001);
        let turn = held.lock().await;
        let waiting = tokio::spawn({
            let ctx = Arc::clone(&ctx);
            async move { download_with(&ctx, film, 7000001).await }
        });
        tokio::time::sleep(Duration::from_millis(200)).await;
        config::update_in(&ctx.config_path, |c| c.opensubtitles.enabled = false).unwrap();
        drop(turn);
        assert_eq!(waiting.await.unwrap(), Ok(SubtitleDownloadDto::Off));
        // A renewal that waited on another finds it off too.
        let seen = ctx.settings();
        assert_eq!(renew(&ctx, &seen).await.err(), Some(OnlineError::Off));
        assert_eq!(server.requests().len(), 0);
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn a_new_sign_in_survives_a_second_401() {
        let _catalogue = db::movie_catalogue();
        seed();
        let dir = std::env::temp_dir().join(format!(
            "zapper-subtitles-second-401-{}",
            std::process::id()
        ));
        let path = dir.join(config::FILE);
        let during = path.clone();
        let n = AtomicUsize::new(0);
        // The second refusal comes as the user signs in again elsewhere.
        let server = serve(move |req, base| match req.route() {
            "/api/v1/login" => login_ok(base),
            _ => {
                if n.fetch_add(1, Ordering::SeqCst) == 1 {
                    config::update_in(&during, |c| {
                        c.opensubtitles.token = Some("fake-os-token-9".into());
                        c.opensubtitles.token_at = Some(unix_now());
                    })
                    .unwrap();
                }
                Reply::new(401, "")
            }
        });
        let ctx = Arc::new(Context::new(
            Client::at_with(
                &format!("{}/api/v1", server.base),
                Pacer::new(4, 30, Duration::from_secs(10)),
            ),
            path.clone(),
            dir.join("subtitles"),
        ));
        let _scratch = Scratch(dir);
        signed_in(&ctx, unix_now());
        assert_eq!(
            download_with(&ctx, VodItem::movie(FILM_1), 7000001)
                .await
                .unwrap(),
            SubtitleDownloadDto::SignIn {
                refused: false,
                email: false
            }
        );
        assert_eq!(
            config::load_from(&path).opensubtitles.token.as_deref(),
            Some("fake-os-token-9"),
            "the new sign-in's token stays"
        );
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn a_stale_renewal_does_not_refuse_a_new_sign_in() {
        let _catalogue = db::movie_catalogue();
        seed();
        let dir = std::env::temp_dir().join(format!(
            "zapper-subtitles-stale-renewal-{}",
            std::process::id()
        ));
        let path = dir.join(config::FILE);
        let during = path.clone();
        // While the old password is refused, the user signs in again with
        // the right one.
        let server = serve(move |req, _| {
            if req.route() == "/api/v1/login" {
                config::update_in(&during, |c| {
                    let os = &mut c.opensubtitles;
                    os.password = "new-pass".into();
                    os.token = Some("fake-os-token-9".into());
                    os.token_at = Some(unix_now());
                })
                .unwrap();
            }
            Reply::new(401, "")
        });
        let ctx = Arc::new(Context::new(
            Client::at_with(
                &format!("{}/api/v1", server.base),
                Pacer::new(4, 30, Duration::from_secs(10)),
            ),
            path.clone(),
            dir.join("subtitles"),
        ));
        let _scratch = Scratch(dir);
        signed_in(&ctx, unix_now() - TOKEN_RENEW_SECS - 1);
        let _ = download_with(&ctx, VodItem::movie(FILM_1), 7000001).await;
        let saved = config::load_from(&path).opensubtitles;
        assert!(!saved.refused, "the new sign-in is not marked refused");
        assert_eq!(saved.token.as_deref(), Some("fake-os-token-9"));
        assert_eq!(routes(&server), ["/api/v1/login"]);
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn a_download_is_kept_under_the_account_it_was_asked_in() {
        let _catalogue = db::movie_catalogue();
        seed();
        // Another account's catalogue lands while the file is on its way.
        let server = serve(|req, base| match req.route() {
            "/api/v1/download" => link_to(base, "7000001.srt"),
            "/files/7000001.srt" => {
                db::with_db(|c| origin::record(c, ORIGIN_B)).unwrap();
                Reply::ok(srt(7000001, 2))
            }
            _ => Reply::new(404, ""),
        });
        let (ctx, _scratch) = context(&server, "origin-switch");
        signed_in(&ctx, unix_now());
        let film = VodItem::movie(FILM_1);
        assert!(matches!(
            download_with(&ctx, film, 7000001).await.unwrap(),
            SubtitleDownloadDto::Downloaded { .. }
        ));
        assert!(
            kept(film).unwrap().is_empty(),
            "nothing under the new account"
        );
        db::with_db(|c| origin::record(c, ORIGIN_A)).unwrap();
        assert_eq!(
            kept(film).unwrap().len(),
            1,
            "kept under the one it was asked in"
        );
        assert!(ctx.files_dir.join("7000001.srt").exists());
    }

    #[test]
    fn kept_files_belong_to_their_account() {
        let _catalogue = db::movie_catalogue();
        seed();
        let server = stand_in();
        let (ctx, _scratch) = context(&server, "account");
        let film = VodItem::movie(FILM_1);
        std::fs::create_dir_all(&ctx.files_dir).unwrap();
        std::fs::write(ctx.files_dir.join("7000001.srt"), srt(7000001, 2)).unwrap();
        let s = KeptSubtitleDto {
            file_id: 7000001,
            language: "en".into(),
            release: "Film.0001.2021.1080p.TEST".into(),
            hearing_impaired: false,
            downloaded_at: 1_794_139_200,
        };
        db::with_db(|c| subtitle_store::keep(c, film, &s)).unwrap();
        assert_eq!(kept(film).unwrap(), [s.clone()]);
        assert!(vtt_in(&ctx.files_dir, film, 7000001).unwrap().is_some());

        db::with_db(|c| origin::record(c, ORIGIN_B)).unwrap();
        assert!(kept(film).unwrap().is_empty());
        assert_eq!(vtt_in(&ctx.files_dir, film, 7000001).unwrap(), None);
        assert_eq!(kept_path_in(&ctx.files_dir, film, 7000001).unwrap(), None);
        db::with_db(|c| origin::record(c, ORIGIN_A)).unwrap();
        assert_eq!(
            kept_path_in(&ctx.files_dir, film, 7000001).unwrap(),
            Some(ctx.files_dir.join("7000001.srt"))
        );
    }

    #[test]
    fn the_kept_file_reads_as_webvtt() {
        let _catalogue = db::movie_catalogue();
        seed();
        let server = stand_in();
        let (ctx, _scratch) = context(&server, "webvtt");
        let film = VodItem::movie(FILM_1);
        std::fs::create_dir_all(&ctx.files_dir).unwrap();
        std::fs::write(ctx.files_dir.join("7000001.srt"), srt(7000001, 2)).unwrap();
        let s = KeptSubtitleDto {
            file_id: 7000001,
            language: "en".into(),
            release: "Film.0001.2021.1080p.TEST".into(),
            hearing_impaired: false,
            downloaded_at: 1_794_139_200,
        };
        db::with_db(|c| subtitle_store::keep(c, film, &s)).unwrap();
        assert_eq!(
            vtt_in(&ctx.files_dir, film, 7000001).unwrap().as_deref(),
            Some("WEBVTT\n\n00:00:01.000 --> 00:00:03.000\nTest line 1 of file 7000001\n\n00:00:03.000 --> 00:00:05.000\nTest line 2 of file 7000001\n")
        );
        assert_eq!(vtt_in(&ctx.files_dir, film, 7000002).unwrap(), None);
        assert_eq!(
            file_path(7000001),
            crate::paths::data_dir()
                .join("subtitles")
                .join("7000001.srt")
        );
    }
}
