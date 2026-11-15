"""Copy campaign posters and videos into site/media/ and refresh the generated page blocks.

    python tools/sync-media.py            (run from marketing/site or anywhere; paths are relative to this file)

What it does (safe to re-run; unchanged files are skipped):
  - posters: ../posters/out/Backtracking-<name>-<lang>.png  -> media/posters/ (full size, for download)
             + media/posters/thumbs/<same>.webp (640 px wide) for the gallery
  - videos:  ../video/out/overview-<lang>.mp4, overview-poster-<lang>.jpg,
             short-<lang>.mp4, short-poster-<lang>.jpg      -> media/
             loop-<name>.mp4 / .webm                         -> media/loops/  + a still frame <name>.webp
             Files that are missing, too small or have the wrong signature (still rendering) are skipped.
  - music:   ../audio-src/out/demo-loop.mp3 -> media/demo-loop.bin, scrambled with a fixed reversible
             byte transform so the site never serves a playable music file (licensed; media/ is not
             committed; skipped with a message when absent). Any old media/demo-loop.mp3 is deleted.
  - pages:   rewrites the blocks between <!-- posters:start --> / <!-- posters:end --> and
             <!-- watch:start --> / <!-- watch:end --> in index.html and zh-tw/index.html from what is
             actually in media/ (gallery order: the five edition posters, then the social formats, as in
             ../posters/src/manifest.js; pixel sizes are read from the PNG files).
  - then runs tools/subset-zh.py if fontTools is importable (the gallery adds Chinese captions).

Needs Python 3.8+ and ffmpeg (with libwebp). ffmpeg is taken from $FFMPEG, then PATH, then the
WinGet Gyan.FFmpeg package folder on Windows.
"""
import glob
import html
import os
import re
import shutil
import struct
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
SITE = os.path.dirname(HERE)
MKT = os.path.dirname(SITE)
POSTERS_OUT = os.path.join(MKT, "posters", "out")
MANIFEST = os.path.join(MKT, "posters", "src", "manifest.js")
VIDEO_OUT = os.path.join(MKT, "video", "out")
AUDIO_OUT = os.path.join(MKT, "audio-src", "out")
MEDIA = os.path.join(SITE, "media")
LANGS = ["en", "zh-TW"]
LOOPS = ["loop-select", "loop-drag-out", "loop-ab-sources", "loop-waveform-only", "loop-snapshot-shortcut"]
THUMB_W = 640
STILL_W = 960

LABELS = {
    "edition-master": ("Master poster", "主視覺海報"),
    "edition-windows": ("Windows desktop", "Windows 桌面版"),
    "edition-macos": ("macOS desktop", "macOS 桌面版"),
    "edition-plugin": ("VST3 / CLAP plug-in", "VST3／CLAP 插件"),
    "edition-linux": ("Linux shortcut helper", "Linux 快捷鍵輔助工具"),
    "github-social": ("GitHub social preview", "GitHub 社群預覽圖"),
    "instagram-feed": ("Instagram feed post", "Instagram 貼文"),
    "instagram-story": ("Instagram story", "Instagram 限時動態"),
    "facebook-cover": ("Facebook cover", "Facebook 封面"),
    "facebook-feed": ("Facebook feed post", "Facebook 貼文"),
    "threads-post": ("Threads post", "Threads 貼文"),
    "buymeacoffee-cover": ("Buy Me a Coffee cover", "Buy Me a Coffee 封面"),
    "youtube-channel": ("YouTube channel banner", "YouTube 頻道橫幅"),
    "youtube-thumbnail": ("YouTube thumbnail", "YouTube 縮圖"),
}
TEXT = {
    "en": {
        "edition": "Edition posters", "social": "Social formats", "download": "Download PNG",
        "newtab": "(opens the full-size PNG in a new tab)",
        "overview": "Overview", "short": "Short, 9:16",
        "overviewLabel": "Backtracking overview video", "shortLabel": "Backtracking short vertical video",
        "missing": "The video could not be loaded.",
    },
    "zh-TW": {
        "edition": "版本海報", "social": "社群尺寸", "download": "下載 PNG",
        "newtab": "（在新分頁開啟原尺寸 PNG）",
        "overview": "完整介紹", "short": "直式短片 9:16",
        "overviewLabel": "Backtracking 介紹影片", "shortLabel": "Backtracking 直式短片",
        "missing": "影片無法載入。",
    },
}


