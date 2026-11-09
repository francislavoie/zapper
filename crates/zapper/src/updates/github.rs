//! zapper's own releases on GitHub: the list from the REST API, the assets
//! from the release download URLs. Never with a token, and never anything
//! about the user's account or server — the request says which version is
//! asking and nothing else.
//!
//! Where a download may go is pinned rather than trusted from the API's
//! answer: it must start under `https://github.com/<RELEASES_REPO>/releases/
//! download/`, and may only be redirected to GitHub's asset storage hosts. A
//! compromised or spoofed listing therefore cannot point an install at an
//! arbitrary server, and the SHA256SUMS that vouches for the package comes
//! from the same pinned place.

use super::release::{GhRelease, SUMS_ASSET};
use reqwest::Url;
use super::{BoxFuture, Progress, UpdateSource};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

pub const GITHUB_API: &str = "https://api.github.com";
pub const GITHUB_WEB: &str = "https://github.com";
/// Where GitHub redirects a release download: its storage, under the old name
/// and the one it moved to in 2025.
pub const ASSET_HOSTS: [&str; 2] = [
    "https://objects.githubusercontent.com",
    "https://release-assets.githubusercontent.com",
];
/// The largest package zapper will download. The `.deb` is a few megabytes;
/// this only stops a runaway response from filling the disk.
pub const MAX_PACKAGE: u64 = 512 * 1024 * 1024;
/// SHA256SUMS at most.
pub const MAX_SUMS: u64 = 64 * 1024;
/// Releases per check, newest first. Enough to skip a run of drafts and
/// pre-releases without paging.
const PER_PAGE: u32 = 30;
/// A list or a SHA256SUMS that takes longer than this is a failed check, not
/// a hang the next check has to queue behind.
const SMALL_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// A package download has no overall deadline (a slow line is still a
/// line), only a limit on silence.
const READ_TIMEOUT: Duration = Duration::from_secs(60);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

pub struct GitHubReleases {
    /// For the API: follows no redirects. A moved repository answers 301, and
    /// following it would quietly take updates from wherever it now points.
    api_http: reqwest::Client,
    /// For assets: follows redirects only to `allowed` places.
    files_http: reqwest::Client,
    repo: String,
    api: String,
    allow: Arc<Allow>,
    /// SHA256SUMS's deadline: [`SMALL_REQUEST_TIMEOUT`], shorter in tests.
    sums_timeout: Duration,
}

/// The places a download may start from or be redirected to, as parsed URLs.
///
/// Every check is made on a URL parsed the way the request will be made, never
/// on the raw text: a WHATWG parser drops tabs and newlines and resolves `.`
/// and `..`, so a string that looks like it is under this repository's
/// releases can name another repository by the time it is requested. Input
/// with control characters or whitespace is refused before parsing, and what
/// is requested is the parsed URL that was checked.
struct Allow {
    repo: String,
    web: Url,
    asset_hosts: Vec<Url>,
}

/// A URL from outside (the API's answer), parsed strictly: no control
/// characters or whitespace anywhere in the text, no credentials.
fn parse_strict(raw: &str) -> Option<Url> {
    if raw.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return None;
    }
    let url = Url::parse(raw).ok()?;
    (url.username().is_empty() && url.password().is_none()).then_some(url)
}

/// `url`'s path segments after `base`'s, when `url` has `base`'s scheme, host
/// and port and its path starts with `base`'s. Refuses any empty, `.` or `..`
/// segment, and any `%` or `\` in one: an encoded `/` or `.` could lead
/// elsewhere once a server decodes it.
fn segments_under(url: &Url, base: &Url) -> Option<Vec<String>> {
    if url.scheme() != base.scheme()
        || url.host_str() != base.host_str()
        || url.port_or_known_default() != base.port_or_known_default()
    {
        return None;
    }
    let path: Vec<&str> = url.path_segments()?.collect();
    if path
        .iter()
        .any(|s| s.is_empty() || *s == "." || *s == ".." || s.contains(['%', '\\']))
    {
        return None;
    }
    let prefix: Vec<&str> = base
        .path_segments()
        .map(|s| s.filter(|p| !p.is_empty()).collect())
        .unwrap_or_default();
    path.starts_with(&prefix)
        .then(|| path[prefix.len()..].iter().map(|s| s.to_string()).collect())
}

