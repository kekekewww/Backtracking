# README media

Animated WebP copies of the five silent feature loops in `marketing/video/out/`, used by `README.md` and `README.zh-TW.md`. GitHub does not play repository videos inside a README, so the loops are converted. The loops carry no sentences, so one file serves both languages.

Re-create them after the loops are re-rendered (`loop-drag-out` at 800 px wide, the others at 640 px):

```sh
ffmpeg -i marketing/video/out/loop-select.mp4 -vf "fps=15,scale=640:-1:flags=lanczos" \
  -c:v libwebp_anim -lossless 0 -q:v 75 -compression_level 6 -loop 0 -an docs/media/loop-select.webp
```
