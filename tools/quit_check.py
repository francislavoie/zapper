#!/usr/bin/env python3
"""Opens and quits the app over and over, and checks that nothing crashes.

    tools/quit_check.py [--bin PATH] [--runs 30] [--quit-min 6] [--quit-max 10]

Each run starts zapper on a scratch account, plays a fixture from
tools/fake_stream.py, and after a few seconds closes the window the way its
close button does (`ZAPPER_QUIT_AFTER`). It then checks how zapper and every
process it started ended (exit status or signal), whether any of them was
still running long after zapper had gone, and whether the kernel logged a
segfault for any of them. It also checks that there was exactly one web
process and that zapper's log says WebKit ended it (`TerminatedByApi`; skip
that with `--baseline`, for a build that does not), and that what the page
wrote to local storage as it was closing is there afterwards:

- a nonce, which a `pagehide` handler stores (`ZAPPER_QUIT_NONCE`);
- the session's folded rail, changed 150 ms before the close, inside the
  session's 400 ms write delay, so only its `pagehide` flush can save it.

`--quit-by sigterm` sends SIGTERM instead of closing the window (and checks
no local storage).

Nothing appears on the desktop: it all runs in a headless GNOME Shell
(Wayland only, a virtual monitor) on a private session bus, with its own
HOME and XDG_RUNTIME_DIR. Every process has a core size limit of exactly one
byte, the value at which the kernel hands no core to a crash reporter: a
crash shows up here, in the results, and not as a desktop dialog or a file
in /var/crash.

The binary must load its interface by itself: a release build
(`npm run build`, then `cargo build --release --features tauri/custom-protocol`).
"""
import argparse, ctypes, datetime, glob, json, os, random, re, resource, signal, sqlite3
import subprocess, sys, tempfile, time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
FATAL = ("SIGSEGV", "SIGABRT", "SIGBUS", "SIGILL", "SIGFPE", "SIGTRAP", "SIGSYS")

ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
ap.add_argument("--bin", default=f"{ROOT}/src-tauri/target/release/zapper")
ap.add_argument("--runs", type=int, default=30)
ap.add_argument("--quit-min", type=float, default=6.0, help="earliest close, in seconds")
ap.add_argument("--quit-max", type=float, default=10.0, help="latest close, in seconds")
ap.add_argument("--port", type=int, default=8788, help="the fixture server's port")
ap.add_argument("--fixture", default="sample1080.ts")
ap.add_argument("--work", help="where logs go (default: a new temporary directory)")
ap.add_argument("--quit-by", choices=("close", "sigterm"), default="close")
ap.add_argument("--baseline", action="store_true",
                help="a build without the orderly quit: do not require TerminatedByApi")
args = ap.parse_args()

if os.environ.get("ZAPPER_QUIT_CHECK") != "1":
    # Start again in a clean environment and on a private session bus, so the
    # shell, the portals it starts and the app see nothing of the desktop's.
    work = os.path.abspath(args.work or tempfile.mkdtemp(prefix="zapper-quit-"))
    for d in ("home", "run"):
        os.makedirs(f"{work}/{d}", mode=0o700, exist_ok=True)
    os.chmod(f"{work}/run", 0o700)
    env = {"PATH": "/usr/local/bin:/usr/bin:/bin", "LANG": os.environ.get("LANG", "C.UTF-8"),
           "USER": os.environ.get("USER", ""), "LOGNAME": os.environ.get("LOGNAME", ""),
           "HOME": f"{work}/home", "XDG_RUNTIME_DIR": f"{work}/run", "ZAPPER_QUIT_CHECK": "1",
           # Resolved against the real HOME, before it is replaced.
           "ZAPPER_FIXTURES": os.environ.get("ZAPPER_FIXTURES")
           or os.path.expanduser("~/.cache/zapper/fixtures")}
    argv = [sys.executable, "-I", os.path.abspath(__file__), *sys.argv[1:], "--work", work]
    sys.exit(subprocess.call(["dbus-run-session", "--", *argv], env=env))

work = args.work
libc = ctypes.CDLL(None, use_errno=True)
# A child subreaper: once zapper has gone, the processes it started are
# reparented here, so how they end can still be waited for.
if libc.prctl(36, 1, 0, 0, 0) != 0:  # PR_SET_CHILD_SUBREAPER
    sys.exit("could not become a child subreaper")
resource.setrlimit(resource.RLIMIT_CORE, (1, 1))
env = dict(os.environ)
env.pop("ZAPPER_QUIT_CHECK")
WL = "wayland-quit-check"


def comm(pid):
    try:
        with open(f"/proc/{pid}/comm") as f:
            return f.read().strip()
    except OSError:
        return "?"


