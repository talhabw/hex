#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["websockets>=13", "httpx>=0.27"]
# ///
"""Fork tool: send the same WAV to Deepgram as a live stream and as a batch
request, with the settings Hex uses, and print both transcripts and timings.

  scripts/deepgram-compare.py [clip.wav ...]   # default: last 5 retained dictations
  options: --language multi|tr|en  --endpointing 300
"""
import argparse, asyncio, glob, json, os, time, wave
import httpx, websockets

HOME = os.path.expanduser("~")
cfg = json.load(open(f"{HOME}/.config/hex/remote-transcription.json"))
labs = json.load(open(f"{HOME}/.local/share/hex/settings.json")).get("labs", {})

ap = argparse.ArgumentParser()
ap.add_argument("wavs", nargs="*")
ap.add_argument("--language", default=labs.get("remote_language", cfg.get("language", "multi")))
ap.add_argument("--endpointing", type=int, default=labs.get("remote_endpointing_ms", 300))
a = ap.parse_args()
wavs = a.wavs or sorted(glob.glob(f"{HOME}/.local/share/hex/diagnostic-dictation-audio/dictation-*[0-9].wav"))[-5:]
if not wavs:
    raise SystemExit("No clips. Dictate a few times first (HEX_RETAIN_DICTATION_AUDIO keeps them).")

def params(extra):
    p = [("model", cfg.get("model", "nova-3")), ("language", a.language),
         ("smart_format", "true"), ("punctuate", "true"),
         ("numerals", str(labs.get("remote_numerals", True)).lower()),
         ("dictation", str(labs.get("remote_dictation", False)).lower())]
    if a.language == "multi":
        p += [("language_hint", h) for h in cfg.get("extra_params", {}).get("language_hint", [])]
    return p + extra

HEAD = {"Authorization": f"Token {cfg['api_key']}"}

def batch(path):
    t = time.time()
    r = httpx.post("https://api.deepgram.com/v1/listen", params=params([]), headers={**HEAD, "Content-Type": "audio/wav"},
                   content=open(path, "rb").read(), timeout=60)
    r.raise_for_status()
    return r.json()["results"]["channels"][0]["alternatives"][0]["transcript"], time.time() - t

async def stream(path):
    with wave.open(path) as w:
        rate, pcm = w.getframerate(), w.readframes(w.getnframes())
    q = httpx.QueryParams(params([("encoding", "linear16"), ("sample_rate", str(rate)), ("channels", "1"),
                                  ("interim_results", "true"), ("endpointing", str(a.endpointing))]))
    finals = []
    async with websockets.connect(f"wss://api.deepgram.com/v1/listen?{q}", additional_headers=HEAD) as ws:
        async def send():
            chunk = rate // 10 * 2  # 100 ms, real time, like Hex
            for i in range(0, len(pcm), chunk):
                await ws.send(pcm[i:i + chunk]); await asyncio.sleep(0.1)
            nonlocal closed; closed = time.time()
            await ws.send(json.dumps({"type": "CloseStream"}))
        closed = None
        sender = asyncio.create_task(send())
        async for msg in ws:
            m = json.loads(msg)
            if m.get("type") == "Results" and m.get("is_final"):
                t = m["channel"]["alternatives"][0]["transcript"].strip()
                if t: finals.append(t)
        await sender
    return " ".join(finals), time.time() - closed

for path in wavs:
    with wave.open(path) as w: secs = w.getnframes() / w.getframerate()
    bt, bs = batch(path)
    st, ss = asyncio.run(stream(path))
    print(f"\n{os.path.basename(path)}  ({secs:.1f} s audio, language={a.language}, endpointing={a.endpointing})")
    print(f"  batch  {bs:4.2f} s after send : {bt}")
    print(f"  stream {ss:4.2f} s after release: {st}")
