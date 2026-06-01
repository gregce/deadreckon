# DeadReckon Seam Examples

This directory is a small conformance kit for the `[seams]` worker protocol.
Run the commands from the repository root so the relative paths in
`config.toml` resolve.

```sh
deadreckon seams validate policy --config examples/seams/config.toml --fixture examples/seams/fixtures/policy-allow.json --sandbox none
deadreckon seams validate catalog --config examples/seams/config.toml --sandbox none
deadreckon seams validate hooks --config examples/seams/config.toml --sandbox none
deadreckon seams validate event-sink --config examples/seams/config.toml --sandbox none
```

The policy seam is fail-closed, the catalog seam is fail-open, and hooks plus
event sink are fail-safe observers. `policy-deny.sh` is also a valid policy
worker; point `[seams.policy].command` at it to test a deliberate denial.

The acceptance gate is not a seam. Workers receive only their JSON request on
stdin and run through the normal seam sandbox; gate nonces, proof files, and
acceptance-marker internals are not part of the seam protocol.

Use `deadreckon run ... --no-seams` or `deadreckon start ... --no-seams` to
force built-in workers for a launch.
