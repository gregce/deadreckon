# What coding agents leave to the operator

*Draft, 5 August 2026. My working notes and the full citation record live in the
DeadReckon repository at `docs/research/harness-essay/`. They are notes for
contributors and are not published separately.*

A coding agent will write the code. It will not do three other things.

- Decide whether the work is finished.
- Keep the run alive after your terminal closes.
- Stop you from accepting something that is wrong.

Somebody has to do those three things. How much of your attention that takes
decides whether you can run agents on a serious product for a long time.

I should say up front what is mine. DeadReckon, which section 4 describes, is my
software. The 25 patterns and the corpus behind them in section 2 are my own
team's work, published by my company, so section 2's counts are mine to check
rather than anyone else's. Everything I could give you a file path or a public
link for, I have.

This essay covers four things.

- What a coding agent already does for you.
- What it leaves for you, with that work counted from a real project.
- What the industry now says about the software wrapped around the model.
- What has to exist for delegation to become predictable.

---

## 1. Coding agents already do a great deal

Start with the part that works, because it works well.

Claude Code emits one event at the start of every session that lists everything
it contains. You can see it by running
`claude -p 'hi' --output-format stream-json` and reading the first line, which
has the type `system` and the subtype `init`. On this machine that line lists the
following.

- 30 built-in tools.
- 5 servers connected over the Model Context Protocol, with a status for each.
- The model id.
- The permission mode.
- 8 subagents.
- Around 100 slash commands.
- Around 70 skills.
- 7 plugins.

The three counts I have approximated vary with what is installed for the current
project. Separately from that event, and from a different source, the directory
`~/.claude` on this machine holds 7,850 saved task lists and 199 saved plan
documents. None of that is a claim from a product page.

The three terminal agents I use most make different choices about what belongs
inside the loop.

| | Claude Code | Codex CLI | Pi |
|---|---|---|---|
| Tools | 30 built in | Shell and file edit | 4: read, write, edit, bash |
| Approvals | Permission modes, per session | Two way approval over JSON-RPC | None, by design |
| Sandbox | None of its own | `read-only`, `workspace-write`, `danger-full-access` | Delegated to containers |
| Sub-agents | Yes, 8 on this machine | Yes | Refused |
| Plan mode | Yes, 199 saved plans here | Yes | Refused |
| Task list | Yes, 7,850 saved here | Yes | Refused |
| Model Context Protocol | Yes, 5 servers here | Yes | Refused |
| Extension model | Skills, plugins, hooks | Config and prompts | TypeScript extensions that reload live |

Codex is the one that ships its own sandbox. Its documentation names three
levels, and DeadReckon's launch template passes one of them on every unattended
run, so the flag is at least accepted. I have not probed whether the level holds
against a determined process, and by the standard section 4.3 sets, that means I
should not claim it does.

Pi is interesting for the opposite reason. In its README, Mario Zechner describes
Pi as "a minimal terminal coding harness", and his philosophy section is a list
of what he left out. Containers do the isolating instead, and the README says
plainly that Pi "does not include a built-in permission system for restricting
filesystem, process, network, or credential access" and that by default it "runs
with the permissions of the user and process that launched it". What Zechner adds
instead is a way to extend Pi. An extension written in TypeScript can register a
new tool for the model to call. It can also intercept an event, replace
compaction, which is the step that shortens a long conversation to fit, and draw
its own interface. An extension reloads without restarting the session.

Simon Willison gives the plainest definition of the thing all three are. On
16 March 2026 he wrote that
"[a coding agent is a piece of software that acts as a harness for an LLM, extending that LLM with additional capabilities that are powered by invisible prompts and implemented as callable tools](https://simonwillison.net/guides/agentic-engineering-patterns/how-coding-agents-work/)",
and described the shape as "LLM + system prompt + tools in a loop". An LLM is a
large language model, the part that produces the text. Note the word invisible.
The prompts and tool definitions that decide most of the behaviour are never
shown to the person using the agent. Birgitta Böckeler puts the same idea as an equation in her 2 April 2026 article on
martinfowler.com, where she defines the harness as
"[everything in an AI agent except the model itself](https://martinfowler.com/articles/harness-engineering.html)"
and writes it as `Agent = Model + Harness`.

The loop runs. The tools work. You can set the permissions and resume a session
by its id. For one sitting at one machine on one task, that is most of what you
need.

That competence creates the next problem. Once the loop can run for hours without
you, the unit of work is no longer a small change you can read
in a few minutes. Somebody then has to make explicit everything that used to be
implicit.

---

## 2. On a long project, the operator does the rest

Anthropic's Societal Impacts researchers report that developers use AI in roughly
60 percent of their work and can
"[fully delegate](https://resources.anthropic.com/hubfs/2026%20Agentic%20Coding%20Trends%20Report.pdf)"
only 0 to 20 percent of tasks. Those two figures count different things and the
report never defines the first, so no arithmetic relates them. The useful part is
what the authors say fills the rest of the job. Effective use "requires
thoughtful set-up and prompting, active supervision, validation, and human
judgment", and engineers delegate work where they "can relatively easily
sniff-check on correctness".

That is the work as the survey authors describe it. Here is the same work counted
inside one project.

