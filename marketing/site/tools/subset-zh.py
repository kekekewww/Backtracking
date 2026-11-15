"""Build fonts/NotoSansTC-subset.woff2 for the Traditional Chinese page.

Collects every CJK / full-width character used in zh-tw/index.html (page text, attributes and the
demo strings JSON) and in the parts of zh-tw/guide/index.html that are set in this font (page chrome,
contents, headings; the guide's body text uses the system CJK fonts), subsets Noto Sans TC (variable)
to exactly those glyphs, limits the weight axis to 400-900 (the weights the pages use) and writes a
woff2. The output is reproducible (no timestamp).

Re-run whenever Chinese copy in zh-tw/index.html or a Chinese guide heading changes
(tools/sync-media.py runs it for you):

    python tools/subset-zh.py [path/to/NotoSansTC-Variable.ttf]

Default input font: ../../fonts/NotoSansTC-Variable.ttf (marketing/fonts/, not deployed).
Needs fontTools and brotli:  python -m pip install --user fonttools brotli
"""
import os
import sys
from html.parser import HTMLParser

from fontTools import subset
from fontTools.ttLib import TTFont
from fontTools.varLib import instancer

HERE = os.path.dirname(os.path.abspath(__file__))
SITE = os.path.dirname(HERE)
PAGES = [os.path.join(SITE, "zh-tw", "index.html")]
GUIDE = os.path.join(SITE, "zh-tw", "guide", "index.html")
SRC = sys.argv[1] if len(sys.argv) > 1 else os.path.join(SITE, "..", "fonts", "NotoSansTC-Variable.ttf")
OUT = os.path.join(SITE, "fonts", "NotoSansTC-subset.woff2")


def wanted(ch):
    cp = ord(ch)
    return (
        0x2E80 <= cp <= 0x9FFF      # CJK radicals, punctuation, kana, unified ideographs
        or 0xF900 <= cp <= 0xFAFF   # compatibility ideographs
        or 0xFE30 <= cp <= 0xFE4F   # CJK compatibility forms
        or 0xFF00 <= cp <= 0xFFEF   # full-width forms (，／：；「」 etc.)
        or 0x20000 <= cp <= 0x2FFFF
    )


chars = set()
for page in PAGES:
    with open(page, encoding="utf-8") as fh:
        chars.update(c for c in fh.read() if wanted(c))


class GuideText(HTMLParser):
    """Text of zh-tw/guide/index.html except the body text inside .doc-body (headings are kept)."""
    HEADS = {"h1", "h2", "h3", "h4", "h5", "h6"}
    VOID = {"meta", "link", "img", "br", "hr", "input", "source"}

    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.stack, self.doc_depth, self.head_depth, self.out = [], None, 0, []

    def handle_starttag(self, tag, attrs):
        a = dict(attrs)
        if a.get("class") == "doc-body" and self.doc_depth is None:
            self.doc_depth = len(self.stack)
        for k, v in attrs:
            if v and k not in ("href", "id", "src"):
                self.out.append(v)
        if tag in self.VOID:
            return
        self.stack.append(tag)
        if tag in self.HEADS:
            self.head_depth += 1

    def handle_endtag(self, tag):
        if tag in self.VOID or tag not in self.stack:
            return
        while self.stack:
            t = self.stack.pop()
            if t in self.HEADS:
                self.head_depth -= 1
            if self.doc_depth is not None and len(self.stack) == self.doc_depth:
                self.doc_depth = None
            if t == tag:
                break

    def handle_data(self, data):
        if self.doc_depth is None or self.head_depth > 0:
            self.out.append(data)


if os.path.exists(GUIDE):
    g = GuideText()
    with open(GUIDE, encoding="utf-8") as fh:
        g.feed(fh.read())
    chars.update(c for c in "".join(g.out) if wanted(c))
text = "".join(sorted(chars))

font = TTFont(SRC, recalcTimestamp=False)
opts = subset.Options()
opts.flavor = None
opts.layout_features = ["*"]
opts.hinting = False
opts.desubroutinize = True
opts.name_IDs = ["*"]
opts.notdef_outline = True
sub = subset.Subsetter(opts)
sub.populate(text=text)
sub.subset(font)

font = instancer.instantiateVariableFont(font, {"wght": (400, 900)})
font.recalcTimestamp = False
font["head"].modified = font["head"].created
font.flavor = "woff2"
font.save(OUT)

print("characters: %d" % len(chars))
print("wrote %s (%d bytes)" % (OUT, os.path.getsize(OUT)))