def log(msg):
    print(msg, flush=True)


def find_ffmpeg():
    cands = [os.environ.get("FFMPEG"), shutil.which("ffmpeg")]
    if os.name == "nt":
        root = os.path.join(os.environ.get("LOCALAPPDATA", ""), "Microsoft", "WinGet", "Packages")
        cands += glob.glob(os.path.join(root, "Gyan.FFmpeg*", "*", "bin", "ffmpeg.exe"))
    for c in cands:
        if not c:
            continue
        try:
            subprocess.run([c, "-version"], capture_output=True, check=True)
            return c
        except (OSError, subprocess.CalledProcessError):
            continue
    return None


def valid(path, kind):
    """Reject missing, tiny or half-written files."""
    if not os.path.isfile(path):
        return False
    size = os.path.getsize(path)
    with open(path, "rb") as fh:
        head = fh.read(12)
    if kind == "mp4":
        return size > 20000 and head[4:8] == b"ftyp"
    if kind == "webm":
        return size > 20000 and head[:4] == b"\x1a\x45\xdf\xa3"
    if kind == "jpg":
        return size > 2000 and head[:3] == b"\xff\xd8\xff"
    if kind == "png":
        return size > 2000 and head[:8] == b"\x89PNG\r\n\x1a\n"
    return False


def png_size(path):
    with open(path, "rb") as fh:
        head = fh.read(24)
    return struct.unpack(">II", head[16:24])


def same(src, dst):
    return os.path.isfile(dst) and os.path.getsize(dst) == os.path.getsize(src) and os.path.getmtime(dst) >= os.path.getmtime(src)


def copy(src, dst):
    if same(src, dst):
        return False
    os.makedirs(os.path.dirname(dst), exist_ok=True)
    shutil.copy2(src, dst)
    return True


def ffmpeg_image(ff, src, dst, width, height=None, seek=None):
    if os.path.isfile(dst) and os.path.getmtime(dst) >= os.path.getmtime(src):
        return False
    os.makedirs(os.path.dirname(dst), exist_ok=True)
    scale = "scale=%d:%d" % (width, height) if height else "scale=%d:-2" % width
    cmd = [ff, "-v", "error", "-y"]
    if seek is not None:
        cmd += ["-ss", str(seek)]
    cmd += ["-i", src, "-frames:v", "1", "-vf", scale + ":flags=lanczos", "-c:v", "libwebp", "-quality", "80", dst]
    subprocess.run(cmd, check=True)
    return True


def manifest_order():
    with open(MANIFEST, encoding="utf-8") as fh:
        text = fh.read()
    return re.findall(r'name:\s*"([^"]+)",\s*w:\s*\d+,\s*h:\s*\d+,\s*set:\s*"(\w+)"', text)


def sync_posters(ff):
    items = {lang: [] for lang in LANGS}
    for name, group in manifest_order():
        for lang in LANGS:
            fname = "Backtracking-%s-%s.png" % (name, lang)
            src = os.path.join(POSTERS_OUT, fname)
            if not valid(src, "png"):
                log("  skip poster (missing or invalid): " + fname)
                continue
            dst = os.path.join(MEDIA, "posters", fname)
            if copy(src, dst):
                log("  copied " + fname)
            w, h = png_size(dst)
            tw, th = THUMB_W, max(1, round(h * THUMB_W / w))
            thumb = os.path.join(MEDIA, "posters", "thumbs", fname[:-4] + ".webp")
            if ffmpeg_image(ff, dst, thumb, tw, th):
                log("  thumb  " + os.path.basename(thumb))
            items[lang].append({"name": name, "set": group, "file": fname, "w": w, "h": h, "tw": tw, "th": th})
    return items


