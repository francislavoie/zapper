// Settings > About > Open source licenses: the notices the production build puts in
// dist/licenses/ (tools/licenses.mjs), one at a time, picked from a list.
//
// Loaded on first use (AboutPane lazy-imports it), so none of it is in the startup bundle, and
// the notices themselves are fetched only when shown: the two generated lists are a few hundred
// kilobytes between them.

import { useEffect, useState } from "react";
import { isPageFallback, noticeLabel, orderNotices, type NoticeFile } from "../logic/licenses";

const url = (name: string) => new URL(`licenses/${name}`, document.baseURI).href;

export default function Licenses() {
  const [files, setFiles] = useState<NoticeFile[] | null | "missing">(null);
  const [current, setCurrent] = useState<string | null>(null);
  const [text, setText] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    fetch(url("index.json"))
      // The dev server, and a build made without the plugin, answer with the app's page instead:
      // its JSON parse fails, and that is the "not generated" case.
      .then((r) => (r.ok ? (r.json() as Promise<{ files: NoticeFile[] }>) : Promise.reject(new Error(String(r.status)))))
      .then(({ files }) => {
        if (!live) return;
        const sorted = orderNotices(files);
        setFiles(sorted.length ? sorted : "missing");
        setCurrent(sorted[0]?.name ?? null);
      })
      .catch(() => {
        if (live) setFiles("missing");
      });
    return () => {
      live = false;
    };
  }, []);

  useEffect(() => {
    if (!current) return;
    let live = true;
    setText(null);
    fetch(url(current))
      .then(async (r) => {
        const body = r.ok ? await r.text() : "";
        if (!r.ok || isPageFallback(r.headers.get("content-type"), body)) throw new Error(String(r.status));
        return body;
      })
      .then((t) => {
        if (live) setText(t);
      })
      .catch(() => {
        if (live) setText(`Couldn’t load ${current}.`);
      });
    return () => {
      live = false;
    };
  }, [current]);

  if (files === "missing") {
    return (
      <p className="licenses-missing">
        This build doesn’t include the license notices. The production build makes them
        (<code>npm run build</code>, which <code>just package</code> runs), with the Rust
        crates’ from <code>just licenses</code>.
      </p>
    );
  }
  if (!files) return <p className="licenses-missing" aria-busy="true">Loading…</p>;
  return (
    <div className="licenses">
      <ul className="licenses-list" aria-label="License files">
        {files.map((f) => {
          const { label, detail } = noticeLabel(f.name);
          return (
            <li key={f.name}>
              <button
                type="button"
                className={f.name === current ? "sel" : ""}
                aria-current={f.name === current}
                onClick={() => setCurrent(f.name)}
              >
                <span className="licenses-label" data-tip-overflow="">
                  {label}
                </span>
                {detail ? (
                  <span className="licenses-detail" data-tip-overflow="">
                    {detail}
                  </span>
                ) : null}
              </button>
            </li>
          );
        })}
      </ul>
      <pre
        className="licenses-text"
        tabIndex={0}
        aria-label={current ? noticeLabel(current).label : "License text"}
        aria-busy={text === null}
      >
        {text ?? ""}
      </pre>
    </div>
  );
}
