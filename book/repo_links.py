#!/usr/bin/env python3
"""mdbook preprocessor: fix the relative links of files included from outside `book/src`.

Most book pages are thin wrappers such as `{{#include ../../../docs/formats/czi.md}}`. mdbook pastes the
included text into the page unchanged, so a link written for GitHub (`[nd2](nd2.md)` in
`docs/formats/czi.md`, `[crate](../../crates/x)` in `docs/formats/x.md`) would resolve against the
book page's directory and break on the published site. This preprocessor runs before mdbook's
own `links` preprocessor, expands those includes itself, and rewrites every relative Markdown
link in the included text by what it points to in the repository:

- a file that some book page includes, or a page of the book itself → a relative link to that page (fragment kept);
- any other file or directory of the repository → its GitHub URL (`blob/main/…`, `tree/main/…`);
- anything else (a missing file, a same-page `#fragment`, an absolute URL) → left as written,
  so `book/check_links.py` reports what is really broken.

The docs therefore keep links that work on GitHub, and the book gets links that work on the
site. Only includes of `.md` files outside `book/src` are handled here (whole files or line
ranges such as `docs/x.md:5:14`); anchor includes and other files (`examples/…:2:13`) are left
to mdbook. Standard library only, Python 3.9+.
Wired up in `book/book.toml` (`[preprocessor.repo-links]`).
"""
from __future__ import annotations

import json
import os
import re
import sys
from pathlib import Path

BOOK_DIR = Path(__file__).resolve().parent
REPO = BOOK_DIR.parent
GITHUB = "https://github.com/openreadout/openreadout"

INCLUDE = re.compile(r"(?<!\\)\{\{\s*#include\s+([^\s}]+)\s*\}\}")
# An inline link or image target: `](target)` or `](target "title")`, target without spaces.
INLINE = re.compile(r"(\]\(\s*<?)([^()\s<>]+)(>?(?:\s+\"[^\"]*\")?\s*\))")
# A reference definition at the start of a line: `[id]: target`.
REFDEF = re.compile(r"^( {0,3}\[[^\]]+\]:\s*<?)(\S+?)(>?(?:\s.*)?)$")
SCHEME = re.compile(r"^[a-zA-Z][a-zA-Z0-9+.-]*:")
FENCE = re.compile(r"^ {0,3}(`{3,}|~{3,})")


LINES = re.compile(r"^(?P<file>[^:]+\.md)(?::(?P<a>\d*)(?::(?P<b>\d*))?)?$")


def split_include(target: str) -> tuple[str, slice] | None:
    """`file.md`, `file.md:2`, `file.md:2:`, `file.md::9`, `file.md:2:9` → (file, lines); else None."""
    m = LINES.match(target)
    if not m:
        return None  # not Markdown, or an anchor include: left to mdbook
    a, b = m.group("a"), m.group("b")
    start = int(a) - 1 if a else 0
    if b is None:  # `file.md` or `file.md:N` (only line N)
        end = int(a) if a else None
    else:
        end = int(b) if b else None
    return m.group("file"), slice(max(start, 0), end)


def included_file(src_dir: Path, chapter_path: str, target: str) -> Path | None:
    """The repository file a `.md` include outside book/src names, else None."""
    parsed = split_include(target)
    if parsed is None:
        return None
    path = Path(os.path.normpath((src_dir / chapter_path).parent / parsed[0]))
    try:
        path.relative_to(src_dir)
        return None  # a page of the book itself: mdbook handles it
    except ValueError:
        pass
    try:
        path.relative_to(REPO)
    except ValueError:
        return None
    return path if path.is_file() else None


def page_map(src_dir: Path) -> dict[Path, str]:
    """Repository file → the book page (path relative to src) that includes it (first wins)."""
    pages: dict[Path, str] = {}
    for md in sorted(src_dir.rglob("*.md")):
        rel = md.relative_to(src_dir).as_posix()
        for m in INCLUDE.finditer(md.read_text(encoding="utf-8")):
            path = included_file(src_dir, rel, m.group(1))
            if path is not None and ":" not in m.group(1):  # whole-file includes only
                pages.setdefault(path, rel)
    return pages


