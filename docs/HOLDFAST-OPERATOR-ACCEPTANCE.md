# Holdfast operator acceptance

Run this after building and installing the candidate binary. It exercises the
real detached supervisor and a deliberately unknown runtime-output name.

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

## 2. Start one real Job

```bash
deadreckon start 'Create app.txt containing exactly "holdfast works", create dist/result.txt containing exactly "ship me", create .made-up-runtime-z91/cache.lock as disposable runtime state, and create a project .gitignore containing /.made-up-runtime-z91/. Declare done only after checking all four files.' --from "$holdfast_project" --yes
```

Copy the printed Job ID, then attach:

```bash
deadreckon attach <job-id>
deadreckon status <job-id>
```

Expected: the Job reaches a verified/reviewable terminal result without
`retry_exhausted`. If your configured independent judge is unavailable,
`needs_review` is acceptable; a repeated staging failure is not.

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
deadreckon finish <job-id> --dry-run
deadreckon finish <job-id>
deadreckon status <job-id>
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
deadreckon start 'Create required-source.js exporting the string "required", create .gitignore containing /required-source.js, and treat required-source.js as a required delivered source file. Declare done only if the delivered result contains that source file.' --from "$hidden_project" --yes
```

Copy this second Job ID and inspect it with `deadreckon attach <job-id>` and
`deadreckon status <job-id> --json`.

Expected: this Job must not become verified while `required-source.js` is
omitted. It may revise the ignore rule and then verify with the file included,
or stop `needs_review`; it must not produce a verified receipt for a candidate
that lacks the required source.

## Report back

Please report:

- the Job ID and final status;
- whether all three step 3 `test` commands exited 0;
- whether `evaluation` was absent;
- whether finish validated and omitted `.made-up-runtime-z91`; and
- whether the hidden-source Job either included `required-source.js` or refused
  verification; and
- the exact command output for any mismatch.
