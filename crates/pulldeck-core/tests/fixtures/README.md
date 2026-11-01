# Protocol fixtures

`frames.json` is one JSON frame per wire message, copied from the server repo
(`pulldeck-server`, `crates/pulldeck-proto/fixtures/frames.json`, at commit `fcf3c82`). Both
sides test against it, so a change to the protocol that only one side makes fails a test instead
of failing in production.

When the protocol changes, copy the new file over this one and fix whatever breaks. Frames
marked `"lossy": true` come from a hypothetical newer server and are not expected to round-trip
byte for byte.
