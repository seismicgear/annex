# Optional local Bark speech backend

`scripts/bark_tts.py` runs the actual Hugging Face Bark model. Piper remains the
default. Bark is an optional operator installation with a separate Python
environment and model cache; it is not bundled into the desktop installer.

Install a compatible PyTorch build and `scripts/bark-requirements.txt` in that
environment. Run its Python interpreter with `scripts/bark_tts.py --prepare`
once to download the model. Normal speech generation is offline and fails if
the pinned model is absent. The default is `suno/bark-small` at commit
`1dbd7a128513b8ae4a4e2130fed57b7ac9da5bcd`; a different model requires an explicit
immutable revision. The adapter refuses remote model code and pickle-based
speaker preset loading.

Set `ANNEX_BARK_BINARY_PATH` (or `voice.bark_binary_path` in server configuration)
to an executable launcher that runs that environment's Python and this script,
forwarding every argument without shell evaluation. On Linux, a launcher can
use `exec /absolute/environment/bin/python /absolute/annex/scripts/bark_tts.py "$@"`.
Set `HF_HOME` consistently for preparation and server execution. Each request
uses the existing `--text ... --output_raw` interface and returns native
24 kHz mono signed 16-bit little-endian PCM on stdout. Diagnostics go to stderr.

Configure a `voice_profiles` row for the server with a stable `profile_id`,
`model='bark'`, and normal speed/pitch. Restart to load operator profiles, then
assign the profile through the moderator's existing agent voice-profile API.
Preparation alone is not voice acceptance: verify a real agent's speech intent,
the assigned profile, and a separate listener's decoded audio.

The current adapter runs on CPU by default with four threads; `ANNEX_BARK_THREADS`
sets a value from 1 through 32. The server bounds Bark synthesis at 180 seconds;
Piper and System retain their 60-second limits. The adapter refuses inputs longer than Bark's 256-token
window instead of silently truncating them. Optional speaker preparation uses
`--prepare --speaker 0` through `9`; matching synthesis uses `--speaker`.
`ANNEX_BARK_AUDIT_LOG` optionally appends model/revision, timing, byte counts and
text/audio hashes to an operator-selected file; it never writes the text itself.

Voice-profile speed, pitch and speaker controls require separate backend
qualification. Do not treat a configured parameter as applied without checking
the selected backend's behavior.
