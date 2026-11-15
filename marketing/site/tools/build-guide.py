"""Generate the on-site user guide from docs/USER_GUIDE.md.

    python tools/build-guide.py          (no dependencies beyond Python 3.8+ and git)

Writes
  guide/index.html        English part + shared sections (English paragraphs where they alternate)
  zh-tw/guide/index.html  Chinese part + shared sections (Chinese paragraphs where they alternate)

How the source is split
  - "## 繁體中文" is the Chinese part, "## English" the English part; every other "##" section and the
    lines before the first "##" are shared.
  - In shared sections a block whose text is "中文 / English" (one " / " between a Chinese side and a
    Latin-only side, top level or inside a link text) shows only its own side. A paragraph containing
    Chinese is the Chinese half of a pair; the Latin-only paragraph right after it is the English half.
    When the Chinese half starts with a bold Latin label ("**REAPER**。") the English half gets the
    same label, so the English list item still says which DAW it is about.
  - Heading ids follow GitHub (lower case, punctuation removed, spaces to "-", "-1", "-2" for repeats),
    so anchors copied from GitHub keep working. Anchors into the other language's part go to the other
    guide page; relative repository links go to https://github.com/kekekewww/Backtracking/blob/main/...

The output depends only on the Markdown file and its last commit (git log -1 -- docs/USER_GUIDE.md),
so running the script twice gives identical files. Re-run tools/subset-zh.py afterwards when the
Chinese headings change (the zh page sets headings in the subset font).
"""
import html
import os
import posixpath
import re
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
SITE = os.path.dirname(HERE)
REPO = os.path.dirname(os.path.dirname(SITE))
SRC_REL = "docs/USER_GUIDE.md"
SRC = os.path.join(REPO, *SRC_REL.split("/"))
GH = "https://github.com/kekekewww/Backtracking"
BLOB = GH + "/blob/main/"
ORIGIN = "https://backtracking.kamelkyp.com"
SUPPORT = "https://buymeacoffee.com/kamelkyp/e/596694"

PAGES = {
    "en": {
        "out": os.path.join(SITE, "guide", "index.html"), "root": "../", "url": "/guide/",
        "other": "../zh-tw/guide/", "home": "../", "part": "English", "html_lang": "en",
        "title": "User guide · Backtracking",
        "desc": "How to capture, select and export audio with Backtracking on Windows and macOS and in the VST3 / CLAP plug-in.",
        "toc": "Contents", "back": "Back to the home page", "download": "Download from GitHub",
        "support": "Support developer", "langnav": "Language", "copy": "Copy", "copied": "Copied",
        "copyfail": "Select the text to copy", "table": "Table, scrolls sideways",
        "gen": 'Generated from <code>docs/USER_GUIDE.md</code>', "view": "View on GitHub",
        "view_frag": "#english", "license": "Apache-2.0 licence", "repo": "GitHub repository",
        "fine": "No cookies, no analytics.", "code": "Code",
    },
    "zh": {
        "out": os.path.join(SITE, "zh-tw", "guide", "index.html"), "root": "../../", "url": "/zh-tw/guide/",
        "other": "../../guide/", "home": "../", "part": "繁體中文", "html_lang": "zh-Hant-TW",
        "title": "操作手冊 · Backtracking",
        "desc": "Backtracking 的擷取、框選與匯出方式，涵蓋 Windows、macOS 桌面版與 VST3／CLAP 插件。",
        "toc": "目錄", "back": "返回首頁", "download": "從 GitHub 下載",
        "support": "贊助開發者", "langnav": "語言", "copy": "複製", "copied": "已複製",
        "copyfail": "請選取文字後複製", "table": "表格，可左右捲動",
        "gen": '由 <code>docs/USER_GUIDE.md</code> 產生', "view": "在 GitHub 上檢視",
        "view_frag": "#繁體中文", "license": "Apache-2.0 授權", "repo": "GitHub 專案",
        "fine": "不使用 Cookie，也不做流量分析。", "code": "程式碼",
    },
}

CJK = re.compile(r"[⺀-鿿豈-﫿︰-﹏＀-￯]")


def has_cjk(s):
    return bool(CJK.search(s))


# ---------------------------------------------------------------- block parser