def rewrite_target(target: str, source: Path, chapter: str, pages: dict[Path, str]) -> str:
    if not target or target.startswith(("#", "/", "//")) or SCHEME.match(target):
        return target
    path_part, hash_, fragment = target.partition("#")
    path_part, _, query = path_part.partition("?")
    resolved = Path(os.path.normpath(source.parent / path_part))
    try:
        rel_repo = resolved.relative_to(REPO).as_posix()
    except ValueError:
        return target
    suffix = (hash_ + fragment) if hash_ else ""
    try:  # a page of the book itself (`../../book/src/guides/x.md` in a docs/ file)
        book_page = resolved.relative_to(BOOK_DIR / "src").as_posix()
    except ValueError:
        book_page = None
    if book_page is not None and resolved.is_file() and book_page.endswith(".md"):
        link = os.path.relpath(book_page, os.path.dirname(chapter) or ".").replace(os.sep, "/")
        return link + suffix
    if resolved in pages:
        page = pages[resolved]
        link = os.path.relpath(page, os.path.dirname(chapter) or ".").replace(os.sep, "/")
        return link + suffix
    if resolved.is_dir():
        return f"{GITHUB}/tree/main/{rel_repo}" + suffix
    if resolved.is_file():
        return f"{GITHUB}/blob/main/{rel_repo}" + ("?" + query if query else "") + suffix
    return target


def rewrite_text(text: str, source: Path, chapter: str, pages: dict[Path, str]) -> str:
    out = []
    fence: str | None = None
    for line in text.split("\n"):
        m = FENCE.match(line)
        if fence is not None:
            if m and m.group(1)[0] == fence[0] and len(m.group(1)) >= len(fence):
                fence = None
            out.append(line)
            continue
        if m:
            fence = m.group(1)
            out.append(line)
            continue

        def fix(match: re.Match) -> str:
            return match.group(1) + rewrite_target(match.group(2), source, chapter, pages) + match.group(3)

        ref = REFDEF.match(line)
        if ref:
            out.append(fix(ref))
            continue
        # Leave inline code spans alone: split on backtick runs, rewrite outside them only.
        parts = re.split(r"(`+)", line)
        result, i, in_code, ticks = [], 0, False, ""
        while i < len(parts):
            part = parts[i]
            if i % 2 == 1:  # a run of backticks
                if not in_code:
                    in_code, ticks = True, part
                elif part == ticks:
                    in_code = False
                result.append(part)
            else:
                result.append(part if in_code else INLINE.sub(fix, part))
            i += 1
        out.append("".join(result))
    return "\n".join(out)


def expand(content: str, chapter: str, src_dir: Path, pages: dict[Path, str]) -> str:
    def include(match: re.Match) -> str:
        path = included_file(src_dir, chapter, match.group(1))
        if path is None:
            return match.group(0)
        text = path.read_text(encoding="utf-8")
        lines = split_include(match.group(1))[1]
        if lines != slice(0, None):
            text = "\n".join(text.split("\n")[lines])
        return rewrite_text(text.rstrip("\n"), path, chapter, pages)

    return INCLUDE.sub(include, content)


def walk(items: list, src_dir: Path, pages: dict[Path, str]) -> None:
    for item in items:
        chapter = item.get("Chapter") if isinstance(item, dict) else None
        if not chapter:
            continue
        if chapter.get("path") and chapter.get("content"):
            chapter["content"] = expand(chapter["content"], chapter["path"], src_dir, pages)
        walk(chapter.get("sub_items", []), src_dir, pages)


def main() -> int:
    if len(sys.argv) > 1 and sys.argv[1] == "supports":
        return 0  # renderer-independent
    context, book = json.load(sys.stdin)
    root = Path(context["root"])
    src_dir = (root / context["config"]["book"].get("src", "src")).resolve()
    pages = page_map(src_dir)
    walk(book.get("items", book.get("sections", [])), src_dir, pages)
    json.dump(book, sys.stdout)
    return 0


if __name__ == "__main__":
    sys.exit(main())
