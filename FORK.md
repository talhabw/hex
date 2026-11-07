# talhabw/hex fork

Upstream Hex ([anomalyco/hex](https://github.com/anomalyco/hex)) plus one small patch: **Linux dictation through Deepgram** (Nova-3, multilingual) instead of the local model.

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

- Hex reads the file on every dictation, so you don't need to restart after editing it. Set `"enabled": false` (or delete the file) to go back to the local model.
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

The source install puts a `source-build` marker next to the binary, so Hex's own auto-updater won't overwrite the patched build. The previous binary is kept at `~/.local/share/hex/hex.previous`.

Build deps (Ubuntu): `clang cmake pkg-config jq libasound2-dev libssl-dev libxkbcommon-x11-dev libx11-xcb-dev libfontconfig1-dev libfreetype-dev libxcursor-dev libxi-dev libxrandr-dev libopenblas-dev libvulkan-dev glslc libshaderc-dev`.