def parse_blocks(lines):
    """Markdown subset: ATX headings, fenced code, pipe tables, lists (nested, loose), blockquotes,
    horizontal rules, paragraphs."""
    blocks = []
    i = 0
    n = len(lines)
    while i < n:
        line = lines[i]
        if not line.strip():
            i += 1
            continue
        m = re.match(r"^(#{1,6})\s+(.*?)\s*#*\s*$", line)
        if m:
            blocks.append({"t": "h", "level": len(m.group(1)), "text": m.group(2)})
            i += 1
            continue
        m = re.match(r"^(\s*)(`{3,}|~{3,})\s*([\w+-]*)", line)
        if m:
            fence, indent = m.group(2), len(m.group(1))
            body = []
            i += 1
            while i < n and not lines[i].strip().startswith(fence):
                body.append(lines[i][indent:] if lines[i][:indent].strip() == "" else lines[i])
                i += 1
            i += 1
            blocks.append({"t": "code", "lang": m.group(3), "text": "\n".join(body)})
            continue
        if re.match(r"^\s*([-*_])(\s*\1){2,}\s*$", line):
            blocks.append({"t": "hr"})
            i += 1
            continue
        if line.lstrip().startswith(">"):
            body = []
            while i < n and lines[i].strip() and lines[i].lstrip().startswith(">"):
                body.append(re.sub(r"^\s*> ?", "", lines[i]))
                i += 1
            blocks.append({"t": "quote", "blocks": parse_blocks(body)})
            continue
        if "|" in line and i + 1 < n and re.match(r"^\s*\|?\s*:?-{3,}:?\s*(\|\s*:?-{3,}:?\s*)*\|?\s*$", lines[i + 1]):
            head = split_row(line)
            aligns = []
            for c in split_row(lines[i + 1]):
                c = c.strip()
                aligns.append("center" if c.startswith(":") and c.endswith(":") else
                              "right" if c.endswith(":") else "left" if c.startswith(":") else "")
            rows = []
            i += 2
            while i < n and lines[i].strip() and "|" in lines[i]:
                rows.append(split_row(lines[i]))
                i += 1
            blocks.append({"t": "table", "head": head, "aligns": aligns, "rows": rows})
            continue
        m = re.match(r"^(\s*)([-*+]|\d{1,9}[.)])(\s+)(.*)$", line)
        if m:
            blk, i = parse_list(lines, i)
            blocks.append(blk)
            continue
        body = [line.strip()]
        i += 1
        while i < n and lines[i].strip() and not starts_block(lines[i]):
            body.append(lines[i].strip())
            i += 1
        blocks.append({"t": "p", "text": " ".join(body)})
    return blocks


def starts_block(line):
    return bool(re.match(r"^(#{1,6}\s|\s*(`{3,}|~{3,})|\s*>|\s*([-*+]|\d{1,9}[.)])\s+)", line))


def parse_list(lines, i):
    first = re.match(r"^(\s*)([-*+]|\d{1,9}[.)])(\s+)", lines[i])
    base = len(first.group(1))
    ordered = first.group(2)[0].isdigit()
    start = int(first.group(2)[:-1]) if ordered else 1
    items = []
    loose = False
    n = len(lines)
    while i < n:
        m = re.match(r"^(\s*)([-*+]|\d{1,9}[.)])(\s+)(.*)$", lines[i])
        if not m or len(m.group(1)) != base or m.group(2)[0].isdigit() != ordered:
            break
        width = len(m.group(1)) + len(m.group(2)) + len(m.group(3))
        body = [m.group(4)]
        i += 1
        while i < n:
            line = lines[i]
            if not line.strip():
                j = i
                while j < n and not lines[j].strip():
                    j += 1
                if j < n and len(lines[j]) - len(lines[j].lstrip()) >= width:
                    body.extend([""] * (j - i))
                    i = j
                    loose = True
                    continue
                break
            indent = len(line) - len(line.lstrip())
            if indent >= width:
                body.append(line[width:])
            elif re.match(r"^\s*([-*+]|\d{1,9}[.)])\s+", line):
                break
            else:
                body.append(line.strip())  # lazy continuation
            i += 1
        items.append(parse_blocks(body))
        # a blank line between items makes the list loose
        if i < n and not lines[i].strip():
            j = i
            while j < n and not lines[j].strip():
                j += 1
            nxt = re.match(r"^(\s*)([-*+]|\d{1,9}[.)])\s+", lines[j]) if j < n else None
            if nxt and len(nxt.group(1)) == base:
                loose = True
                i = j
    for it in items:
        if len(it) > 1:
            loose = True
    return {"t": "list", "ordered": ordered, "start": start, "items": items, "loose": loose}, i


