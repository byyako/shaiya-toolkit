"""Offline check of the built documentation site (no network).

1. Every relative link and asset in the HTML points at a file that exists, and every #fragment at an id in it.
2. No page loads anything from another host: no external <script>, <link> (except rel=canonical),
   <img>, <source>, <iframe>; no http(s) URL in inline scripts or styles, or in CSS; JavaScript may
   only contain the known lazy URLs of the theme, which this configuration never triggers.
Links to other sites (<a href="https://...">) are listed, not fetched.

Usage: python .github/docs-site/check_site.py [site-dir] [--base /shaiya-toolkit/]
"""

import re
import sys
from html.parser import HTMLParser
from pathlib import Path
from urllib.parse import unquote, urlsplit

# Strings in the theme's JavaScript bundle. They are only used when repo_url is set (GitHub API),
# a mermaid diagram is on the page, or the browser lacks ResizeObserver. None applies here.
JS_ALLOWED = ("https://api.github.com/repos/", "https://api.github.com/users/", "https://clipboardjs.com/",
              "https://unpkg.com/mermaid", "https://unpkg.com/resize-observer-polyfill",
              "https://bit.ly/2s3ChXG")  # a help link in a lunr error message of the search worker
EXTERNAL = re.compile(r"(?:https?:)?//[a-z0-9.-]+\.[a-z]{2,}", re.I)
ASSET_TAGS = {"script": "src", "img": "src", "source": "src", "iframe": "src", "link": "href",
              "audio": "src", "video": "src", "embed": "src", "object": "data"}


class Page(HTMLParser):
    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.ids, self.links, self.assets, self.inline = set(), [], [], []
        self._raw = None

    def handle_starttag(self, tag, attrs):
        a = dict(attrs)
        for k in ("id", "name"):
            if a.get(k):
                self.ids.add(a[k])
        if tag == "a" and a.get("href"):
            self.links.append(a["href"])
        if tag in ASSET_TAGS and a.get(ASSET_TAGS[tag]):
            if not (tag == "link" and a.get("rel", "").lower() in ("canonical",)):
                self.assets.append((tag, a[ASSET_TAGS[tag]]))
        if a.get("srcset"):
            self.assets += [(tag, s.strip().split(" ")[0]) for s in a["srcset"].split(",")]
        if tag in ("script", "style"):
            self._raw = []

    def handle_data(self, data):
        if self._raw is not None:
            self._raw.append(data)

    def handle_endtag(self, tag):
        if tag in ("script", "style") and self._raw is not None:
            self.inline.append("".join(self._raw))
            self._raw = None


def main():
    args = sys.argv[1:]
    base = "/shaiya-toolkit/"
    if "--base" in args:
        i = args.index("--base")
        base = args[i + 1]
        del args[i:i + 2]
    site = Path(args[0] if args else "site").resolve()
    if not (site / "index.html").is_file():
        sys.exit(f"{site}: no index.html, build the site first")
    pages, errors, outside = {}, [], set()
    for f in sorted(site.rglob("*.html")):
        p = Page()
        p.feed(f.read_text(encoding="utf-8"))
        pages[f] = p

    def resolve(f, url):
        parts = urlsplit(url)
        path = unquote(parts.path)
        if path.startswith("/"):
            if not path.startswith(base):
                return None, parts.fragment
            t = site / path[len(base):]
        elif path:
            t = f.parent / path
        else:
            t = f
        t = t.resolve()
        if t.is_dir():
            t = t / "index.html"
        return t, parts.fragment

    for f, p in pages.items():
        rel = f.relative_to(site).as_posix()
        for tag, url in p.assets:
            if EXTERNAL.match(url):
                errors.append(f"{rel}: <{tag}> loads {url} from another host")
        for text in p.inline:
            for m in EXTERNAL.finditer(text):
                errors.append(f"{rel}: inline script or style mentions {m.group(0)}")
        for url in p.links + [u for _, u in p.assets]:
            if EXTERNAL.match(url) or re.match(r"^[a-z][a-z0-9+.-]*:", url, re.I):
                if url.startswith(("http://", "https://")):
                    outside.add(url)
                continue
            target, frag = resolve(f, url)
            if target is None or not target.is_file():
                errors.append(f"{rel}: broken link {url}")
            elif frag and target.suffix == ".html":
                tp = pages.get(target)
                if tp is not None and frag not in tp.ids:
                    errors.append(f"{rel}: {url}: no id {frag!r} in {target.relative_to(site).as_posix()}")
    for f in sorted(site.rglob("*.css")):
        for m in re.finditer(r"@import|url\(\s*['\"]?((?:https?:)?//[^)'\"]+)", f.read_text(encoding="utf-8")):
            errors.append(f"{f.relative_to(site).as_posix()}: {m.group(0)}")
    for f in sorted(site.rglob("*.js")):
        if "lunr" in f.relative_to(site).parts:
            continue  # stemmers and tokenizers for other languages: licence and doc URLs only, never fetched
        code = re.sub(r"/\*.*?\*/", "", f.read_text(encoding="utf-8"), flags=re.S)  # licence comments
        for m in re.finditer(r"https?://[A-Za-z0-9.-]+[^\s\"'`)]*", code):
            if not m.group(0).startswith(JS_ALLOWED) and "w3.org" not in m.group(0):
                errors.append(f"{f.relative_to(site).as_posix()}: unexpected URL {m.group(0)}")
    print(f"check_site: {len(pages)} pages, {len(outside)} external links (not fetched)")
    for u in sorted(outside):
        print(f"  external: {u}")
    if errors:
        print(f"check_site: {len(errors)} problems", file=sys.stderr)
        for e in errors:
            print(f"  {e}", file=sys.stderr)
        sys.exit(1)
    print("check_site: ok")


if __name__ == "__main__":
    main()
