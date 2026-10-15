# Voice profile behavior

Profiles are operator-provisioned records loaded by the running speech service.
Assignment requires the current database record to match the validated loaded
profile. Changing a record requires reloading the service before it can be
assigned. A missing or invalid explicit profile is unavailable; it does not
silently select a different voice.

Every synthesis uses one profile and one configuration snapshot. The format
sent to the voice encoder comes from that same snapshot. Piper receives a
private temporary copy of the validated JSON so a concurrent configuration
edit cannot change the sample rate after Annex has selected it.

`speed` is a finite multiplier from 0.1 through 10.0. `pitch` is a finite
frequency factor from 0.25 through 4.0: 2.0 raises pitch one octave, and 0.5
lowers it one octave. Both default to 1.0. Unsupported values are rejected.
Pitch never changes the declared sample rate. Neutral processing retains
the original PCM bytes. Other processing uses pinned MIT Signalsmith Stretch
0.1.3, with bounded input/output and cancellation checks between chunks.

| Backend | Model and configuration | Speaker | Speed and pitch |
|---|---|---|---|
| Piper | `model_path` is an existing ONNX file, absolute or relative to the configured voices directory. `config_path` names its JSON; absent means `<model_path>.json`. The JSON must declare `audio.sample_rate`. | An explicit ID must be below the model's declared `num_speakers`. | Piper runs at its explicit neutral native timing (`length_scale=1`); independent DSP applies both factors to the complete utterance, including pauses. |
| Bark | Empty `model_path` selects `suno/bark-small` at revision `1dbd7a128513b8ae4a4e2130fed57b7ac9da5bcd`. Otherwise it names a model repository and requires a versioned JSON configuration with its exact 40-character revision. | English `v2/en_speaker_0` through `v2/en_speaker_9`, only when present in the selected pinned snapshot and already cached. | Independent DSP applies both factors after genuine 24 kHz Bark output. |
| System | Empty `model_path` selects installed `en-us`; otherwise it names an installed espeak-ng voice such as `en-us+m2`. Optional JSON supplies an explicit numeric-speaker mapping. The actual voice and variant catalogs must contain the result. | Requires `config.speakers` to map the ID to an installed voice. | espeak-ng runs at its explicit native baseline (`-s 175 -p 50`); independent DSP applies the shared factors. The native 0–99 pitch control is not misrepresented as a frequency ratio. |

Bark configuration example:

```json
{"version":1,"revision":"1dbd7a128513b8ae4a4e2130fed57b7ac9da5bcd"}
```

System configuration example:

```json
{"version":1,"speakers":{"0":"en-us+m1","1":"en-us+f2"}}
```

Configuration files are bounded to 1 MiB. Bark and System configurations reject
unknown fields and unsupported versions. Voice paths, model IDs and speaker
values never become shell command text.

The Bark wrapper's `--prepare` operation is the explicit model/preset download
step. Include `--speaker N` to prepare a requested speaker. Normal synthesis and
`--check` are offline. The readiness check confirms local configuration, tokenizer,
weights and selected preset files without loading Torch or generating audio.
Missing cached resources are a failure, never an implicit download.

Validation and assignment do not prove speech quality or delivery. Qualification
must separately retain actual provider audio, independent frequency/duration
measurements, and decoded remote-session evidence. Component process fixtures
test the invocation contract only.