def split_row(line):
    s = line.strip()
    if s.startswith("|"):
        s = s[1:]
    if s.endswith("|") and not s.endswith("\\|"):
        s = s[:-1]
    cells, cur, tick = [], "", False
    k = 0
    while k < len(s):
        ch = s[k]
        if ch == "\\" and k + 1 < len(s) and s[k + 1] == "|":
            cur += "|"
            k += 2
            continue
        if ch == "`":
            tick = not tick
        if ch == "|" and not tick:
            cells.append(cur.strip())
            cur = ""
        else:
            cur += ch
        k += 1
    cells.append(cur.strip())
    return cells


# ---------------------------------------------------------------- headings / ids

def plain(text):
    """Text of a heading as GitHub renders it (markup removed)."""
    t = re.sub(r"`([^`]*)`", r"\1", text)
    t = re.sub(r"!?\[([^\]]*)\]\([^)]*\)", r"\1", t)
    t = re.sub(r"(\*\*|__)(.+?)\1", r"\2", t)
    t = re.sub(r"(?<![\w*])[*_](.+?)[*_](?![\w*])", r"\1", t)
    return t


def slug(text, seen):
    s = plain(text).strip().lower()
    s = re.sub(r"[^\w\- ]", "", s)
    s = s.replace(" ", "-")
    base, k = s, 0
    while s in seen:
        k += 1
        s = "%s-%d" % (base, k)
    seen.add(s)
    return s


def assign_ids(blocks, seen):
    for b in blocks:
        if b["t"] == "h":
            b["id"] = slug(b["text"], seen)
        elif b["t"] == "list":
            for it in b["items"]:
                assign_ids(it, seen)
        elif b["t"] == "quote":
            assign_ids(b["blocks"], seen)


def collect_ids(blocks, out):
    for b in blocks:
        if b["t"] == "h":
            out.add(b["id"])
        elif b["t"] == "list":
            for it in b["items"]:
                collect_ids(it, out)
        elif b["t"] == "quote":
            collect_ids(b["blocks"], out)
    return out


# ---------------------------------------------------------------- language selection

def top_level_seps(s):
    """Positions of ' / ' outside links, link targets and code spans."""
    seps, depth, paren, tick = [], 0, 0, False
    for k, ch in enumerate(s):
        if ch == "`":
            tick = not tick
        elif tick:
            continue
        elif ch == "[":
            depth += 1
        elif ch == "]":
            depth = max(0, depth - 1)
        elif ch == "(" and k > 0 and s[k - 1] == "]":
            paren += 1
        elif ch == ")" and paren:
            paren -= 1
        elif s.startswith(" / ", k) and depth == 0 and paren == 0:
            seps.append(k)
    return seps


def pick_side(s, lang):
    seps = top_level_seps(s)
    if len(seps) != 1:
        return None
    left, right = s[:seps[0]], s[seps[0] + 3:]
    if not has_cjk(left) or has_cjk(right):
        return None
    if lang == "zh":
        return left.strip()
    # "Backtracking 操作手冊 / User Guide": keep a shared Latin lead-in on the English side
    lead = re.match(r"^([A-Za-z0-9][\w.+-]*(?: [A-Za-z0-9][\w.+-]*)*) ", left)
    if lead and not right.startswith(lead.group(1)) and has_cjk(left[lead.end():]):
        return lead.group(1) + " " + right.strip()
    return right.strip()


def pick(s, lang):
    """Return (text, True) when s is a '中文 / English' pair (whole text or link texts)."""
    whole = pick_side(s, lang)
    if whole is not None:
        return whole, True
    changed = [False]

    def link(m):
        side = pick_side(m.group(1), lang)
        if side is None:
            return m.group(0)
        changed[0] = True
        return "[%s](%s)" % (side, m.group(2))
    out = re.sub(r"\[([^\]]+)\]\(([^)\s]+)\)", link, s)
    return out, changed[0]


LABEL = re.compile(r"^(\*\*[^*⺀-鿿]+\*\*)[。．.]\s*")


