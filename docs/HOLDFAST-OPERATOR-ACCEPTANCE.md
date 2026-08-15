# Holdfast operator acceptance

Run this after building and installing the candidate binary. It exercises the
real detached supervisor and a deliberately unknown runtime-output name.

Pin the candidate before leaving the source tree. This prevents a shadowed
Homebrew or older PATH entry from silently running a different build:

```bash
holdfast_deadreckon="$(git rev-parse --show-toplevel)/target/release/deadreckon"
test -x "$holdfast_deadreckon"
"$holdfast_deadreckon" doctor --json
```

Expected: doctor is verified and its supervisor checkpoint reports the same
binary and bundle as `$holdfast_deadreckon`.

## 1. Prepare a disposable greenfield project

```bash
holdfast_project="$(mktemp -d)/holdfast-project"
mkdir -p "$holdfast_project"
cd "$holdfast_project"
git init -q
git config user.email holdfast@example.invalid
git config user.name 'Holdfast operator test'
printf '%s\n' '# Holdfast greenfield fixture' > README.md
git add README.md
git commit -q -m 'operator baseline'
```

Expected: the commit succeeds and `test ! -e .gitignore` exits 0. The project
has no ignore policy at admission. The name `.made-up-runtime-z91` used below
is intentionally not a framework DeadReckon knows.

Define and commit a real deterministic done contract before admission. The
requested files and ignore rule must be present. Do not require the runtime
directory to be absent here: the agent must leave it in the mutable workspace
so the sealed-result boundary—not the agent—proves its omission.

```bash
mkdir -p .deadreckon
cat > .deadreckon/acceptance.yaml <<'YAML'
name: holdfast unknown runtime projection
checks:
  - kind: file_exists
    path: "{working_dir}/app.txt"
  - kind: content_match
    path: "{working_dir}/app.txt"
    pattern: "holdfast works"
  - kind: file_exists
    path: "{working_dir}/dist/result.txt"
  - kind: content_match
    path: "{working_dir}/dist/result.txt"
    pattern: "ship me"
  - kind: content_match
    path: "{working_dir}/.gitignore"
    pattern: "/.made-up-runtime-z91/"
YAML
git add -f .deadreckon/acceptance.yaml
git commit -q -m 'add holdfast done contract'
"$holdfast_deadreckon" def-done show
```

Expected: `def-done show` displays the five checks above. A pre-run
`"$holdfast_deadreckon" def-done check` should fail because the requested
files do not exist yet. Do not accept the unknown-project directory-exists
fallback: that would test the projection while weakening the definition of
done.

## 2. Start one real Job

```bash
"$holdfast_deadreckon" start 'Create app.txt containing exactly "holdfast works", create dist/result.txt containing exactly "ship me", create .made-up-runtime-z91/cache.lock as disposable runtime state, leave that ignored runtime file present in the working tree, and create a project .gitignore containing /.made-up-runtime-z91/. Declare done only after checking all four files.' --mode run --from "$holdfast_project" --yes --plain
```

Copy the printed Job ID, then attach:

```bash
"$holdfast_deadreckon" attach <job-id>
"$holdfast_deadreckon" status <job-id>
```

Expected: the Job reaches a verified/reviewable terminal result without
`retry_exhausted`. If your configured independent judge is unavailable,
`needs_review` is acceptable; a repeated staging failure is not.

Confirm that this newly admitted Job opted into Holdfast before inspecting its
candidate:

```bash
find "$HOME/.deadreckon" -path "*<job-id>*/result-projection-activation.json" -print
```

Expected: exactly one controller-owned activation record is printed for this
Job. Its absence means the Job was admitted by an older binary and is exercising
historical compatibility rather than the new result boundary.

## 3. Inspect the sealed result

Locate the manifest without changing it:

```bash
find "$HOME/.deadreckon" -path "*<job-id>*/result-projection/manifest.json" -print
```

Let `<projection-dir>` be its parent, then run:

```bash
test -f <projection-dir>/candidate/app.txt
test -f <projection-dir>/candidate/dist/result.txt
test ! -e <projection-dir>/candidate/.made-up-runtime-z91
jq '{tree_sha256,included_files,included_bytes,omissions}' <projection-dir>/manifest.json
```

Expected: all three `test` commands exit 0. The manifest reports the unknown
runtime tree as an omission and records a non-empty tree digest.

Also inspect `"$holdfast_deadreckon" status <job-id> --json` and
`"$holdfast_deadreckon" show <job-id> --json`. Both views must report the same
candidate tree digest, projection digest and omission count.

## 4. Check that verification did not rewrite the candidate

```bash
test ! -e <projection-dir>/evaluation
git -C <projection-dir>/candidate status --porcelain 2>/dev/null || true
```

Expected: the disposable evaluation directory is absent. The candidate is a
controller-owned file tree, not a trusted Git repository; the second command
may print Git's ordinary “not a repository” message and is informational only.

## 5. Finish and inspect what ships

```bash
"$holdfast_deadreckon" finish <job-id> --dry-run
"$holdfast_deadreckon" finish <job-id> --yes
"$holdfast_deadreckon" status <job-id>
```

Expected: the preview and finish validate the receipt. The promoted/library
result contains `app.txt` and `dist/result.txt`, does not contain
`.made-up-runtime-z91`, and status remains verified rather than regressing.

## 6. Prove that an ignore rule cannot hide required source

Create a second disposable baseline with no ignore file:

```bash
hidden_project="$(mktemp -d)/holdfast-hidden-source"
mkdir -p "$hidden_project"
cd "$hidden_project"
git init -q
git config user.email holdfast@example.invalid
git config user.name 'Holdfast operator test'
printf '%s\n' '# Hidden-source refusal fixture' > README.md
git add README.md
git commit -q -m 'operator baseline'
mkdir -p .deadreckon
cat > .deadreckon/acceptance.yaml <<'YAML'
name: holdfast cannot hide required source
checks:
  - kind: file_exists
    path: "{working_dir}/required-source.js"
  - kind: content_match
    path: "{working_dir}/required-source.js"
    pattern: "required"
YAML
git add -f .deadreckon/acceptance.yaml
git commit -q -m 'add hidden-source done contract'
"$holdfast_deadreckon" start 'Create required-source.js exporting the string "required", create .gitignore containing /required-source.js, and treat required-source.js as a required delivered source file. Declare done only if the delivered result contains that source file.' --mode run --from "$hidden_project" --yes --plain
```

Copy this second Job ID and inspect it with
`"$holdfast_deadreckon" attach <job-id>` and
`"$holdfast_deadreckon" status <job-id> --json`.

Expected: this Job must not become verified while `required-source.js` is
omitted. It may revise the ignore rule and then verify with the file included,
or stop `needs_review`; it must not produce a verified receipt for a candidate
that lacks the required source.

## Report back

Please report:

- the Job ID and final status;
- whether the Holdfast activation record existed;
- whether all three step 3 `test` commands exited 0;
- whether `evaluation` was absent;
- whether status and show reported the same candidate/projection identity;
- whether finish validated and omitted `.made-up-runtime-z91`; and
- whether the hidden-source Job either included `required-source.js` or refused
  verification; and
- the exact command output for any mismatch.