def sync_videos(ff):
    have = set()
    for lang in LANGS:
        for base, kind in (("overview-%s.mp4", "mp4"), ("overview-poster-%s.jpg", "jpg"),
                           ("short-%s.mp4", "mp4"), ("short-poster-%s.jpg", "jpg")):
            fname = base % lang
            src = os.path.join(VIDEO_OUT, fname)
            if valid(src, kind):
                if copy(src, os.path.join(MEDIA, fname)):
                    log("  copied " + fname)
                have.add(fname)
            else:
                log("  skip video (missing or still rendering): " + fname)
                stale = os.path.join(MEDIA, fname)
                if os.path.isfile(stale) and not valid(stale, kind):
                    os.remove(stale)
            if os.path.isfile(os.path.join(MEDIA, fname)) and valid(os.path.join(MEDIA, fname), kind):
                have.add(fname)
    for loop in LOOPS:
        ok = False
        for ext in ("mp4", "webm"):
            src = os.path.join(VIDEO_OUT, loop + "." + ext)
            if valid(src, ext):
                if copy(src, os.path.join(MEDIA, "loops", loop + "." + ext)):
                    log("  copied %s.%s" % (loop, ext))
                ok = True
            else:
                log("  skip loop (missing or still rendering): %s.%s" % (loop, ext))
        mp4 = os.path.join(MEDIA, "loops", loop + ".mp4")
        if ok and valid(mp4, "mp4"):
            if ffmpeg_image(ff, mp4, os.path.join(MEDIA, "loops", loop + ".webp"), STILL_W, seek=1.0):
                log("  still  %s.webp" % loop)
    return have


SCRAMBLE_MAGIC = b"BTLOOP1\n"  # js/demo.js checks this before unscrambling


def scramble(data):
    """Fixed, reversible byte transform (XOR with a key table and the byte index; applying it twice
    gives the input back). js/demo.js has the same function. This is NOT copy protection: anyone with
    the page's JavaScript can reverse it. Its only purpose is that the site does not serve the licensed
    music as a file that plays on its own; the result has no ID3/MPEG header and players and ffprobe
    do not recognise it as audio."""
    key, x = [], 0x9E3779B9
    for _ in range(256):
        x ^= (x << 13) & 0xFFFFFFFF
        x ^= x >> 17
        x ^= (x << 5) & 0xFFFFFFFF
        key.append(x & 0xFF)
    out = bytearray(data)
    for i in range(len(out)):
        out[i] ^= key[i & 255] ^ ((i * 131 + (i >> 8)) & 0xFF)
    return bytes(out)


def sync_demo_music():
    """The demo's licensed loop, written scrambled as media/demo-loop.bin (never a playable .mp3).
    media/ is not committed (see .gitignore); the .bin is deployed with the site."""
    src = os.path.join(AUDIO_OUT, "demo-loop.mp3")
    dst = os.path.join(MEDIA, "demo-loop.bin")
    old = os.path.join(MEDIA, "demo-loop.mp3")
    if os.path.exists(old):
        os.remove(old)
        log("  removed media/demo-loop.mp3 (the site must not serve the music as a file)")
    ok = os.path.isfile(src) and os.path.getsize(src) > 20000
    if ok:
        with open(src, "rb") as fh:
            head = fh.read(3)
        ok = head == b"ID3" or (head[0] == 0xFF and head[1] & 0xE0 == 0xE0)
    if not ok:
        log("  skip demo music: %s not found (the demo will use its synthesised fallback)"
            % os.path.relpath(src, MKT))
        return
    if os.path.isfile(dst) and os.path.getmtime(dst) >= os.path.getmtime(src):
        return
    with open(src, "rb") as fh:
        data = fh.read()
    os.makedirs(MEDIA, exist_ok=True)
    with open(dst, "wb") as fh:
        fh.write(SCRAMBLE_MAGIC + scramble(data))
    log("  wrote demo-loop.bin (scrambled demo-loop.mp3)")


