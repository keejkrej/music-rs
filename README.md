# Music RS

`daw` starts as a manual loop-sketch DAW with a blank project. It has no AI prompt box in the UI.

Projects on disk are always a **folder** with `project.json` plus one JSON file per track under `tracks/`. Each note in a track file is a compact four-number array (same information as a MIDI note-on plus duration in beats):

`[pitch, velocity_midi, start_beat, length_beats]`

`velocity_midi` is a standard MIDI velocity (0–127). Use `File > Open project.json...` or **Open Project Folder...** to load. A small example lives at `examples/projects/happy_birthday/`.

External agents can control the same DAW over a localhost WebSocket using JSON-RPC 2.0:

```bash
cargo run
cargo run -- serve 4141
```

Connect to:

```text
ws://127.0.0.1:4141
```

Each WebSocket text message is one JSON-RPC 2.0 request. Requests with an `id` receive a JSON-RPC response. Requests without an `id` are notifications and do not receive a response.

```json
{"jsonrpc":"2.0","id":"1","method":"get_summary"}
{"jsonrpc":"2.0","id":"2","method":"apply_commands","params":{"commands":[{"action":"set_tempo","bpm":124.0}]}}
{"jsonrpc":"2.0","id":"3","method":"play","params":{"looping":true}}
{"jsonrpc":"2.0","id":"4","method":"export_wav","params":{"path":"/tmp/loop.wav"}}
```

Successful response:

```json
{"jsonrpc":"2.0","id":"2","result":{"summary":"Untitled Loop: 124.0 BPM, 4 bars, 2 tracks"}}
```

Error response:

```json
{"jsonrpc":"2.0","id":"2","error":{"code":-32000,"message":"tempo must be between 40 and 240 BPM"}}
```

Supported methods are `get_summary`, `get_project`, `apply_commands`, `play`, `stop`, `undo`, `redo`, `save`, `load`, and `export_wav`.

`apply_commands` accepts the structured edit protocol from `src/commands.rs`, including `create_track`, `add_notes`, `replace_clip`, `set_tempo`, `make_drum_pattern`, `arrange_loop`, and `set_mixer`.

Convert a Standard MIDI File into the same project layout:

```bash
cargo run -- midi-to-json piece.mid ./my_project
# writes ./my_project/project.json and ./my_project/tracks/*.json
```

Track files are named from each track’s **title** (e.g. `000_kurt_cobain_vocals.json`), not its internal id. To rewrite an existing folder on disk (refresh filenames and strip stale `tracks/*.json`), run:

```bash
cargo run -- resave-project /path/to/project_or_project.json
```