impl Allow {
    fn allowed(&self, url: &Url) -> bool {
        self.release_download(url)
            || self.asset_hosts.iter().any(|h| segments_under(url, h).is_some())
    }

    /// `<web>/<owner>/<repo>/releases/download/<tag>/<file>` of the
    /// configured repository, exactly those seven segments: scheme, host and
    /// port as configured, owner and repo in any case (GitHub's own rule).
    fn release_download(&self, url: &Url) -> bool {
        let Some(rest) = segments_under(url, &self.web) else {
            return false;
        };
        match rest.as_slice() {
            [owner, repo, releases, download, _tag, _file] => {
                releases == "releases"
                    && download == "download"
                    && self.repo.eq_ignore_ascii_case(&format!("{owner}/{repo}"))
            }
            _ => false,
        }
    }

    /// The parsed URL to request, if `raw` is one of the repository's files.
    fn start(&self, raw: &str) -> Result<Url, String> {
        parse_strict(raw)
            .filter(|u| self.release_download(u))
            .ok_or_else(|| format!("refusing to download from {raw:?}: not one of {}'s release files", self.repo))
    }
}

fn base_url(raw: &str) -> Result<Url, String> {
    parse_strict(raw.trim_end_matches('/')).ok_or_else(|| format!("{raw:?} is not a URL"))
}

fn user_agent() -> String {
    format!("zapper/{}", env!("CARGO_PKG_VERSION"))
}

impl GitHubReleases {
    /// github.com's releases of `repo` (`owner/name`).
    pub fn github(repo: &str) -> Result<Self, String> {
        Self::new(
            repo,
            GITHUB_API,
            GITHUB_WEB,
            ASSET_HOSTS.iter().map(|h| h.to_string()).collect(),
        )
    }

    /// At other endpoints, for tests: a local server standing in for all three.
    pub fn new(repo: &str, api: &str, web: &str, asset_hosts: Vec<String>) -> Result<Self, String> {
        let allow = Arc::new(Allow {
            repo: repo.to_string(),
            web: base_url(web)?,
            asset_hosts: asset_hosts.iter().map(|h| base_url(h)).collect::<Result<_, _>>()?,
        });
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::ACCEPT,
            reqwest::header::HeaderValue::from_static("application/vnd.github+json"),
        );
        headers.insert(
            "X-GitHub-Api-Version",
            reqwest::header::HeaderValue::from_static("2022-11-28"),
        );
        let api_http = reqwest::Client::builder()
            .user_agent(user_agent())
            .default_headers(headers)
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(SMALL_REQUEST_TIMEOUT)
            .build()
            .map_err(|e| format!("building the update client: {e}"))?;
        let policy_allow = allow.clone();
        let files_http = reqwest::Client::builder()
            .user_agent(user_agent())
            .redirect(reqwest::redirect::Policy::custom(move |attempt| {
                if attempt.previous().len() >= 5 {
                    attempt.error("too many redirects")
                } else if policy_allow.allowed(attempt.url()) {
                    attempt.follow()
                } else {
                    let to = attempt.url().host_str().unwrap_or("?").to_string();
                    attempt.error(format!("redirected to {to}, which is not GitHub's release storage"))
                }
            }))
            .connect_timeout(CONNECT_TIMEOUT)
            .read_timeout(READ_TIMEOUT)
            .build()
            .map_err(|e| format!("building the update client: {e}"))?;
        Ok(Self {
            api_http,
            files_http,
            repo: repo.to_string(),
            api: api.trim_end_matches('/').to_string(),
            allow,
            sums_timeout: SMALL_REQUEST_TIMEOUT,
        })
    }

    async fn get_ok(&self, req: reqwest::RequestBuilder) -> Result<reqwest::Response, String> {
        let resp = req.send().await.map_err(describe)?;
        let status = resp.status();
        if status.is_redirection() {
            return Err(format!("GitHub answered {status} and zapper didn't follow it"));
        }
        if !status.is_success() {
            // GitHub's unauthenticated limit is 60 requests an hour per
            // address; worth saying in words rather than as a bare 403.
            let limited = status.as_u16() == 403 || status.as_u16() == 429;
            return Err(if limited {
                format!("GitHub refused the request (HTTP {status}); probably its rate limit. Try again later.")
            } else {
                format!("GitHub answered HTTP {status}")
            });
        }
        Ok(resp)
    }
}