def ppid(pid):
    try:
        with open(f"/proc/{pid}/stat") as f:
            st = f.read()
        return int(st[st.rindex(")") + 2:].split()[1])
    except (OSError, ValueError):
        return None


def descendants(root):
    seen, todo = [], [root]
    while todo:
        p = todo.pop()
        try:
            for t in os.listdir(f"/proc/{p}/task"):
                with open(f"/proc/{p}/task/{t}/children") as f:
                    for c in map(int, f.read().split()):
                        if c not in seen:
                            seen.append(c)
                            todo.append(c)
        except OSError:
            pass
    return seen


def reap(statuses):
    while True:
        try:
            pid, st = os.waitpid(-1, os.WNOHANG)
        except ChildProcessError:
            return
        if pid == 0:
            return
        statuses[pid] = st


def streams(port):
    """Connections the fixture server is serving: a stream being played."""
    with open("/proc/net/tcp") as f:
        rows = [line.split() for line in f.read().splitlines()[1:]]
    return sum(int(r[1].split(":")[1], 16) == port and r[3] == "01" for r in rows)


def stored(root):
    """Every local-storage item WebKit wrote under `root`, as {key: value}."""
    items = {}
    for path in glob.glob(f"{root}/**/*", recursive=True):
        if not os.path.isfile(path) or "localstorage" not in path.lower():
            continue
        try:
            db = sqlite3.connect(f"file:{path}?mode=ro", uri=True)
            for key, value in db.execute("SELECT key, value FROM ItemTable"):
                if isinstance(value, bytes):
                    value = value.decode("utf-16-le", errors="replace")
                items[key] = value
            db.close()
        except sqlite3.Error:
            pass
    return items


def describe(st):
    if os.WIFSIGNALED(st):
        return f"signal {signal.Signals(os.WTERMSIG(st)).name}"
    return f"exit {os.WEXITSTATUS(st)}"


shell = subprocess.Popen(
    ["gnome-shell", "--headless", "--wayland", "--no-x11", "--virtual-monitor", "1920x1080",
     "--wayland-display", WL],
    env=env, stdout=open(f"{work}/shell.log", "w"), stderr=subprocess.STDOUT)
fixtures = subprocess.Popen(
    [sys.executable, "-I", f"{ROOT}/tools/fake_stream.py", "--port", str(args.port)],
    env=env, stdout=open(f"{work}/fixtures.log", "w"), stderr=subprocess.STDOUT)
for _ in range(100):
    if os.path.exists(f"{env['XDG_RUNTIME_DIR']}/{WL}"):
        break
    time.sleep(0.2)
else:
    shell.kill()
    fixtures.kill()
    sys.exit("the headless shell did not start")
time.sleep(2)

