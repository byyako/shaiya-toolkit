"""MkDocs hooks for the documentation site.

Adds the repository's CHANGELOG.md, CONTRIBUTING.md and SECURITY.md as pages, rewrites Markdown links
so the same files read correctly on GitHub and on the site, and turns a screenshot that is not committed
yet into nothing, so the site builds with --strict before the images exist.
"""

import logging
import posixpath
import re
from pathlib import Path

from mkdocs.structure.files import File

log = logging.getLogger("mkdocs.hooks.docs-site")

BLOB = "https://github.com/byyako/shaiya-toolkit/blob/main/"
ROOT_PAGES = {"CHANGELOG.md": "changelog.md", "CONTRIBUTING.md": "contributing.md", "SECURITY.md": "security.md"}
SCREENSHOTS = "assets/screenshots/"
LINK = re.compile(r"(!?)\[([^\]]*)\]\(([^)\s]+)\)")
SCHEME = re.compile(r"^(?:[a-z][a-z0-9+.-]*:|#|/)", re.I)
# a fenced block (any indentation, ``` or ~~~, closed by the same fence or the end of the page) or an inline code span
SKIP = re.compile(
    r"^[ \t]*(?P<f>`{3,}|~{3,})[^\n]*\n.*?(?:^[ \t]*(?P=f)[`~]*[ \t]*$|\Z)|(?P<t>`+)(?!`).+?(?<!`)(?P=t)(?!`)",
    re.M | re.S,
)


def _repo_root(config):
    return Path(config["config_file_path"]).parent


def on_files(files, config):
    root = _repo_root(config)
    for src, dest in ROOT_PAGES.items():
        files.append(File.generated(config, dest, abs_src_path=str(root / src)))
    return files


def _repo_path(src_uri):
    for repo, site in ROOT_PAGES.items():
        if src_uri == site:
            return repo
    return "docs/" + src_uri


def on_page_markdown(markdown, page, config, files):
    src = page.file.src_uri
    here = posixpath.dirname(_repo_path(src))
    page_dir = posixpath.dirname(src)
    docs_dir = Path(config["docs_dir"])

    def fix(m):
        bang, text, url = m.groups()
        if SCHEME.match(url):
            return m.group(0)
        path, sep, frag = url.partition("#")
        target = posixpath.normpath(posixpath.join(here, path))
        if target.startswith("docs/"):
            dest = target[len("docs/"):]
        elif target in ROOT_PAGES:
            dest = ROOT_PAGES[target]
        else:
            return f"{bang}[{text}]({BLOB}{target}{sep}{frag})"
        if bang and dest.startswith(SCREENSHOTS) and not (docs_dir / dest).is_file():
            log.info("screenshot not committed yet, left out: docs/%s (%s)", dest, src)
            return ""
        rel = posixpath.relpath(dest, page_dir or ".")
        return f"{bang}[{text}]({rel}{sep}{frag})"

    # leave fenced code blocks and inline code spans alone
    out, last = [], 0
    for f in SKIP.finditer(markdown):
        out.append(LINK.sub(fix, markdown[last:f.start()]))
        out.append(f.group(0))
        last = f.end()
    out.append(LINK.sub(fix, markdown[last:]))
    return "".join(out)