def localize(blocks, lang):
    out, prev_zh = [], None
    for b in blocks:
        t = b["t"]
        if t in ("p", "h"):
            text, split = pick(b["text"], lang)
            if split:
                out.append(dict(b, text=text))
                prev_zh = None
                continue
            lone_bold = re.match(r"^\*\*[^*]+\*\*$", b["text"])
            if t == "p" and has_cjk(b["text"]) and not lone_bold:
                if lang == "zh":
                    out.append(b)
                prev_zh = b
                continue
            if t == "p" and prev_zh is not None and not lone_bold:
                if lang == "en":
                    text = b["text"]
                    lab = LABEL.match(prev_zh["text"])
                    if lab and not text.startswith("**"):
                        text = lab.group(1) + ". " + text
                    out.append(dict(b, text=text))
                prev_zh = None
                continue
            out.append(b)
            prev_zh = None
        elif t == "list":
            items = [localize(it, lang) for it in b["items"]]
            # a list is loose only because of its "中文 + English" pairs: after picking one language
            # every item is a single paragraph again, so render it tight
            loose = b["loose"] and any(len(it) > 1 for it in items)
            out.append(dict(b, items=items, loose=loose))
            prev_zh = None
        elif t == "quote":
            out.append(dict(b, blocks=localize(b["blocks"], lang)))
            prev_zh = None
        elif t == "table":
            out.append(dict(b, head=[pick(c, lang)[0] for c in b["head"]],
                            rows=[[pick(c, lang)[0] for c in r] for r in b["rows"]]))
            prev_zh = None
        else:
            out.append(b)
            prev_zh = None
    return out


# ---------------------------------------------------------------- inline rendering

class Ctx:
    def __init__(self, lang, here_ids, other_ids, other_page):
        self.lang, self.here, self.other, self.other_page = lang, here_ids, other_ids, other_page
        self.problems = []

    def href(self, url):
        if url.startswith("#"):
            frag = url[1:]
            if frag in self.here:
                return url
            if frag in self.other:
                return self.other_page + url
            self.problems.append("anchor not found: " + url)
            return url
        if re.match(r"^[a-z][a-z0-9+.-]*:", url, re.I):
            return url
        path, _, frag = url.partition("#")
        target = posixpath.normpath(posixpath.join(posixpath.dirname(SRC_REL), path))
        if target == SRC_REL and frag:
            return self.href("#" + frag)
        if target.startswith(".."):
            self.problems.append("link outside the repository: " + url)
        elif not os.path.exists(os.path.join(REPO, *target.split("/"))):
            self.problems.append("link target missing in the repository: " + url)
        return BLOB + target + ("#" + frag if frag else "")


def inline(s, ctx):
    out = []
    k = 0
    n = len(s)
    while k < n:
        ch = s[k]
        if ch == "\\" and k + 1 < n and s[k + 1] in "\\`*_{}[]()#+-.!|<>~":
            out.append(html.escape(s[k + 1]))
            k += 2
            continue
        if ch == "`":
            run = re.match(r"`+", s[k:]).group(0)
            end = s.find(run, k + len(run))
            if end != -1:
                code = s[k + len(run):end]
                if code.startswith(" ") and code.endswith(" ") and code.strip():
                    code = code[1:-1]
                out.append("<code>%s</code>" % html.escape(code, quote=False))
                k = end + len(run)
                continue
        if ch == "[":
            m = match_link(s, k)
            if m:
                text, url, end = m
                out.append('<a href="%s">%s</a>' % (html.escape(ctx.href(url)), inline(text, ctx)))
                k = end
                continue
        if s.startswith("**", k) or s.startswith("__", k):
            mark = s[k:k + 2]
            end = s.find(mark, k + 2)
            if end > k + 2:
                out.append("<strong>%s</strong>" % inline(s[k + 2:end], ctx))
                k = end + 2
                continue
        if ch in "*_" and k + 1 < n and s[k + 1] not in " *_" and (ch == "*" or k == 0 or not s[k - 1].isalnum()):
            end = s.find(ch, k + 1)
            if end > k + 1 and s[end - 1] != " " and (ch == "*" or end + 1 >= n or not s[end + 1].isalnum()):
                out.append("<em>%s</em>" % inline(s[k + 1:end], ctx))
                k = end + 1
                continue
        if s.startswith("<http", k):
            end = s.find(">", k)
            if end != -1:
                url = s[k + 1:end]
                out.append('<a href="%s">%s</a>' % (html.escape(url), html.escape(url)))
                k = end + 1
                continue
        out.append(html.escape(ch, quote=False))
        k += 1
    return "".join(out)


