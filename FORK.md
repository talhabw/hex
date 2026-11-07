# talhabw/hex fork

Upstream Hex ([anomalyco/hex](https://github.com/anomalyco/hex)) plus one small patch: **Linux dictation through Deepgram** (Nova-3, multilingual) instead of the local model.

## What the patch does

- **Labs › Deepgram transcription** turns it on. The options under it: Mode (Live / Whole clip), Language (Multilingual / Turkish / English), pause before a phrase ends (endpointing), Spoken punctuation, Numbers as digits.
- Live mode streams over Deepgram's WebSocket, so you get live text without a GPU. A stream that fails falls back to a whole-clip request.
- In multilingual mode, a result in a non-Latin script (e.g. Persian) is transcribed again with `retry_language` (default `tr`).
- Hex starts without loading a local model and hides General's missing-model warnings while Deepgram is on.
- `scripts/deepgram-compare.py` sends your last retained dictations to Deepgram both live and as a batch request, for comparison. Clips are kept via the `HEX_RETAIN_DICTATION_AUDIO=20` systemd drop-in in `~/.config/systemd/user/hex.service.d/fork.conf`.

## Setup

`~/.config/hex/remote-transcription.json` (`chmod 600`):

```json
{
  "enabled": true,
  "provider": "deepgram",
  "api_key": "YOUR_DEEPGRAM_KEY",
  "model": "nova-3",
  "language": "multi",
  "smart_format": true,
  "use_vocabulary": true,
  "fallback_to_local": true,
  "timeout_seconds": 30,
  "extra_params": {}
}
```

- Hex reads the file on every dictation, so you don't need to restart after editing it. The Labs options override `streaming`, `language`, `stream_endpointing_ms`, `dictation` and `numerals` from this file.
- `api_key` can be left empty to use `DEEPGRAM_API_KEY` from `~/.local/share/hex/session.env`.
- `use_vocabulary` sends Hex's vocabulary terms as Nova-3 `keyterm`s.
- `fallback_to_local`: if the API fails, the clip is transcribed with the selected local model.
- `extra_params`: any additional Deepgram `/v1/listen` query params (e.g. `{"numerals": "true"}`).
- While remote is on, live text/streaming is off: text arrives after you release the shortcut.
- Hex's modes, replacements, transformations and history still apply to the Deepgram text.

The patch: `src/transcription/remote.rs` + a few lines in `src/transcription/engine.rs` (`begin`, `finish`).

## Update to the newest upstream release

```sh
scripts/fork-update.sh        # rebase on upstream/production, push to fork, build, install, restart
scripts/fork-update.sh dev    # track upstream/dev instead
scripts/fork-update.sh --no-rebase
```

Upstream keeps adding Labs features in the same lists, so `src/labs.rs`, `src/app_settings.rs`, `src/control/settings.rs` and `src/rune_ui/labs.rs` usually conflict on a rebase. Both sides only add lines, so the fix is to keep both. `git rerere` remembers each resolution.

The source install puts a `source-build` marker next to the binary, so Hex's own auto-updater won't overwrite the patched build. The previous binary is kept at `~/.local/share/hex/hex.previous`.

Build deps (Ubuntu): `clang cmake pkg-config jq libasound2-dev libssl-dev libxkbcommon-x11-dev libx11-xcb-dev libfontconfig1-dev libfreetype-dev libxcursor-dev libxi-dev libxrandr-dev libopenblas-dev libvulkan-dev glslc libshaderc-dev`.
