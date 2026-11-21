# Parity audit 2: decisions left to the owner

Each section states what the audit asked, what was done on the branch, and the
choice that remains, with a recommendation. Nothing here is acted on until the
owner decides.

## M7: key order of `serde_json` maps

Done on the branch: `serde_json/preserve_order` is no longer forced on every
dependent. The library turns it on through its own `preserve-order` feature,
on by default, and builds and passes its tests without it (removal goes
through `json_map::RemoveKey`, which keeps the order in both
representations; the tests that check key order are gated on the feature).

Why on by default: the differential harness compares key order at every
depth, because it reaches the model (the properties of a tool schema) and the
on-disk transcripts the CLI reads. Values the program builds are
`serde_json::Value`s; without the feature their keys are sorted the moment
they are built, and no code in the library can restore the order the program
wrote. Off by default would make every program diverge from Python unless it
knew to turn the feature on.

Remaining choice:

- **Keep it (recommended for now).** Parity by default, an opt-out for the
  programs that need sorted maps, documented in the crate docs and README.
- **Own ordered JSON type in the public API.** Replace `serde_json::Value`
  and `Map` in public fields (`extra`, tool input, schemas) with a type that
  always keeps order, so the library never touches the program's
  `serde_json` features. Exact in every build, but a breaking change of most
  of the public surface: a major release, and an owner decision.

## M5: opting out of the process-group sweep

Done on the branch: what is left in the group of the CLI when it exits gets
`SIGTERM`, then `SIGKILL` 5 s later only if it is still running.

Remaining choice: an option to leave background processes alone (Python
signals only the direct child). The current policy was the owner's decision
A12 of the first audit; an opt-out would be a new public option
(`ClaudeAgentOptions::keep_background_processes` or similar), which is a
naming decision. Recommendation: no option until a user asks; `setsid`
already lets an agent detach a process on purpose.