def match_link(s, k):
    depth, j, tick = 0, k, False
    while j < len(s):
        c = s[j]
        if c == "`":
            tick = not tick
        elif not tick and c == "[":
            depth += 1
        elif not tick and c == "]":
            depth -= 1
            if depth == 0:
                break
        j += 1
    if j >= len(s) or not s.startswith("(", j + 1):
        return None
    depth, e = 0, j + 1
    while e < len(s):
        if s[e] == "(":
            depth += 1
        elif s[e] == ")":
            depth -= 1
            if depth == 0:
                break
        e += 1
    if e >= len(s):
        return None
    url = s[j + 2:e].strip().split(" ")[0].strip("<>")
    return s[k + 1:j], url, e + 1


# ---------------------------------------------------------------- block rendering

def render(blocks, ctx, tight=False):
    out = []
    for b in blocks:
        t = b["t"]
        if t == "h":
            lvl = min(6, b["level"])
            out.append('<h%d id="%s">%s</h%d>' % (lvl, html.escape(b["id"]), inline(b["text"], ctx), lvl))
        elif t == "p":
            out.append(inline(b["text"], ctx) if tight else "<p>%s</p>" % inline(b["text"], ctx))
        elif t == "code":
            cls = ' class="language-%s"' % html.escape(b["lang"]) if b["lang"] else ""
            label = html.escape(b["lang"] or PAGES[ctx.lang]["code"])
            out.append('<div class="code"><div class="code-bar"><span class="code-lang">%s</span></div>'
                       '<pre><code%s>%s</code></pre></div>' % (label, cls, html.escape(b["text"], quote=False)))
        elif t == "hr":
            out.append("<hr>")
        elif t == "quote":
            out.append("<blockquote>\n%s\n</blockquote>" % render(b["blocks"], ctx))
        elif t == "table":
            def cell(tag, text, a):
                style = ' style="text-align:%s"' % a if a else ""
                return "<%s%s>%s</%s>" % (tag, style, inline(text, ctx), tag)
            aligns = b["aligns"] + [""] * 20
            head = "".join(cell("th", c, aligns[k]) for k, c in enumerate(b["head"]))
            rows = "\n".join("<tr>%s</tr>" % "".join(cell("td", c, aligns[k]) for k, c in enumerate(r)) for r in b["rows"])
            cols = len(b["head"])
            # .table-box holds the right-edge fade (js/guide.js shows it while more columns are off-screen)
            out.append('<div class="table-box"><div class="table-wrap cols-%d" role="region" tabindex="0" aria-label="%s">'
                       '<table>\n<thead><tr>%s</tr></thead>\n<tbody>\n%s\n</tbody>\n</table></div></div>'
                       % (cols, html.escape(PAGES[ctx.lang]["table"]), head, rows))
        elif t == "list":
            tag = "ol" if b["ordered"] else "ul"
            start = ' start="%d"' % b["start"] if b["ordered"] and b["start"] != 1 else ""
            items = []
            for it in b["items"]:
                items.append("<li>%s</li>" % render(it, ctx, tight=not b["loose"]))
            out.append("<%s%s>\n%s\n</%s>" % (tag, start, "\n".join(items), tag))
    return "\n".join(out)


# ---------------------------------------------------------------- page

def split_parts(blocks):
    pre, parts, cur = [], [], None
    for b in blocks:
        if b["t"] == "h" and b["level"] == 2:
            cur = {"head": b, "blocks": []}
            parts.append(cur)
        elif cur is None:
            pre.append(b)
        else:
            cur["blocks"].append(b)
    return pre, parts


def page_blocks(pre, parts, lang):
    """Return (h1 block, preamble blocks, sections) for one language. Each section is
    (heading or None, blocks, wrapper id)."""
    own = PAGES[lang]["part"]
    langs = {PAGES["en"]["part"], PAGES["zh"]["part"]}
    h1 = next((b for b in pre if b["t"] == "h" and b["level"] == 1), None)
    rest = [b for b in pre if b is not h1]
    h1 = localize([h1], lang)[0] if h1 else None
    sections = []
    for p in parts:
        title = plain(p["head"]["text"]).strip()
        if title == own:
            sections.append((None, p["blocks"], p["head"]["id"]))
        elif title in langs:
            continue
        else:
            head = localize([p["head"]], lang)[0]
            sections.append((head, localize(p["blocks"], lang), None))
    return h1, localize(rest, lang), sections