statuses, results = {}, []
try:
    for i in range(args.runs):
        zhome = f"{work}/run-{i}"
        os.makedirs(zhome, exist_ok=True)
        quit_after = round(random.uniform(args.quit_min, args.quit_max), 2)
        nonce = os.urandom(8).hex()
        # Its own XDG directories, so each run's local storage starts empty.
        xdg = f"{zhome}/xdg"
        zenv = dict(env, WAYLAND_DISPLAY=WL, GDK_BACKEND="wayland", ZAPPER_HARNESS="1",
                    ZAPPER_MUTE="1", ZAPPER_AUTOPLAY="1", ZAPPER_HOME=zhome,
                    ZAPPER_FIXTURE_BASE=f"http://127.0.0.1:{args.port}",
                    ZAPPER_FIXTURE_NAME=args.fixture, XDG_DATA_HOME=f"{xdg}/data",
                    XDG_CACHE_HOME=f"{xdg}/cache", XDG_CONFIG_HOME=f"{xdg}/config")
        if args.quit_by == "close":
            zenv.update(ZAPPER_QUIT_AFTER=str(quit_after), ZAPPER_QUIT_NONCE=nonce)
        t0 = time.time()
        since = time.strftime("%Y-%m-%d %H:%M:%S", time.localtime(t0 - 1))
        z = subprocess.Popen([args.bin], env=zenv, stdout=open(f"{zhome}/zapper.out", "w"),
                             stderr=subprocess.STDOUT)
        kids, seen, gone, parent, zst, played, signalled = {}, {}, {}, {}, None, None, False
        while time.time() - t0 < quit_after + 30:
            now = descendants(z.pid)
            for d in now:
                # Read again each time: a child forked to run another program
                # carries its parent's name until its exec.
                name = comm(d)
                if name != "?":
                    kids[d] = name
                if d not in seen:
                    seen[d] = round(time.time() - t0 - quit_after, 2)
                    parent[d] = ppid(d)
            for d in kids:
                if d not in now and d not in gone:
                    gone[d] = round(time.time() - t0 - quit_after, 2)
            # Just before the close: is the fixture being played?
            if played is None and time.time() - t0 > quit_after - 1:
                played = streams(args.port) > 0
            if args.quit_by == "sigterm" and not signalled and time.time() - t0 > quit_after:
                z.send_signal(signal.SIGTERM)
                signalled = True
            reap(statuses)
            if z.pid in statuses:
                zst = statuses.pop(z.pid)
                break
            time.sleep(0.05)
        hung = zst is None
        if hung:
            z.kill()
            while z.pid not in statuses:
                reap(statuses)
                time.sleep(0.05)
            zst = statuses.pop(z.pid)
        exited_at = time.time()

        def running(k):
            return k not in statuses and os.path.exists(f"/proc/{k}") and comm(k) == kids[k]

        deadline = time.time() + 15
        while time.time() < deadline and any(running(k) for k in kids):
            reap(statuses)
            time.sleep(0.05)
        reap(statuses)
        lingering = [k for k in kids if running(k)]
        for k in lingering:
            try:
                os.kill(k, signal.SIGKILL)
            except ProcessLookupError:
                pass
        time.sleep(0.3)
        reap(statuses)
        klog = subprocess.run(["journalctl", "-k", "--no-pager", "--since", since],
                              capture_output=True, text=True).stdout
        pids = [z.pid, *kids]
        kernel = [line for line in klog.splitlines()
                  if ("segfault" in line or "coredump" in line)
                  and any(f"[{p}]" in line or f" {p}(" in line for p in pids)]
        ended = {f"{kids[k]}[{k}]": "still running after 15 s" if k in lingering
                 else describe(statuses.pop(k)) if k in statuses
                 else "reaped by zapper" for k in kids}
        crashed = bool(kernel) or any(v.endswith(FATAL) for v in [describe(zst), *ended.values()])
        played = bool(played)
        with open(f"{zhome}/zapper.out", errors="replace") as f:
            log = f.read()
        # When each web process appeared and went, in seconds from the close.
        web_seen = [[seen[k], gone.get(k)] for k, c in kids.items()
                    if c.startswith("WebKitWebProc") and parent.get(k) == z.pid]
        web = len(web_seen)
        # From zapper logging that it is quitting to its process having exited.
        quit_secs = None
        for line in log.splitlines():
            if "quitting" in line:
                m = re.search(r"(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d+)Z", line)
                if m:
                    began = datetime.datetime.fromisoformat(m.group(1)).replace(
                        tzinfo=datetime.timezone.utc)
                    quit_secs = round(exited_at - began.timestamp(), 2)
                break
        by_api = "web process ended: TerminatedByApi" in log
        items = stored(xdg)
        nonce_kept = items.get("zapper.quit-check") == nonce
        try:
            session_kept = json.loads(items.get("zapper.session", "{}")).get("railCollapsed") is True
        except ValueError:
            session_kept = False
        clean = (not crashed and not hung and not lingering and describe(zst) == "exit 0"
                 and web == 1 and (by_api or args.baseline))
        r = dict(run=i, quit_after=quit_after, quit_secs=quit_secs, zapper=describe(zst),
                 hung=hung, processes=ended, kernel=kernel, played=played, web_processes=web,
                 web_seen=web_seen,
                 terminated_by_api=by_api, crashed=crashed, clean=clean)
        if args.quit_by == "close":
            r.update(nonce_kept=nonce_kept, session_kept=session_kept)
        results.append(r)
        print(json.dumps(r), flush=True)
finally:
    for p in (fixtures, shell):
        p.terminate()
    time.sleep(2)
    for p in (fixtures, shell):
        if p.poll() is None:
            p.kill()
    for k in descendants(os.getpid()):
        try:
            os.kill(k, signal.SIGKILL)
        except ProcessLookupError:
            pass
    reap(statuses)

with open(f"{work}/results.json", "w") as f:
    json.dump(results, f, indent=1)
crashed = sum(r["crashed"] for r in results)
unclean = sum(not r["clean"] for r in results)
played = sum(r["played"] for r in results)
kept = sum(r.get("nonce_kept", True) for r in results)
session = sum(r.get("session_kept", True) for r in results)
print(f"{len(results)} runs: {crashed} crashed, {unclean} not clean, {played} played, "
      f"nonce kept in {kept}, session change kept in {session} (logs in {work})")
sys.exit(1 if unclean or min(played, kept, session) < len(results) else 0)