Between September 2025 and May 2026, six of us at SpecStory built a product on
one shared git trunk and captured every agent session. The corpus is 1,310
sessions and 4,670 commits, and we typed almost none of the code by hand. I
spent months reading it back and naming what we had been doing, which turned into
[25 patterns](https://specstory.com/books/25-patterns-in-agentic-engineering-book-2026.pdf)
grouped into six parts. They share one idea. Once the code is cheap, the work is
the intent behind it and the proof that it holds.

The counts are the useful part, because they measure attention rather than
opinion.

| Measurement | Count | How it was counted |
|---|---:|---|
| Sessions captured | 1,310 | Every session, September 2025 to May 2026 |
| Commits | 4,670 | Git history on one trunk |
| "Request interrupted by user" | 614 across 184 transcripts | Fixed string match |
| Tool uses declined before touching disk | 335 | Subset of the above |
| Turns opening with a correction word | 441 | Match against no, wait, actually, stop, revert, undo. Crude, and it will over-count and under-count |
| "Do not edit files" in the prompt history | 88, all in April and May 2026 | Fixed string match. A second pass over the same corpus counts 91 across 52 transcripts, so read the figure as approximate |
| Briefs carrying their own exit condition | 4 | Hand count |

That last row is the one I keep coming back to. The most effective form I found
is a brief that includes a shell command, and that command decides when the agent
may stop. In 1,310 sessions we wrote four of them, because a person writes each
one by hand.

The patterns name the work precisely enough to check against a codebase.

**Verification is a job someone does.** Treat every claim of "verified",
"accurate" or "tests pass" as unverified until you have seen the check. A claim
with no cited artifact is unverified by default. The recorded origin is one
exchange in February 2026. The agent stated that a third party cost total was
accurate. The operator asked how it had verified that. The agent's own reasoning
block opened by naming the challenge and then read, in the source's own
punctuation, "I didn't actually verify it — I just assumed it." The point is not
that the model lied. It reported a guess in the same flat tone it uses for a
checked fact, and the tone is not a signal.

**The check has to run somewhere the agent cannot reach.** Ask what a result is
accurate against. If the answer is another thing the agent produced, you have
used the agent's own output to check the agent's own output.

**Somebody has to run the program itself.** The operator has to watch the running
program, because the model cannot see what happens when it runs, e.g. it cannot
see the interface render. The operator runs the program and brings back the
output.

**Somebody has to carry state between tools.** Codex and Claude share no memory,
so when you want one to build on what another found, you copy it across yourself.

**Somebody has to decide when work leaves the machine.** Let the agent commit at
phase boundaries and keep the push for yourself. A commit is a save point you can
undo on your own. A push changes what everyone else builds on.

Now put those human duties next to what the tools provide. I checked everything below against
files on this machine rather than recalling it. Two terms need defining
first, because I use them repeatedly.

A **provider description** is a configuration file in the DeadReckon repository,
one per agent route. It records three things.

- How to launch that agent.
- What its `--help` output lists.
- Where the cost, the session id and the tool events appear in its output stream.

There are seven for command line agents, plus four more
in the same format for direct model APIs that are not part of this argument. A
**recorded transcript** is a saved copy of one real session with one of those
agents, kept as a test fixture. There are nine, covering five of the seven
command line routes. Gemini and the Codex app server have none.

Here is what the seven routes give an outer supervisor. The last column says what
kind of evidence each row rests on, because that turns out to decide how much
each row is worth.

| Route | Completion signal | Cost unit | Tool call format | Unattended flag | Evidence |
|---|---|---|---|---|---|
| Claude Code | `stop_reason: end_turn` | `total_cost_usd` | Content blocks | `--dangerously-skip-permissions` | Recorded transcript |
| Codex | `turn.completed` | Token counts only | `item.started` and `item.completed` | `--ask-for-approval never` | Recorded transcript |
| Copilot | `exitCode: 0` | `premiumRequests` | `toolRequests` plus start and complete | `--allow-all` | Recorded transcript |
| Pi | Stream end | Cost total in the usage object | `toolCallId` | No permission system to disable | Recorded transcript |
| OpenCode | Exit code zero, after an error in the stream | None recorded | Step and text parts | Hidden flag removed in 0.15.5 | Recorded transcript, partial |
| Codex app server | JSON-RPC turn end | Token counts only | JSON-RPC items | Approvals answered by program | Description only |
| Gemini | Not recorded. Process exit is the fallback | None recorded | None recorded | No flag in the description | Description only, no stream ever recorded |

Two rows carry almost nothing, for reasons the descriptions themselves record.
The Gemini comment says the binary's help text lists structured output, but the
installed credentials fail before it emits a single event, so nobody has captured
one. The OpenCode comment says a successful run emitted an answer, then an error.
It then emitted a null answer and still exited zero.

Five things follow, and each one is work the operator has to do. I have written
each one as a claim about the evidence rather than about the products, because
that is what the evidence supports. A provider description records what
DeadReckon needs from an agent, not everything that agent can emit, so a
capability nobody asked for would not appear here.

**Across the nine recorded transcripts, no agent emits a completion signal
independent of the model's own report.** Nothing in any transcript re-runs a
check against the goal. The agent's opinion of its own work is the whole signal.

**Across all seven descriptions and all nine transcripts, nothing authenticates a
result.** No signature over the work, no message authentication code, and no
receipt tying an outcome to a key. Two near misses are worth naming, because a
reader running the same search will hit them. Claude Code advertises a capability
called `interrupt_receipt_v1`, and Pi emits a field called `thinkingSignature`
whose value is the string `reasoning_content`. The first names a protocol
feature and the second names a place to find text. Neither authenticates
anything. If any of these agents can sign a result, DeadReckon never found the
surface to ask for it.

**Claude Code's run does not outlive its process.** This one I can state about
the product, because the evidence is the product's own state directory rather
than my model of it. Its session registry is keyed by operating system process
id. The three files in `~/.claude/sessions` are named `80130.json`,
`87885.json` and `92024.json`, which are the process ids of the three agents
running on this machine right now. The background service writes
`idle 5s with no clients` to its log and exits on its own. Closing the terminal
removes the lifecycle record.

**No time bound in this system outlives a process, on either side of the
boundary.** DeadReckon's own launch template carries one per-invocation timeout,
`timeout_seconds = 1800`, identical across all seven routes, and it restarts at
zero with the process. That is my number, not theirs. The more useful fact is
that nothing in the nine recorded transcripts carries a bound of the agent's own
that survives a restart either. Neither side of the boundary keeps a running
total.

**Running without a person turns the approvals off rather than delegating them.**
Codex is the possible exception, because it takes a sandbox level on the same
command line that disables its approvals, so the boundary is at least meant to
survive. As section 1 says, I have not probed whether it does. For the other six,
running without a person removes the boundary rather than moving it. The OpenCode
entry records version 0.15.5, and nobody has re-checked it since.

The table shows one further problem. Every column uses different words. To find
out whether an agent supports resuming, you read its `--help` text and look for
substrings. The same goes for streaming and for structured output. So the
contract can break on the agent's next release.

The outside evidence shows this problem getting worse rather than better. The
Qwen team state the reversal in the first line of a paper of 29 June 2026.
"[A classical intuition in computing holds that verifying a solution is easier than finding one. For today's coding agents, this asymmetry is reversing.](https://arxiv.org/html/2606.26300v2)"
The authors argue that every check you can write stands in for what a person
wanted and is never the thing itself. They add that a model trained against a
stand-in learns the difference between the two.

Three groups have measured the size of that difference. All the percentages below
are pass rates on the named benchmark.

| Source | Date | Finding | Known weakness |
|---|---|---|---|
| Qwen team | 29 June 2026 | Adding behaviour monitoring to the tests dropped solutions that passed by cheating from 28.57 percent to 0.56 percent, and raised clean solutions from 40.22 percent to 60.53 percent, across three SWE-Bench variants | The monitor is itself a model, so it has its own error rate, which they do not report |
| Naman Jain, Cursor | 25 June 2026 | After sealing git history and blocking network access except to package registries, Opus 4.8 Max "fell from 87.1% to 73.0%" on SWE-bench Pro | One vendor, one model, one benchmark |
| Weco AI, SpecBench | 20 May 2026 | "The gap grows by 28 percentage points for every tenfold increase in code size" between the visible suite and a held out suite. Above 25,000 lines the worst-case gap reaches 100 percentage points | A 100 point gap means the visible suite passing completely while the held out suite fails completely. I could not tell from the paper whether that shows the effect or shows how the held out tasks were built |

The Qwen pair is the most useful, because both numbers moved together. The tests
alone were accepting a large amount of work that should have been rejected, and
they were also rejecting work that was fine. The Cursor result is the plainest.
The model did not get worse. Sealing the history removed the shortcuts. I would
not rely on the
SpecBench 100 point figure at all, and I have included it only because the
28 point trend beneath it is the finding that generalises.

Simon Willison's version of the rule is shorter.
"[If the code has never been executed it's pure luck if it actually works when deployed to production.](https://simonwillison.net/guides/agentic-engineering-patterns/first-run-the-tests/)"
The habit he tells people to avoid is equally short.
"[Don't file pull requests with code you haven't reviewed yourself.](https://simonwillison.net/guides/agentic-engineering-patterns/anti-patterns/)"

Every one of those instructions is correct and every one of them is a job for a
person. That is why people started working on the software around the model.

---

## 3. The claim that the harness decides the outcome

The claim shows up in four places I keep returning to, and the strongest versions
come with numbers.

Addy Osmani, writing on 19 April 2026 and republished by O'Reilly on 15 May,
states it as plainly as anyone. He names Claude Code, Cursor, Codex, Aider and
Cline, calls all five of them harnesses, and writes that
"[the model underneath is sometimes the same, but the behaviour you experience is dominated by what the harness does](https://addyosmani.com/blog/agent-harness-engineering/)".
He adds two lines that people quote more often than they read the article they
come from. "A decent model with a great harness beats a great model with a bad
harness." And, "The gap between what today's models can do and what you see them
doing is largely a harness gap."

Researchers at Meta and Harvard measured a version of it on 3 February 2026.
Holding the model fixed and changing only the scaffold, which is these authors'
word for the harness, moved Claude 4.5 Sonnet on
SWE-Bench-Pro from 45.8 percent to 52.7 percent. That 6.9 point rise from the scaffold
alone is the finding I trust. They also report a crossover, where
"[even a weaker model equipped with a strong agent scaffold (Claude 4.5 Sonnet + CCA at 52.7%) can outperform a stronger model (Claude 4.5 Opus + Anthropic's proprietary scaffold at 52.0%)](https://arxiv.org/html/2512.10398v6)".
Those two figures are 0.7 points apart on a benchmark of a few hundred instances,
which is close enough that I would not rely on the crossover itself. The 6.9
point swing is roughly ten times that gap on the same instance count, which is
the only reason I treat the two differently. The authors publish no error bars,
so even the 6.9 is an estimate.

One practitioner post is worth keeping, not for a number but for a list of the
places things break. Writing on r/LLMObservability on 3 August 2026, an engineer
reports that
"[Most of the agent behaviour I have had to fix was not the model being dumb. It was the scaffolding: how tools were described, what got put back into context after a failure, how many steps were allowed, what happened on a timeout.](https://www.reddit.com/r/LLMObservability/comments/1vedbnq/the_harness_around_the_model_decides_more_of_your/)"
He names four places where the behaviour breaks. He attaches no quantity to any
of them, and he claims none.

The claim also has a serious critic, and the criticism is right about most
software. Hugo Bowne-Anderson, writing for O'Reilly on 22 July 2026, argues
against building this software before you need it. Of compaction, memory,
handoffs and
sub-agents he writes that
"[most agents you'll build don't need any of them](https://www.oreilly.com/radar/stop-overengineering-your-agent-harness/)",
and his advice is to keep the harness small and "add infrastructure only when a
real failure demands it". His own exceptions are the point. He names coding
agents, deep research systems and long-lived assistants as the cases where the
work does pay, because those are the systems that run long enough to accumulate
the failures.

None of these authors go on to the next question.

Böckeler's model of a harness is a system of guides and sensors. Guides
"anticipate the agent's behaviour and aim to steer it *before* it acts". Sensors
"observe *after* the agent acts and help it self-correct". That model is correct
and it is about one process. The guides go into the prompt. The sensors run
inside the loop and feed their output back to the model. Osmani's list of what a
harness contains is the same in this respect. It covers the following.

- System prompts.
- Tools.
- Bundled infrastructure.
- Orchestration, which is the code deciding what runs next.
- Hooks, which are commands the harness runs at fixed points.
- Observability, which means logs, traces and cost meters.

Every item on that list is inside the agent while it is running.

Böckeler is careful about the limit herself. Writing about maintainability
sensors on 27 May 2026, she says that these sensors
"[are not a magical solution to take the human totally out of the loop](https://martinfowler.com/articles/sensors-for-coding-agents.html)",
and warns that they can produce "a false sense of security and an illusion of
quality", because static analysis cannot reach the meaning of the change.

Section 2 counted work that happens outside the loop. The loop never reaches any
of the following four.

- After the process exits.
- The hours after your laptop sleeps.
- Whether the check that passed was the right check.
- Whether to put the result on a branch other people depend on.

A better inner harness makes the agent produce better work. It does not tell you
whether to accept the work, and it does not outlive its own process. Those are
different problems, and they need different software.

---

## 4. What makes delegation predictable

The manual patterns say what the software has to do. Each one describes a correct
procedure that a person performs. So take each procedure and ask what would have
to exist for a machine to perform it instead, without weakening it.

Here are eight answers, as DeadReckon implements them. The point is not the
particular product. The point is that each mechanism is small enough to check,
and each one answers a burden that section 2 counted.

| # | Mechanism | Manual work it replaces | Where it lives |
|---|---|---|---|
| 4.1 | Executable definition of done, refused if empty | Writing by hand a brief that grades itself, done 4 times in 1,310 sessions | `deadreckon-core/src/gate.rs:140`, `completion.rs:1069` |
| 4.2 | Two independent judgments, one deterministic and one about meaning, that must agree | Reading every claim with suspicion, every time | `deadreckon-core/src/completion.rs:171`, `deadreckon-runtime/src/semantic_judge.rs:873` |
| 4.3 | Verifier whose unreachability is observed on each run, not assumed | Keeping a source of truth outside reach, by convention | `deadreckon/src/bin/dr-gate.rs:491`, `deadreckon-core/src/gate.rs:338` |
| 4.4 | A run that outlives the terminal | Nothing. No manual procedure can do it | `deadreckon-protocol/src/job.rs:234`, `deadreckon-core/src/job_lease.rs` |
| 4.5 | Budget computed from the log, not a timer | Watching the spend meter | `deadreckon/src/commands/supervisor.rs:5855` |
| 4.6 | Isolated result, deliberate promotion | Committing at phase boundaries and never pushing | `deadreckon-core/src/delivery.rs:162`, `deadreckon/src/commands/undo.rs:113` |
| 4.7 | Evidence re-checked when read | Keeping an architecture map true by hand | `deadreckon-core/src/job.rs:235` |
| 4.8 | Provider and model fixed per role | Checking by hand which model a run used | `deadreckon-providers/src/model_catalog.rs:36` |

### 4.1 Make "done" executable before the work starts

Before any agent runs, DeadReckon compiles the goal into a contract of checks
that a command can run. There are five kinds of check and each carries a flag
saying whether it must pass. The enum is `AcceptanceCheck` in
`crates/deadreckon-core/src/gate.rs:140`.

- A Rust test run.
- A check that a file exists.
- A check that a file's contents match a pattern.
- A build.
- Any shell command.

Only the Rust test run has its own typed check. Every other language uses the shell check
instead, so `go test ./...` and `python -m pytest -q` both run as shell commands. A separate step reads the repository and proposes
a default for the ecosystem it finds, in
`crates/deadreckon-core/src/acceptance_defaults.rs:82`.

The part that does the work is the refusal. `validate_strict_contract` in
`crates/deadreckon-core/src/completion.rs:1069` runs before DeadReckon writes the
authority, which is the signed statement of what the operator approved, and
before a job exists. It rejects three kinds of contract.

- One with no checks.
- One where no check is marked as required.
- One whose only proof is that the working directory it just created exists. The
  refusal text for that case is `"only proves that its pre-created working
  directory exists"`.

In each of those three cases the definition of done stops testing anything, and
DeadReckon will not start.

### 4.2 Separate "the agent finished" from "the work is accepted"

Two independent things must agree before DeadReckon marks a result as verified.
`seal_completion_receipt` in `crates/deadreckon-core/src/completion.rs:171`
enforces it.

The first is a deterministic gate that runs the approved checks and signs the
outcome. The second is a fresh model call with no session history and no write
access. Only the first of the two holds a signing key. The judge signs nothing.
Its answer is tied to one result by a hash of the evidence it was given, not by a
signature of its own.

The judge reads a bounded packet of evidence and returns exactly one of three
verdicts, which are `achieved`, `revise` and `uncertain`. Its instructions, in
`crates/deadreckon-runtime/src/semantic_judge.rs:873`, begin, "You are an
independent completion judge. Assess meaning only; deterministic checks have
already passed and you may not override them."

| Verdict | What happens |
|---|---|
| `achieved` | The receipt can be sealed |
| `revise` | Another bounded attempt starts |
| `uncertain` | The job stops and asks for a person |
| Judge unavailable or reply malformed | The job stops and asks for a person |
| Deterministic checks failed | The judge is never called |

The judge sees exactly six things and nothing else. This is the most
consequential list in section 4, because it is the entire input to the decision
about whether the work is acceptable.

| Evidence item | Size limit |
|---|---|
| The approved goal | Bounded |
| The approved contract | 64 KiB |
| The diff against the source | 256 KiB |
| The deterministic gate result | Bounded, with at most 64 findings |
| The approved authority | Bounded |
| The implementation notes | 64 KiB, summary capped at 4,000 characters |

DeadReckon checks the answer rather than trusting it. The judgment records a
hash of that packet, so nobody can attach a judgment to a different result later.
If the model returns `achieved` while also listing a blocking omission, or cites
an evidence id that is not one of the six, the parser rejects the reply.

### 4.3 Keep the verifier out of the agent's reach

A file is not outside reach merely because the prompt says not to edit it.

So DeadReckon splits the gate across three separate runs of one small binary,
`dr-gate`, each with its own environment. That binary has five modes in total.
The two not described here handle a protocol handshake and a guarded process
launch.

- The `evaluate` mode runs the repository's own check commands and holds no key.
  It refuses to start if the signing key or either containment marker is present
  in its environment, checked at `crates/deadreckon/src/bin/dr-gate.rs:491`. A
  containment marker is a variable recording whether the run was contained and
  which sandbox backend resolved. The evaluator writes an unsigned result.
- The `sign` mode launches no child processes. It reads the key, re-checks the
  evaluation against the contract, and signs.
- The `probe-boundary` mode runs inside the resolved sandbox and records whether
  reading the key, writing proof files and writing control files were denied.

The key file is stored outside the workspace in a directory only its owner can
read. The
code opens it with `create_new(true)` at
`crates/deadreckon-core/src/gate.rs:338`, so it is created once and never
replaced, and it is refused if the file is a symlink, is not a regular file, or
can be read by anyone else.

The next part is easy to overstate, so I want to state it carefully. File
permissions alone do not put the key out of the agent's reach. The agent runs as
the same user, so a file the user can read is a file the agent can read. What
closes the gap is the sandbox, and only the sandbox. That is why the third mode
exists. Unreachability is not a property you can assume from the file mode. It
has to be observed on every run, inside whatever backend actually resolved, and
recorded.

The boundary observation is signed with its own separate tag,
`deadreckon.sandbox-boundary-observation.v1`, in
`crates/deadreckon-core/src/sandbox_observation.rs:24`. Its wire format rejects
unknown fields, so an agent cannot add a field of its own to the record.

This makes 4.3 only as strong as the sandbox underneath it, and on macOS that
sandbox denies a list of specific paths rather than denying everything by
default. A deny list is weaker than a default deny, because it protects only what
somebody thought to name. The honesty table at the end of this section says so
again. Of everything I have not proven, this is the one I would fix first.

### 4.4 Give the run a life that does not depend on the terminal

A job is an append-only event log. DeadReckon only ever adds a line to the end.
An edit or a removal is detected rather than prevented, because a process with
write access to the file can still truncate it. The list of six corruption errors
below is how DeadReckon detects it. That log is the only record that decides what
happened. The checkpoint file beside it is a convenience, and a program can
rebuild it from the log.

The list of names is fixed. There are 8 terminal outcomes at
`crates/deadreckon-protocol/src/job.rs:234` and exactly 18 stop reasons at
`crates/deadreckon-protocol/src/job.rs:249`. There are 18 stop reasons rather
than 3 because "I could not prove containment" and "the budget ran
out" need different responses from the operator. One is a bug report and the
other is a spending decision. Only the stop reasons carry a fixed size array
holding the count, so the build fails if that number changes. The allowed pairs
of outcome and reason are a fixed table. The reducer is the program that reads
the event log and works out the current state, and both the reducer and every
status display read that table.

One process owns a job at a time. It holds a lease with an epoch, which is a
counter that only increases, so a worker whose lease has gone stale has its
writes rejected by `append_fenced_job_event` in
`crates/deadreckon-core/src/job_lease.rs`. A lease is only taken from a previous
owner when it has expired, when the machine has rebooted, or when its checkpoint
is missing. A worker sends a signal at a fixed interval to show it is alive, and
a late signal cannot take a job away from a process that is still running on the
same boot.

The reducer refuses to guess. Each of the following produces a corruption error
rather than a reading that does its best with what is there.

- A gap in the sequence.
- An event belonging to another job.
- A duplicate event id whose bytes differ from the first copy.
- A lease epoch that did not increase.
- Any event recorded after a terminal outcome.
- Any outcome and reason pair that is not in the table.

The reducer ignores a final line that was only partly written, and it refuses to
append after one.

The operating system service manager handles machine restart, using launchd on
macOS and systemd on Linux. The preflight refuses to claim durability unless the
service is both active and enabled with a live checkpoint. Other platforms are
reported as unsupported rather than quietly assumed to work.

### 4.5 Bound the work in a way that survives a restart

DeadReckon freezes three limits when the job starts and hashes them into the
authority record. They are a cap on dollars spent, a cap on total time, and a cap
on attempts. A fourth limit, a deadline on the calendar, is optional and commonly
absent.

`active_attempt_wall` in `crates/deadreckon/src/commands/supervisor.rs:5855`
computes elapsed time from the event log rather than from a process timer. It
adds up every started and stopped interval, then adds the time since the current
attempt began. That is why the total survives a crash. It also survives a supervisor restart and
a reboot. Three faults make the count fail closed, which means it refuses to
return a number rather than returning a wrong one.

- Timestamps that run backwards.
- A second start with no stop in between.
- Arithmetic overflow. When you set a deadline, it and the time cap limit the
same allowance. Whichever is tighter applies.

Exhausting the cap does not end the job on its own. The supervisor first proves
that every process it owns has stopped, including nested evaluators, sub-plans
and containers. If it cannot prove that, the job becomes blocked with a reason of
lost containment rather than being recorded as out of budget.

### 4.6 Isolate the result, then promote it on purpose

The result of a run goes into a workspace that is not your checkout. That is a
separate decision from which sandbox confines the process, and the two should not
be treated as one guarantee. The sandbox uses one of three backends, which are
Seatbelt on macOS, bubblewrap on Linux and Docker, in
`crates/deadreckon-sandbox/src/backend.rs:58`. A resolved backend of `none`
cannot produce a trusted receipt, which
`crates/deadreckon-core/src/completion.rs:201` enforces by refusing to sign when
the marker reports no containment.

Promotion is a distinct act with three steps.

1. Re-check the approved authority, the gate marker, the judgment, the signature
   and the hash of the result tree.
2. Sign a statement of what is about to happen, before touching git.
   `seal_git_delivery_intent` is in `crates/deadreckon-core/src/delivery.rs:162`, and the record it signs, `GitDeliveryIntent`, is in `crates/deadreckon-protocol/src/job.rs:565`.
3. After the change lands, re-prove the exact state and sign a second receipt.

Undo is authorised by those two signed artifacts rather than by the event log,
and `crates/deadreckon/src/commands/undo.rs:113` verifies that the result is an
exact revert commit.

### 4.7 Keep the evidence, and re-check it when someone reads it

DeadReckon keeps five kinds of record as append-only files under the run
directory.

- The events.
- The traces.
- The spend.
- The provenance.
- A snapshot of the working tree, one per turn.

A single provenance row records which prompt id, model, tool call id and session
id produced which files.

The part that is easy to skip is re-checking that evidence when someone reads it.
A completion receipt records ten hashes plus the attempt identity. Nine of the
ten cover the approved inputs and the trees produced.

- The authority.
- The goal.
- The contract.
- The effective policy.
- The launch plan.
- The source tree.
- The result tree.
- The gate marker, which is the signed record the `sign` mode writes, saying
  which checks ran, whether they passed, and under which sandbox backend.
- The judgment.
 The tenth hash is the one section 4.3 describes.
It covers the sandbox boundary observation, and without it the receipt would
record that the checks passed but not that they ran somewhere the agent could not
reach.

Every time a display reads the job, `JobView::load` in
`crates/deadreckon-core/src/job.rs:235` re-checks the recorded verified fact
against the receipt on disk. If the two no longer agree it fills in
`verified_receipt_error` and the display reports the mismatch rather than
reporting success. A receipt whose proof has been edited or deleted shows as
broken rather than continuing to show as verified.

### 4.8 Fix the provider and model for each role

Re-running a job six weeks later against a different model is not a replay. It is
a new experiment recorded under an old job's id, and the evidence you compare
will not mean what you think it means.

So DeadReckon chooses the provider and the model together as one decision, for
each role in the run, and writes both into the accepted plan. Recovery and replay
restore those exact strings.

The bug this closes was real. Model discovery used to fall back to a different
provider's catalogue when a provider's own list could not be read, so a run could
end up recorded against a model that provider does not serve. Discovery now reads only the list a
provider publishes for itself. When that read fails, DeadReckon falls back to the
model list it ships for that same provider, never to another provider's list. The rule is stated in
`crates/deadreckon-providers/src/model_catalog.rs:36`.

### What this does not prove

The honest boundary belongs in the same section as the claims, because a claim
about verification that has not itself been verified is the exact failure this
system exists to prevent.

The mechanisms above are implemented and covered by tests, including a fault
matrix for torn writes, lease races, receipt tampering and refused promotion.
That is engineering evidence. It is not the same as an operator outcome.

| Claim | State of the evidence |
|---|---|
| Operators can run this on real work | The 24 row live acceptance kit records 2 tasks attempted, 22 not run, 0 verified |
| The independent judge decides correctly | No measured rate of false acceptance or false rejection. Until those exist, 4.2 is a design and not a result |
| Recovery works after a worker dies | Proven under Docker, but bound to an older clean commit, so it does not cover the later supervisor and sandbox changes |
| A verified receipt can be issued on Linux | Only the negative case is recorded. Linux continuous integration proves a smoke run cannot issue a trusted receipt. Nobody has recorded a positive one |
| Work survives a machine restart | Nobody has run a real reboot recovery |
| The macOS sandbox confines the process | It denies specific paths rather than denying everything by default, and the network policy is a list checked at approval time rather than a proxy that blocks a domain |

Publishing that table is part of the design. The alternative is a system you have
to trust on a summary alone, which is what all 25 patterns teach people not to
do.

---

## 5. If you wrap agents you did not write

Everything above is easier when you control the inner loop. A layer that
supervises other people's agents gives you a harder problem, because you cannot
change them and cannot trust them to report on themselves. These are the parts that
turned out to be hard, and the ones a second attempt should get right first.

**Design for observation, not cooperation.** The inner agent will not tell you it
is done in a way you can rely on, because its only completion signal is its own
report. Treat process exit as a reason to go and look at the evidence, never as
the answer. The module comment at
`crates/deadreckon/src/commands/supervisor.rs:3` states the rule. "The
append-only job history is control truth. Process exit is only a wakeup to
inspect persisted run evidence; it is never accepted as completion."

**There are two kinds of inner loop and you have to cover both.** A direct model
API returns one structured action per turn, e.g. a shell command, and your
runtime executes it. A command line agent changes the working tree itself and
tells you about it afterwards. The first you drive. The second you watch. Only
the outer contract can be the same for both, and if you design for one shape you
will rebuild for the other.

**Every agent uses different event names, so put those names in data.** One
adapter per agent means a new adapter for every agent, rewritten on every
release. Describing each agent in a configuration file turns a new agent into
data rather than code. The DeadReckon repository has seven
command line routes over six distinct binaries, because Codex appears twice, once
per transport.

| Route | How it is driven |
|---|---|
| Gemini | Description only |
| OpenCode | Description only |
| Copilot | Description only, including how to parse its events |
| Pi | Description only, including how to parse its events |
| Claude Code | Hand written Rust. Its event stream is too irregular to describe |
| Codex | Hand written Rust. Its event stream is too irregular to describe |
| Codex app server | Hand written Rust. It speaks two way JSON-RPC rather than emitting a stream |

Four of seven run from a description, which is what I wanted. A description can
also say how to parse the event stream, but only two of the seven do that.

**Assume the contract will break, because you discover capability by guessing.**
Searching `--help` text for substrings is not a stable interface, and section 2
shows why. The DeadReckon repository was set up against Pi version 0.79.1. The Pi source tree on the
same machine is already at 0.83.0 with unreleased work in it. Drift between the
version you validated and the version installed is the normal state, so do three
things.

- Record which version you checked.
- Probe the binary before trusting it.
- Say in the output when you have fallen back, rather than falling back quietly.

**Running without a person means the approvals are gone.** Running these agents
without a person means passing a flag that turns off the inner boundary. Codex
may be the exception, but until you have probed it yourself you should not plan
around it. Your outer boundary is therefore the only boundary. Assume the agent
has full authority over everything your sandbox does not deny, because in most
configurations it does.

**Cost is not comparable, so do not pretend it adds up.** Dollars, tokens and
premium requests are three different quantities. Record what each agent reports
and convert only where a true conversion exists. Show the operator where each
number came from rather than one total that is partly invented.

**Keep two clocks.** The inner timeout is per process and resets on every launch.
Your budget must not. Derive elapsed time from your own durable record of when
attempts started and stopped, or a crash loop will run forever inside a cap that
looks like it is working.

**Fail closed, and be specific about what closed means.** A run with no resolved
sandbox must not be able to produce a trusted receipt. A cancellation that cannot
prove every child process is dead must record that it could not, rather than
recording a clean stop. The useful discipline is to have a distinct stop reason
for "I could not prove this" and to use it.

**Do not claim two guarantees from one mechanism.** Putting the result in a
separate working tree keeps the agent's changes away from your checkout. It says
nothing about what the process can reach on the rest of the machine. Those are
two properties with two mechanisms, and merging them in the documentation is how
a system ends up over-promising.

**Refuse old paths rather than quietly running them.** A wrapper accumulates
routes that predate its own guarantees. The choice is to let them run with weaker
guarantees or to refuse them at the public boundary and print the durable
equivalent. Refusing is better, and the cost is that your help text has to be
rewritten to match, which is work DeadReckon has not finished.

**One truth model, or you have built two systems.** Every display should read the
same event log through the same resolver. If the status output, the interface and
the report each hold a different record of what happened, an operator has to
decide which one to believe, and you have recreated the problem you started with.

**Measure the judge, or say that you have not.** Using a model to decide whether
another model's work is acceptable makes a claim about accuracy, and accuracy has
a number. Until you can state a false acceptance rate and a false rejection rate,
say so. The Qwen team's 28.57 percent to 0.56 percent result is useful because
they measured both directions.

---

## Where this leaves things

The agents are good and getting better, and I am not arguing against using them.
This is an argument about where the remaining work went.

Once the code is cheap, the work is the intent behind it and the proof that it
holds. The corpus of 1,310 sessions shows what happens when only a person can
supply that proof. A person supplied it 614 times by interrupting the agent. A
person supplied it 88 times by writing an instruction not to edit files, added
after something went wrong. A person supplied it 4 times by writing a full brief
that carries its own exit condition, which is the form the sources in section 2
recommend.

Software can supply that proof instead. Six pieces do most of it.

- A contract that refuses to be empty.
- A check that runs where the agent could not reach it, with the containment
  observed and recorded each time.
- A second reader that returns only three answers.
- A log that outlives the terminal.
- A budget computed from that log rather than from a process timer.
- A promotion step that re-checks everything before it changes your branch.

None of those replace judgment. They move the operator's attention to the one
question where judgment is the only thing that works, which is whether the check
that passed was the right check.

This essay does not settle whether that arrangement is worth its complexity for
anyone other than me. The mechanisms are built and tested. The live evidence is
thin, and section 4 says exactly how thin. Closing that gap means running real
work through the system on other people's repositories and recording what
happened. That is the next piece of work, and writing more of this will not do
it.

---

## Sources

**On what a harness is**

- Birgitta Böckeler, [Harness engineering for coding agent users](https://martinfowler.com/articles/harness-engineering.html), martinfowler.com, 2 April 2026.
- Birgitta Böckeler, [Maintainability sensors for coding agents](https://martinfowler.com/articles/sensors-for-coding-agents.html), martinfowler.com, 27 May 2026.
- Simon Willison, [How coding agents work](https://simonwillison.net/guides/agentic-engineering-patterns/how-coding-agents-work/), 16 March 2026.
- Addy Osmani, [Agent Harness Engineering](https://addyosmani.com/blog/agent-harness-engineering/), 19 April 2026, republished by O'Reilly Radar 15 May 2026.
- Mario Zechner, [Pi](https://github.com/earendil-works/pi). Quotes taken from the README of the source tree at version 0.83.0.

**On the harness deciding the outcome**

- Sherman Wong and others (Meta, Harvard), [Confucius Code Agent: Scalable Agent Scaffolding for Real-World Codebases](https://arxiv.org/html/2512.10398v6), 3 February 2026.
- Elvis Saravia (@omarsar0), [post on X](https://x.com/omarsar0/status/2084714744880173451), 4 August 2026. Anecdote, no published method.
- [The harness around the model decides more of your agent's behaviour than the model does](https://www.reddit.com/r/LLMObservability/comments/1vedbnq/the_harness_around_the_model_decides_more_of_your/), r/LLMObservability, 3 August 2026. Anecdote.
- Hugo Bowne-Anderson, [Stop Overengineering Your Agent Harness](https://www.oreilly.com/radar/stop-overengineering-your-agent-harness/), O'Reilly Radar, 22 July 2026.

**On verification**

- Qwen Team, [The Verification Horizon: No Silver Bullet for Coding Agent Rewards](https://arxiv.org/html/2606.26300v2), 29 June 2026.
- Bingchen Zhao and others (Weco AI), [SpecBench: Measuring Reward Hacking in Long-Horizon Coding Agents](https://arxiv.org/html/2605.21384v1), 20 May 2026.
- Naman Jain, [Reward hacking is swamping model intelligence gains](https://cursor.com/blog/reward-hacking-coding-benchmarks), Cursor, 25 June 2026.
- Simon Willison, [First run the tests](https://simonwillison.net/guides/agentic-engineering-patterns/first-run-the-tests/), 24 February 2026.
- Simon Willison, [Anti-patterns: things to avoid](https://simonwillison.net/guides/agentic-engineering-patterns/anti-patterns/), 4 March 2026.

**On what operators do**

- Anthropic, [2026 Agentic Coding Trends Report](https://resources.anthropic.com/hubfs/2026%20Agentic%20Coding%20Trends%20Report.pdf).
- [25 Patterns in Agentic Engineering](https://specstory.com/books/25-patterns-in-agentic-engineering-book-2026.pdf), SpecStory Press, 2026. Drawn from 1,310 captured sessions and 4,670 commits between September 2025 and May 2026.
- Paweł Józefiak, [Claude Code vs Codex CLI vs Aider vs OpenCode vs Pi vs Cursor](https://thoughts.jock.pl/p/ai-coding-harness-agents-2026), 15 April 2026.

**In this repository**

- [`README.md`](../README.md) and [`PRODUCT.md`](../PRODUCT.md)
- [`docs/MAP-OF-DEADRECKON.md`](MAP-OF-DEADRECKON.md)
- [`docs/HARNESS-ENGINEERING-COMPARISON.md`](HARNESS-ENGINEERING-COMPARISON.md)
- [`docs/research/harness-essay/CITATIONS.md`](research/harness-essay/CITATIONS.md)