def toc_html(sections, ctx):
    items = []
    for head, blocks, _ in sections:
        subs = [b for b in blocks if b["t"] == "h" and b["level"] == 3]
        if head is None:
            items.extend('<li><a href="#%s">%s</a></li>' % (html.escape(b["id"]), inline(b["text"], ctx)) for b in subs)
        else:
            inner = "".join('<li><a href="#%s">%s</a></li>' % (html.escape(b["id"]), inline(b["text"], ctx)) for b in subs)
            items.append('<li class="toc-group"><a href="#%s">%s</a>%s</li>'
                         % (html.escape(head["id"]), inline(head["text"], ctx), "<ol>%s</ol>" % inner if inner else ""))
    return "<ol>\n%s\n</ol>" % "\n".join(items)


DL_ICON = ('<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M8 2v8M4.5 6.8 8 10.3l3.5-3.5M2.5 13.5h11" '
           'fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="square"/></svg>')


def build_page(lang, h1, pre, sections, commit, ctx):
    P = PAGES[lang]
    root = P["root"]
    body_sections = []
    for head, blocks, wrap_id in sections:
        inner = render(blocks, ctx)
        if head is None:
            body_sections.append('<section class="part" id="%s">\n%s\n</section>' % (html.escape(wrap_id), inner))
        else:
            body_sections.append('<section class="part shared">\n%s\n%s\n</section>' % (render([head], ctx), inner))
    h, date = commit
    title_text = inline(h1["text"], ctx) if h1 else "Backtracking"
    lang_nav = (
        '<span aria-current="page">EN</span><a class="sys-cjk" href="%s" hreflang="zh-Hant-TW" lang="zh-Hant-TW">中文</a>'
        % P["other"] if lang == "en" else
        '<a href="%s" hreflang="en" lang="en">EN</a><span aria-current="page">中文</span>' % P["other"])
    parts = [
        "<!doctype html>",
        '<html lang="%s">' % P["html_lang"],
        "<head>",
        '<meta charset="utf-8">',
        '<meta name="viewport" content="width=device-width, initial-scale=1">',
        "<title>%s</title>" % html.escape(P["title"]),
        '<meta name="description" content="%s">' % html.escape(P["desc"]),
        '<meta name="theme-color" content="#1e32d2">',
        '<link rel="canonical" href="%s%s">' % (ORIGIN, P["url"]),
        '<link rel="alternate" hreflang="en" href="%s%s">' % (ORIGIN, PAGES["en"]["url"]),
        '<link rel="alternate" hreflang="zh-Hant-TW" href="%s%s">' % (ORIGIN, PAGES["zh"]["url"]),
        '<link rel="alternate" hreflang="x-default" href="%s%s">' % (ORIGIN, PAGES["en"]["url"]),
        '<meta property="og:type" content="article">',
        '<meta property="og:site_name" content="Backtracking">',
        '<meta property="og:title" content="%s">' % html.escape(P["title"]),
        '<meta property="og:description" content="%s">' % html.escape(P["desc"]),
        '<meta property="og:url" content="%s%s">' % (ORIGIN, P["url"]),
        '<meta property="og:image" content="%s/img/og-%s.png">' % (ORIGIN, "en" if lang == "en" else "zh-TW"),
        '<meta name="twitter:card" content="summary_large_image">',
        '<link rel="icon" href="%simg/icon.svg" type="image/svg+xml">' % root,
        '<link rel="apple-touch-icon" href="%simg/apple-touch-icon.png">' % root,
        '<link rel="preload" href="%sfonts/Archivo-Variable-latin.woff2" as="font" type="font/woff2" crossorigin>' % root,
        '<link rel="stylesheet" href="%scss/site.css">' % root,
        '<link rel="stylesheet" href="%scss/guide.css">' % root,
        '<script src="%sjs/guide.js" defer></script>' % root,
        "</head>",
        '<body class="guide" data-copy="%s" data-copied="%s" data-copyfail="%s">'
        % (html.escape(P["copy"]), html.escape(P["copied"]), html.escape(P["copyfail"])),
        "<!-- Generated by tools/build-guide.py from %s; edit the Markdown, not this file. -->" % SRC_REL,
        '<header class="top wrap">',
        '  <a class="lockup" href="%s"><img src="%simg/icon.svg" width="44" height="44" alt=""><span class="wm">Backtracking</span></a>' % (P["home"], root),
        '  <nav class="lang wide" aria-label="%s">%s</nav>' % (P["langnav"], lang_nav),
        "</header>",
        '<div class="g-head wrap">',
        '  <a class="g-back" href="%s"><span aria-hidden="true">←</span> %s</a>' % (P["home"], P["back"]),
        '  <h1 class="g-title mega">%s</h1>' % title_text,
        '  <p class="g-actions"><a class="btn btn-primary" href="%s">%s%s</a></p>' % (GH, DL_ICON, P["download"]),
        "</div>",
        '<div class="g-layout wrap">',
        '<details class="toc" open>',
        '  <summary class="wide">%s</summary>' % P["toc"],
        '  <nav class="toc-nav" aria-label="%s">\n%s\n  </nav>' % (P["toc"], toc_html(sections, ctx)),
        "</details>",
        '<main class="sheet">',
        '<p class="g-gen">%s · <a href="%s/commit/%s"><code>%s</code></a> · <time datetime="%s">%s</time> · <a href="%s%s%s">%s</a></p>'
        % (P["gen"], GH, h, h, date, date, BLOB, SRC_REL, P["view_frag"], P["view"]),
        '<div class="doc-body">',
        render(pre, ctx),
        "\n".join(body_sections),
        "</div>",
        "</main>",
        "</div>",
        '<footer class="foot g-foot wrap">',
        '  <div class="foot-rule"></div>',
        '  <div class="foot-row">',
        '    <a class="lockup" href="%s"><img src="%simg/icon.svg" width="44" height="44" alt=""><span class="wm">Backtracking</span></a>' % (P["home"], root),
        '    <a class="btn btn-primary" href="%s">%s%s</a>' % (GH, DL_ICON, P["download"]),
        "  </div>",
        '  <nav class="foot-links" aria-label="Project links">',
        '    <a href="%s">%s</a>' % (P["home"], P["back"]),
        '    <a href="%s">%s</a>' % (GH, P["repo"]),
        '    <a href="%s/blob/main/LICENSE">%s</a>' % (GH, P["license"]),
        '    <a href="%s" target="_blank" rel="noopener">%s</a>' % (SUPPORT, P["support"]),
        "  </nav>",
        '  <p class="fine">%s</p>' % P["fine"],
        "</footer>",
        "</body>",
        "</html>",
        "",
    ]
    return "\n".join(parts)