/// reqwest's errors, without the URL it likes to append (it is ours and long)
/// but with the cause, which is the useful part ("dns error", "timed out").
fn describe(e: reqwest::Error) -> String {
    let e = e.without_url();
    let mut msg = e.to_string();
    let mut source = std::error::Error::source(&e);
    while let Some(s) = source {
        msg.push_str(": ");
        msg.push_str(&s.to_string());
        source = s.source();
    }
    msg
}

impl UpdateSource for GitHubReleases {
    fn releases(&self) -> BoxFuture<'_, Vec<GhRelease>> {
        Box::pin(async move {
            let url = format!("{}/repos/{}/releases?per_page={PER_PAGE}", self.api, self.repo);
            let resp = self.get_ok(self.api_http.get(&url)).await?;
            resp.json::<Vec<GhRelease>>()
                .await
                .map_err(|e| format!("GitHub's release list didn't parse: {}", describe(e)))
        })
    }

    /// SHA256SUMS: small, so the whole exchange has a deadline (the files
    /// client deliberately has none, for packages on slow lines), and it stops
    /// between chunks once the download it belongs to is cancelled.
    fn fetch<'a>(&'a self, url: &'a str, cancel: Arc<AtomicBool>) -> BoxFuture<'a, Vec<u8>> {
        Box::pin(async move {
            let parsed = self.allow.start(url)?;
            let read = async {
                let mut resp = self
                    .get_ok(self.files_http.get(parsed).timeout(self.sums_timeout))
                    .await?;
                let mut body = Vec::new();
                while let Some(chunk) = resp.chunk().await.map_err(describe)? {
                    if cancel.load(Ordering::SeqCst) {
                        return Err("cancelled".to_string());
                    }
                    body.extend_from_slice(&chunk);
                    if body.len() as u64 > MAX_SUMS {
                        return Err(format!("{SUMS_ASSET} is larger than {MAX_SUMS} bytes"));
                    }
                }
                Ok(body)
            };
            // The request's own timeout should already bound this; the outer
            // one is the guarantee whatever the client's settings become.
            tokio::time::timeout(self.sums_timeout, read)
                .await
                .unwrap_or_else(|_| Err(format!("{SUMS_ASSET} took longer than {}s", self.sums_timeout.as_secs())))
        })
    }

    fn download<'a>(
        &'a self,
        url: &'a str,
        dest: &'a Path,
        progress: Progress,
        cancel: Arc<AtomicBool>,
    ) -> BoxFuture<'a, u64> {
        Box::pin(async move {
            let got = async {
                let parsed = self.allow.start(url)?;
                let mut resp = self.get_ok(self.files_http.get(parsed)).await?;
                if resp.content_length().is_some_and(|n| n > MAX_PACKAGE) {
                    return Err(format!("the package is larger than {} MB", MAX_PACKAGE / (1024 * 1024)));
                }
                // Plain std writes: chunks are tens of kilobytes into the
                // page cache, microseconds each, and tokio's async file needs
                // a feature this crate does not otherwise use.
                let mut file = std::fs::File::create(dest).map_err(|e| format!("{}: {e}", dest.display()))?;
                let mut total = 0u64;
                while let Some(chunk) = resp.chunk().await.map_err(describe)? {
                    if cancel.load(Ordering::SeqCst) {
                        return Err("cancelled".to_string());
                    }
                    total += chunk.len() as u64;
                    if total > MAX_PACKAGE {
                        return Err(format!("the package is larger than {} MB", MAX_PACKAGE / (1024 * 1024)));
                    }
                    file.write_all(&chunk).map_err(|e| format!("{}: {e}", dest.display()))?;
                    progress(total);
                }
                file.sync_all().map_err(|e| format!("{}: {e}", dest.display()))?;
                Ok(total)
            }
            .await;
            if got.is_err() {
                let _ = std::fs::remove_file(dest);
            }
            got
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::net::TcpListener;
    use std::sync::Mutex;

    fn allow() -> Allow {
        Allow {
            repo: "Someone/zapper".into(),
            web: base_url(GITHUB_WEB).unwrap(),
            asset_hosts: ASSET_HOSTS.iter().map(|h| base_url(h).unwrap()).collect(),
        }
    }

    #[test]
    fn downloads_start_only_under_the_repos_release_files() {
        let a = allow();
        for ok in [
            "https://github.com/Someone/zapper/releases/download/v0.2.0/zapper_0.2.0_amd64.deb",
            "https://github.com/someone/ZAPPER/releases/download/v0.2.0/SHA256SUMS",
            "https://github.com:443/Someone/zapper/releases/download/v0.2.0/SHA256SUMS",
        ] {
            let url = a.start(ok).unwrap_or_else(|e| panic!("{ok}: {e}"));
            assert!(a.release_download(&url), "{ok}");
        }
        for bad in [
            "https://github.com/Other/zapper/releases/download/v0.2.0/zapper_0.2.0_amd64.deb",
            "https://github.com/Someone/zapper/releases/tag/v0.2.0",
            "https://github.com/Someone/zapper/releases/download/",
            "https://github.com/Someone/zapper/releases/download/v1",
            "https://github.com/Someone/zapper/releases/download/v1/",
            "https://github.com.evil.example/Someone/zapper/releases/download/v1/x",
            "http://github.com/Someone/zapper/releases/download/v1/x",
            "https://github.com:8443/Someone/zapper/releases/download/v1/x",
            "https://user:pw@github.com/Someone/zapper/releases/download/v1/x",
            "https://github.com/Someone/zapper/releases/download/../../Other/zapper/x",
            "https://github.com/Someone/zapper/releases/download/v1/%2e%2e/x",
            "https://github.com/Someone%2fzapper/releases/download/v1/x",
            "https://github.com/Someone/zapper/releases/download/v1/a\\b",
            "https://evil.example/https://github.com/Someone/zapper/releases/download/v1/x",
            // Tabs and newlines that a WHATWG parser strips, leaving dot
            // segments that it then resolves: the raw text looks like this
            // repository's, the request would go to another's.
            "https://github.com/Someone/zapper/releases/download/v1/.\t./.\n./.\t./.\t./Evil/repo/releases/download/v9/x.deb",
            "https://github.com/Someone/zapper/releases/download/v1/x.deb\r",
            "https://github.com/Someone/zapper/releases/download/v1/x y.deb",
        ] {
            assert!(a.start(bad).is_err(), "{bad:?}");
        }
        // The same trick without the control characters: the parser resolves
        // the dot segments, and the check sees where the request would go.
        let resolved = Url::parse(
            "https://github.com/Someone/zapper/releases/download/v1/../../../../../Evil/repo/releases/download/v9/x.deb",
        )
        .unwrap();
        assert_eq!(resolved.path(), "/Evil/repo/releases/download/v9/x.deb");
        assert!(!a.release_download(&resolved));
    }

    #[test]
    fn redirects_may_reach_githubs_storage_and_nothing_else() {
        let a = allow();
        let url = |s: &str| Url::parse(s).unwrap();
        assert!(a.allowed(&url("https://objects.githubusercontent.com/github-production-release-asset/1?sig=x")));
        assert!(a.allowed(&url("https://release-assets.githubusercontent.com/x")));
        assert!(!a.allowed(&url("https://objects.githubusercontent.com.evil.example/x")));
        assert!(!a.allowed(&url("https://evil.example/x")));
        assert!(!a.allowed(&url("http://objects.githubusercontent.com/x")), "https only");
    }

    /// Serves canned answers on a local port, one connection per request,
    /// and records each request's first line.
    struct Server {
        base: String,
        seen: Arc<Mutex<Vec<String>>>,
    }

    fn serve(answer: impl Fn(&str, &str) -> (u16, Vec<(String, String)>, Vec<u8>) + Send + 'static) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        let me = base.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut first = String::new();
                reader.read_line(&mut first).unwrap();
                let mut line = String::new();
                while reader.read_line(&mut line).is_ok() && line != "\r\n" && !line.is_empty() {
                    line.clear();
                }
                let path = first.split_whitespace().nth(1).unwrap_or("").to_string();
                log.lock().unwrap().push(first.trim().to_string());
                let (status, headers, body) = answer(&me, &path);
                let mut out = format!("HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n", body.len());
                for (k, v) in headers {
                    out.push_str(&format!("{k}: {v}\r\n"));
                }
                out.push_str("\r\n");
                let _ = stream.write_all(out.as_bytes());
                let _ = stream.write_all(&body);
            }
        });
        Server { base, seen }
    }

    fn source(s: &Server) -> GitHubReleases {
        GitHubReleases::new(
            "Someone/zapper",
            &format!("{}/api", s.base),
            &format!("{}/web", s.base),
            vec![format!("{}/objects", s.base)],
        )
        .unwrap()
    }

    #[tokio::test]
    async fn lists_the_releases_in_one_request() {
        let s = serve(|_, _| {
            (200, vec![], br#"[{"tag_name":"v0.3.0","html_url":"h","assets":[{"name":"SHA256SUMS","size":10,"browser_download_url":"u"}]}]"#.to_vec())
        });
        let list = source(&s).releases().await.unwrap();
        assert_eq!(list[0].tag_name, "v0.3.0");
        assert_eq!(list[0].assets[0].name, "SHA256SUMS");
        let seen = s.seen.lock().unwrap().clone();
        assert_eq!(seen, ["GET /api/repos/Someone/zapper/releases?per_page=30 HTTP/1.1"]);
    }

    #[tokio::test]
    async fn the_api_redirecting_elsewhere_is_a_failed_check() {
        let s = serve(|_, _| (301, vec![("Location".into(), "https://example.com/repos/x/releases".into())], vec![]));
        let e = source(&s).releases().await.unwrap_err();
        assert!(e.contains("didn't follow it"), "{e}");
    }

    #[tokio::test]
    async fn downloads_follow_the_redirect_to_storage_only() {
        let body: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        let served = body.clone();
        let s = serve(move |me, path| {
            if path.starts_with("/web/Someone/zapper/releases/download/v0.3.0/good") {
                (302, vec![("Location".into(), format!("{me}/objects/abc?sig=1"))], vec![])
            } else if path.starts_with("/web/") {
                (302, vec![("Location".into(), "http://127.0.0.2:9/stolen".into())], vec![])
            } else {
                (200, vec![], served.clone())
            }
        });
        let r = source(&s);
        let dir = std::env::temp_dir().join(format!("zapper-dl-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("pkg.part");
        let last = Arc::new(Mutex::new(0u64));
        let l = last.clone();
        let url = format!("{}/web/Someone/zapper/releases/download/v0.3.0/good", s.base);
        let n = r
            .download(&url, &dest, Arc::new(move |n| *l.lock().unwrap() = n), Arc::new(AtomicBool::new(false)))
            .await
            .unwrap();
        assert_eq!(n, body.len() as u64);
        assert_eq!(*last.lock().unwrap(), n, "progress reached the end");
        assert_eq!(std::fs::read(&dest).unwrap(), body);

        let bad = format!("{}/web/Someone/zapper/releases/download/v0.3.0/bad", s.base);
        let e = r
            .download(&bad, &dest, Arc::new(|_| {}), Arc::new(AtomicBool::new(false)))
            .await
            .unwrap_err();
        assert!(e.contains("not GitHub's release storage"), "{e}");
        assert!(!dest.exists(), "a failed download leaves nothing behind");

        let elsewhere = format!("{}/web/Other/zapper/releases/download/v0.3.0/good", s.base);
        let e = r.fetch(&elsewhere, Arc::new(AtomicBool::new(false))).await.unwrap_err();
        assert!(e.contains("refusing"), "{e}");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A server that sends SHA256SUMS's headers and a few bytes, then
    /// nothing: the fetch gives up at its deadline instead of waiting on the
    /// files client's read timeout forever-ish.
    #[tokio::test]
    async fn a_stalled_sha256sums_fails_at_its_deadline() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                while reader.read_line(&mut line).is_ok() && line != "\r\n" && !line.is_empty() {
                    line.clear();
                }
                let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1000\r\n\r\nabc");
                let _ = stream.flush();
                std::thread::sleep(Duration::from_secs(5));
            }
        });
        let mut r = GitHubReleases::new("Someone/zapper", &base, &format!("{base}/web"), vec![]).unwrap();
        r.sums_timeout = Duration::from_millis(300);
        let url = format!("{base}/web/Someone/zapper/releases/download/v0.3.0/SHA256SUMS");
        let started = std::time::Instant::now();
        let e = r.fetch(&url, Arc::new(AtomicBool::new(false))).await.unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(3), "took {:?}", started.elapsed());
        assert!(e.contains("took longer") || e.contains("timed out") || e.contains("operation"), "{e}");
    }
}