def gallery_html(lang, items, prefix):
    t = TEXT[lang]
    out = ["<!-- posters:start (generated by tools/sync-media.py; edit the script, not this block) -->"]
    for group in ("edition", "social"):
        rows = [i for i in items if i["set"] == group]
        if not rows:
            continue
        out.append('<h3 class="gallery-title wide">%s</h3>' % t[group])
        out.append('<ul class="gallery gallery-%s">' % group)
        for i in rows:
            label = LABELS.get(i["name"], (i["name"], i["name"]))[0 if lang == "en" else 1]
            full = prefix + "media/posters/" + i["file"]
            thumb = prefix + "media/posters/thumbs/" + i["file"][:-4] + ".webp"
            size = "%d × %d" % (i["w"], i["h"])
            out.append(
                '<li class="poster"><a class="poster-img" href="%s" target="_blank" rel="noopener">'
                '<img src="%s" width="%d" height="%d" loading="lazy" decoding="async" alt="%s, %s">'
                '<span class="sr-only"> %s</span></a>'
                '<p class="poster-cap"><span class="poster-name">%s</span> <span class="poster-size">%s</span></p>'
                '<a class="poster-dl" href="%s" download>%s</a></li>'
                % (full, thumb, i["tw"], i["th"], html.escape(label), size, t["newtab"],
                   html.escape(label), size, full, t["download"]))
        out.append("</ul>")
    out.append("<!-- posters:end -->")
    return "\n".join(out)


def watch_html(lang, have, prefix):
    t = TEXT[lang]
    players = []
    for kind, ratio, cls in (("overview", "wide", "player-wide"), ("short", "tall", "player-tall")):
        mp4 = "%s-%s.mp4" % (kind, lang)
        poster = "%s-poster-%s.jpg" % (kind, lang)
        if kind == "short" and mp4 not in have:
            continue  # the vertical short is optional; the overview frame is always shown
        attr = ' data-poster="%smedia/%s"' % (prefix, poster) if poster in have else ""
        players.append(
            '<figure class="player %s"><div class="video-frame frame-%s">'
            '<video controls preload="none" playsinline%s aria-label="%s">'
            '<source src="%smedia/%s" type="video/mp4"></video></div>'
            '<p class="video-note" hidden>%s</p>'
            '<figcaption class="video-cap wide">%s</figcaption></figure>'
            % (cls, ratio, attr, t[kind + "Label"], prefix, mp4, t["missing"], t[kind]))
    return ("<!-- watch:start (generated by tools/sync-media.py) -->\n"
            '<div class="players players-%d">\n%s\n</div>\n<!-- watch:end -->' % (len(players), "\n".join(players)))


def replace_block(text, name, block):
    pat = re.compile(r"<!-- %s:start.*?<!-- %s:end -->" % (name, name), re.S)
    if not pat.search(text):
        raise SystemExit("marker <!-- %s:start --> not found" % name)
    return pat.sub(lambda m: block, text)


def main():
    ff = find_ffmpeg()
    if not ff:
        raise SystemExit("ffmpeg not found: set FFMPEG=<path to ffmpeg>")
    log("posters")
    posters = sync_posters(ff)
    log("videos")
    have = sync_videos(ff)
    log("demo music")
    sync_demo_music()
    for lang, page, prefix in (("en", "index.html", ""), ("zh-TW", os.path.join("zh-tw", "index.html"), "../")):
        path = os.path.join(SITE, page)
        with open(path, encoding="utf-8") as fh:
            text = fh.read()
        new = replace_block(text, "posters", gallery_html(lang, posters[lang], prefix))
        new = replace_block(new, "watch", watch_html(lang, have, prefix))
        if new != text:
            with open(path, "w", encoding="utf-8", newline="\n") as fh:
                fh.write(new)
            log("updated " + page)
        else:
            log("unchanged " + page)
    try:
        import fontTools  # noqa: F401
        subprocess.run([sys.executable, os.path.join(HERE, "subset-zh.py")], check=True)
    except ImportError:
        log("fontTools not available: run tools/subset-zh.py yourself if the Chinese captions changed")


if __name__ == "__main__":
    main()