def last_commit():
    try:
        out = subprocess.run(["git", "log", "-1", "--format=%h %cs", "--", SRC_REL], cwd=REPO,
                             capture_output=True, text=True, check=True).stdout.strip()
    except (OSError, subprocess.CalledProcessError):
        out = ""
    if not out:
        raise SystemExit("could not read the last commit of %s (git log)" % SRC_REL)
    h, d = out.split()
    return h, d


def main():
    with open(SRC, encoding="utf-8") as fh:
        lines = fh.read().replace("\r\n", "\n").split("\n")
    blocks = parse_blocks(lines)
    assign_ids(blocks, set())
    pre, parts = split_parts(blocks)
    commit = last_commit()

    built = {}
    for lang in ("en", "zh"):
        built[lang] = page_blocks(pre, parts, lang)
    ids = {}
    for lang in ("en", "zh"):
        h1, rest, sections = built[lang]
        s = collect_ids(rest, set())
        for head, blocks_, wrap in sections:
            if head is not None:
                s.add(head["id"])
            if wrap:
                s.add(wrap)
            collect_ids(blocks_, s)
        ids[lang] = s

    problems = []
    for lang in ("en", "zh"):
        other = "zh" if lang == "en" else "en"
        ctx = Ctx(lang, ids[lang], ids[other], PAGES[lang]["other"])
        h1, rest, sections = built[lang]
        page = build_page(lang, h1, rest, sections, commit, ctx)
        out = PAGES[lang]["out"]
        os.makedirs(os.path.dirname(out), exist_ok=True)
        old = None
        if os.path.exists(out):
            with open(out, encoding="utf-8") as fh:
                old = fh.read()
        if old != page:
            with open(out, "w", encoding="utf-8", newline="\n") as fh:
                fh.write(page)
            print("wrote " + os.path.relpath(out, SITE))
        else:
            print("unchanged " + os.path.relpath(out, SITE))
        problems += ["%s: %s" % (lang, p) for p in ctx.problems]
    for p in sorted(set(problems)):
        print("WARNING " + p)
    if problems:
        sys.exit(1)


if __name__ == "__main__":
    main()
