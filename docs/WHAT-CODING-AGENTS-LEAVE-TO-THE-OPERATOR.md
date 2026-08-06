# What coding agents leave to the operator

Revised 5 August 2026. The research notes and source audit are in
[`docs/research/harness-essay/`](research/harness-essay/).

The important change is the size of the result we can delegate. One request can
now start a long sequence of repository reads, code changes, tests, tool calls,
and work by other agents. Claude Code and Codex can keep working toward a goal
across several turns. Both can isolate parallel work. Pi can expose its loop as
JSON or RPC so another program can drive it.

This progress has reduced the work of producing a candidate change. It has not
settled how a team should accept that change.

A candidate change is what an agent produced. An accepted change is the exact
result that an owner approved after the required evidence passed. This essay is
about the distinction between those two states.

For a short task, a person can make the acceptance decision after the agent
stops. They can read the diff and run the tests. Long or unattended work needs a
different process. The owner must define the result, limits, and required proof
before the run. Software outside the worker must preserve those decisions while
the work continues.

The central claim is simple. A coding agent can choose effective steps inside a
run. It cannot be the sole authority for what counts as done, whether its proof
is enough, or whether its result should enter a shared product. Those are
product decisions. An operator can make them by hand or encode them in a
separate controller. In either case, the worker must not control the final
decision.

That leaves four questions to settle before long work begins.

- What result did the operator approve?
- What evidence will show that the work is complete?
- What should happen if a process stops or the Job hits a limit?
- Who may accept the result and apply it to the shared repository?

Current agent products answer parts of these questions. They do not answer them
in the same way or at the same level. Some controls belong to a turn. Some
belong to a session or worktree. None provides a common Job record across
different products. An operator who combines them must supply that common
control.

This essay first compares where Claude Code, Codex, and Pi place their control.
It then tests the claim that the harness matters more than the model. It ends
with the parts of DeadReckon that try to make acceptance explicit across
different agents.

I should state my interest. DeadReckon is my software. I use its implementation
as a design case, not as proof that this design improves outcomes in live use.

## 1. Coding agents already do a great deal

Simon Willison gives a useful short account of a coding agent. It is an
"LLM + system prompt + tools in a loop." The model produces the next response.
The product supplies instructions and gives the model tools. It reports each
result and repeats the process. That product layer also decides what the model
can see and what actions it can request. [Willison calls the whole product a
harness for the model](https://simonwillison.net/guides/agentic-engineering-patterns/how-coding-agents-work/).

That account now covers products with very different scopes. A list of tools
does not show the difference. The useful comparison is where each product puts
control over a long piece of work.

| Axis | Claude Code | Codex | Pi |
|---|---|---|---|
| Unit of work | A saved session holds the conversation. One `/goal` condition can keep that session working across turns and returns when the session resumes. | A saved chat or thread holds the conversation. Goal mode keeps one outcome, its constraints, and its checks in that chat. | A JSONL session stores a tree of messages by working directory. The user can resume, branch, fork, clone, import, or export it. |
| Completion | [`/goal` uses a separate small model to judge a condition from the conversation](https://code.claude.com/docs/en/goal). The evaluator cannot run a command or read a file. A Stop hook can instead run a script. | [Goal mode asks Codex to verify its own progress against the goal](https://learn.chatgpt.com/docs/long-running-work). In scripts, `turn.completed` reports that a turn ended, and a JSON Schema can constrain the final message. | The core ends a model turn and reports its events. Pi does not include a goal loop. An extension or RPC client can add one. |
| Coordination | [Subagents, agent teams, and scripted workflows divide work in different ways](https://code.claude.com/docs/en/agents). A workflow can hold the plan outside an agent's context. | Subagent threads can work in parallel and return results to the main thread. Separate chats can also run in parallel. | The core [does not include subagents](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/README.md#what-pi-doesnt-have). The user can add them through extensions, packages, or other processes. |
| State and recovery | Sessions can resume. An active goal returns too, but its timer, turn count, and token total start again. Workflows can resume inside the same session. | Chats can resume. Codex keeps each chat's transcript and working directory. App worktrees stay associated with their chats and can restore a saved snapshot after cleanup. | The full session tree stays in one JSONL file. Compaction is lossy in the active context, but the original history remains available. |
| Execution boundary | [Tool permissions and an operating system sandbox control file and network access](https://code.claude.com/docs/en/sandboxing). Worktree rules can stop a session from editing the main checkout. | [The operating system sandbox and approval policy are separate controls](https://learn.chatgpt.com/docs/agent-approvals-security). Network access is off by default. An automatic reviewer can decide some approval requests without widening the sandbox. | [Pi has no built in permission system](https://github.com/earendil-works/pi#security). It runs with the rights of its host process. The user must add a container, a virtual machine, or an extension when they need a stronger boundary. |
| Evidence and accounting | JSON and JSONL output include a session ID, result, usage, and cost. Hooks can observe session, turn, tool, and worktree events. Goal status records the latest reason and token use. | [`codex exec --json` emits thread, turn, tool, file change, and error events](https://learn.chatgpt.com/docs/non-interactive-mode). A completed turn includes token use. | JSON, RPC, and the SDK expose the loop to other programs. The saved session records messages and tool results. The interface shows token use and cost. |
| Result isolation and promotion | [Worktrees isolate sessions and subagents](https://code.claude.com/docs/en/worktrees). Keeping a result still requires a chosen Git action such as a merge, push, or pull request. | [The desktop app can keep a chat in a worktree, hand it into the local checkout, or create a branch for review](https://learn.chatgpt.com/docs/environments/git-worktrees). The noninteractive guide also shows a safer split between generating a patch and opening a pull request. | Pi edits the directory supplied by its host. The host must create an isolated copy and decide how a result enters the shared repository. |
| Limits | A goal can name a turn or time limit and shows token use. Those counters restart when a session resumes. | A goal keeps the current sandbox and approval rules. An outside workflow must track a limit that spans several chats and retries. | A session totals tokens and cost. An outside runner can enforce a total across several Pi sessions. |

This table records documented product behavior on 5 August 2026. It compares the
terminal products and the local Codex app where noted. It is not a scorecard.
Claude Code builds goals and scripted groups of agents into the product. Codex
distributes control across chats, worktrees, automation, and event streams. Pi
keeps the core small and makes outside control easy to add.

The comparison changes the claim. Modern coding agents do provide substantial
harness machinery. The remaining problem is that each product defines its own
session, completion signal, evidence, limits, and promotion path. A team can use
one product's rules for one run. Those rules do not become a common acceptance
process when work spans products, processes, or restarts.

## 2. Long work requires intent and proof

Anthropic researchers report that developers use artificial intelligence for
about 60 percent of their work but fully delegate only 0 to 20 percent of their
tasks. The authors say the rest still needs setup and supervision. It also needs
validation and human judgment. The vendor did not publish enough method detail
to treat the figures as a measure of all developers. The useful point is
narrower. Frequent use and full delegation are not the same thing.
[Anthropic's own account keeps validation with the
developer](https://resources.anthropic.com/hubfs/2026%20Agentic%20Coding%20Trends%20Report.pdf).

Long work changes when the operator must make decisions. If the operator waits
until the end to define proof, the agent has already chosen what to test and
what to ignore. If the operator waits for a permission request to define the
boundary, unattended work must either stop or run with broader access. If the
agent edits the shared checkout, review begins only after the shared state has
changed.

Goal modes, sandboxes, worktrees, and structured events reduce these problems.
They do not remove the need for an acceptance rule outside the worker. A goal
evaluator may read only what the worker put in the conversation. A worktree can
isolate a result without deciding whether to promote it. A turn completion event
can show that the agent stopped without showing that the approved result exists.

Published studies show why these distinctions affect the result. They do not
test the same setting, so their findings should remain separate.

Cursor researchers reviewed 731 agent runs from one model on SWE Bench Pro.
In many successful runs, the agent recovered the original fix from Git history.
After the researchers sealed that history and limited network access, the
reported score fell from 87.1 percent to 73.0 percent. The model had not changed.
The test setting had stopped exposing an answer. This is a vendor study on one
benchmark, but it shows that the boundary around a run changes what a passing
result means. [Cursor published the setting and the
counts](https://cursor.com/blog/reward-hacking-coding-benchmarks).

The Qwen team studied a different problem during model training. They added a
quality judge and a monitor for suspicious behavior to three versions of SWE
Bench. The share of solutions that passed through unwanted behavior fell from
28.57 percent to 0.56 percent. The share judged clean rose from 40.22 percent to
60.53 percent. This result supports stronger checks during training. It does not
test DeadReckon's completion design, and it does not tell us the error rate of a
judge in live product work. [The authors report both the method and its
limits](https://arxiv.org/html/2606.26300v2).

Willison gives the operator rule that follows from both cases. Run the code, and
review it before you publish it. He says that code never executed only works by
luck, and he lists an unreviewed pull request as an agent use pattern to avoid.
[His testing guide](https://simonwillison.net/guides/agentic-engineering-patterns/first-run-the-tests/)
and [review guide](https://simonwillison.net/guides/agentic-engineering-patterns/anti-patterns/)
put responsibility for the first review on the person who used the agent.

Those rules are sound. They also limit delegation. If each long run ends by
giving all intent and proof work back to a person, the agent can work alone but
the product process cannot.

This operator work led writers to focus on harnesses.

## 3. The harness claim is useful but incomplete

Writers now often say that the harness matters more than the model. Addy Osmani
states the strong form. He says that a good harness around a decent model can
beat a poor harness around a better model. His article is a useful account from
practice. It is still an opinion, not a controlled comparison.
[Osmani explains which product choices he includes in the
harness](https://addyosmani.com/blog/agent-harness-engineering/).

A study from researchers at Meta and Harvard gives the claim firmer support.
The researchers held Claude 4.5 Sonnet and the task set fixed, then changed the
agent software around the model. On 731 tasks from SWE Bench Pro, the reported
mean pass rate rose from 45.8 percent with Live SWE Agent to 52.7 percent with
their CCA system. They repeated the measurement three times with different
random starting values. The 6.9 point change shows that the surrounding software
can change measured results while the model stays the same. It does not show
that the harness always matters more than the model.
[The CCA paper gives the task set and comparison](https://arxiv.org/html/2512.10398v6).

Birgitta Böckeler uses a wider meaning of harness. She includes the tools and
instructions inside the agent. She also describes an outer harness that users
build from project rules and checks. In her account, a person still directs the
work through those instructions and checks. In a later article, Böckeler warns
that static checks can give a false sense of safety. [Her main article describes both the inner and outer
parts](https://martinfowler.com/articles/harness-engineering.html), and
[Böckeler gives the full limit in her article on sensors](https://martinfowler.com/articles/sensors-for-coding-agents.html).

Hugo Bowne Anderson argues for a narrower scope. Most agents do not need complex
memory or handoffs. Most also do not need groups of agents. The right amount
depends on how long the work runs and how many outside systems it can change.
Coding work often needs more support than a short research task, but the support
should answer a known failure or risk. [Anderson gives his full argument
here](https://www.oreilly.com/radar/stop-overengineering-your-agent-harness/).

Together, these sources support a more exact claim. The software around a model
can change results by a large amount. More software is not always better. The
word `harness` is too broad to identify the part responsible for a failure.

It helps to separate four layers.

| Layer | What it does | What it does not prove by itself |
|---|---|---|
| Model | Proposes the next response | That a requested action ran or worked |
| Agent product | Runs the model tool loop and manages its session. It applies product permissions. | That its own report satisfies the operator's goal |
| Project harness | Applies repository instructions and checks | That the worker could not alter the evidence or that the result should enter the shared product |
| Job controller | Records the approved Job and its state. It enforces limits. It keeps independent checks and publication rights outside the worker. | That the approved contract expresses the right product choice |

The last layer does not make product judgment disappear. It requires specific
decisions before the run and records them. It can then refuse a result when the
agreed proof is missing, even if the agent says the work is complete.

This is the part DeadReckon tries to provide.

## 4. What predictable delegation requires

DeadReckon did not begin with its current completion design. The first unmet
needs study in May 2026 focused on the burden around agent runs. It ranked live
context and spend tracking first. It also called for the following controls.

- Coordination across isolated work copies.
- Reliable undo.
- Prompt to code records.
- Disposable sandboxes.
- Billing limits.

The build plan later pulled provider routing and a run queue into its first
release. The [first research brief](research/harness-essay/RESEARCH-BRIEF.md)
preserves the source findings used for this account.

That research did not state the full argument in this essay. It did not begin
with an independent meaning check or a signed completion receipt. Those parts
came later. Building the first controls raised a deeper question. A system could
keep a process alive and record its spend. It could preserve the output while
still having no sound reason to accept the result.

The current design treats an agent run as one attempt inside a durable Job. A
Job is the stored record for one approved result. It holds the limits and the
evidence. It accepts a result only after separate checks agree. It then applies
that exact result through a trusted controller.

Eight controls follow from that requirement.

### 4.1 Approve an executable account of done

A goal written in prose is necessary, but it is not enough. The operator also
needs a completion contract that names checks which another process can run.

DeadReckon refuses to start a strict Job in any of these cases.

- The acceptance file has no checks.
- The file requires no check.
- The only check confirms that a working directory exists.

The [strict contract validator](../crates/deadreckon-core/src/completion.rs)
makes an empty account of done an error before the first agent turn.

DeadReckon accepts the check types listed below.

- A test command.
- A build.
- A file check.
- A text match.
- A shell command.

Each check says whether it must pass. The
[acceptance types](../crates/deadreckon-core/src/gate.rs) turn part of the goal
into a repeatable decision.

This work happens before the run. The operator must decide what evidence is
useful before the agent changes the code. That is extra setup, but it prevents a
weak test chosen at the end from becoming the definition of success.

### 4.2 Keep execution proof separate from meaning

A test can show that a command passed. It cannot show by itself that the result
meets the whole goal. A model can judge the goal in context. It should not be
allowed to accept the result after a required command fails.

DeadReckon therefore uses two decisions. A fixed checker runs the approved checks.
A fresh model call then judges whether the result covers the approved goal. The
judge receives a limited evidence set and has no worker session. It can return
one of these results.

- `achieved`.
- `revise`.
- `uncertain`.

Only `achieved` can support completion.
The [semantic judge](../crates/deadreckon-runtime/src/semantic_judge.rs) cannot
override a failed fixed check.

This split does not make the second decision objective. It makes the source of
each decision clear. If the fixed check fails, the command or result failed. If
the meaning check is uncertain, a person must review the Job or the agent must
make another attempt.

### 4.3 Keep proof outside the worker's control

Independent checks are not independent if the worker can replace their output
or read the secret used to approve it.

DeadReckon stores its signing material outside the agent workspace. It runs the
check before a separate trusted process signs the result. Before completion, it
also probes these boundaries.

- The worker cannot read the signing key.
- The worker cannot change the control records.
- The worker cannot write the proof.

The
[boundary record](../crates/deadreckon-core/src/sandbox_observation.rs) binds
that observation to the result under review.

This is stronger than asking the agent to run the tests and report what
happened. It is also narrower than a claim that no hostile program can escape
the host controls. The probe checks named boundaries. It does not prove every
property of the operating system sandbox.

### 4.4 Bind completion to one result

The result that passed must be the result that gets accepted. A later edit must
not inherit earlier proof.

DeadReckon creates a signed completion receipt only when all required facts are
present.

- The fixed checks passed.
- The meaning judge returned `achieved`.
- The boundary observation belongs to the same Job attempt.

The receipt records a content hash for each approved input and each proof file.
A content hash is a short value that changes when file content changes. The
[completion code](../crates/deadreckon-core/src/completion.rs) checks those links
again whenever it validates the receipt.

The word `receipt` is literal here. The controller stores what it accepted and
why. The receipt does not mean that the work is good in some wider sense. It
means that this exact result met the contract through the recorded process.

### 4.5 Keep the Job record after one process ends

The controller should not lose the Job record when a terminal closes or a
supervisor restarts.

DeadReckon records Job changes in an append only event file. A lease gives one
worker authority at a time. Each new owner receives a higher lease number, and
an old owner cannot add another event. The
[Job state code](../crates/deadreckon-core/src/job.rs) rejects gaps and invalid
state changes. The [lease code](../crates/deadreckon-core/src/job_lease.rs)
checks ownership again while it holds the control lock.

Process exit no longer decides the Job result by itself. After a restart, the
controller rebuilds the Job state from its record and decides whether the work
may continue. It does not treat a new process as a new task.

### 4.6 Make limits survive restarts

A timeout on one child process does not limit a Job that can start another
child. The same problem applies to attempt counts and spend.

DeadReckon stores the approved limits with the Job. Its wall clock total comes
from all recorded attempts, so a restart does not set it back to zero. It keeps
the attempt limit and deadline in the same durable policy. The
[supervisor](../crates/deadreckon/src/commands/supervisor.rs) refuses to record a
clean stop until it has accounted for the child processes it started.

Provider cost records need more care because products report different units.
The controller can enforce only the spend it can observe. Missing cost data is
an evidence limit, not a zero cost.

### 4.7 Isolate the result and apply it on purpose

An agent may produce a valid candidate without receiving authority to change
the shared branch.

DeadReckon keeps candidate work in an isolated Git copy. A trusted step applies
only the result named by the verified receipt. It records the branch state
before and after that action so undo can check that it is reversing the same
delivery. The [promotion code](../crates/deadreckon-core/src/promotion.rs) keeps
completion and publication as separate decisions.

This preserves a useful operator choice. The operator can approve the contract
before the run and still reserve the right to publish after seeing the result.

### 4.8 Recheck old proof

A later reader should be able to detect missing or changed evidence.

When DeadReckon reads a Job marked `verified`, it validates the current receipt
again. If someone deletes or changes the receipt, the public Job view reports
that the verified event no longer matches the stored proof. It does not silently
trust the old label.

That check handles a common problem in event based systems. An event says what
the controller accepted at one time. Each later read must still check whether
the evidence for that event exists and remains valid.

### What the current evidence does not prove

The repository contains substantial implementation and test evidence for these
parts. It does not yet show that DeadReckon makes operators more effective in
daily product work.

The checked hostile case record reports 13 passing proof groups and no failures.
It also marks nine live claims as unproven. They include these distinct gaps.

- Recovery after a machine restart.
- A positive Linux boundary result using bubblewrap.
- A live hostile worker trial with an independent judge.

The [credential free record](../examples/watchkeeper-dogfood/credential-free-results.json)
binds its
passing groups to a clean source revision, but later changes need new evidence.

A separate 24 task live matrix records only two attempts. Neither reached a
verified receipt, and 22 tasks were not run. The
[dogfood guide](../examples/watchkeeper-dogfood/README.md) states those results
without treating an attempted run as success.

The meaning judge has another open question. The code handles every result that
does not support completion. The project has not yet measured how often the
judge accepts bad work or rejects good work on representative Jobs.

The [Map of DeadReckon](MAP-OF-DEADRECKON.md) records further limits. The macOS
sandbox applies selected denials rather than a complete policy. Its network
control is weaker than its file control. Several recovery claims still need a
host event.

The fair claim is therefore limited. DeadReckon implements and tests a design
for durable, independently checked Jobs. It has not yet proved the operator
outcome that motivated the design.

## 5. If you wrap agents you did not write

An outer controller must work with products that use different words and change
at different speeds. The errors corrected in this revision show the first rule.
A local product snapshot is not current product documentation, and an adapter is
not a full account of the product.

The work produces several practical rules.

### Record the exact integration

Store these facts for each attempt.

- The route and product version.
- The launch arguments.
- The model choice.
- The event format.

Check the product maker's current documentation before making a broad capability
claim. Keep old test streams as evidence for the adapter version they cover, not
as evidence about the latest product.

### Observe more than the agent's final text

Collect machine readable events when the product has them. Also record these
facts.

- Process exit.
- Files changed.
- Checks run.
- The exact source and result states.

A product may not offer a completion event that means what the outer controller
needs. The controller should derive its own Job result from evidence it can
inspect.

### Treat unattended control as a full policy

Products now offer several forms of unattended work. One can retain a sandbox
while suppressing prompts. Another can send approval requests to an automatic
reviewer. Another has no built in permission system. Record the exact sandbox
settings. Also record the approval and reviewer settings. Probe the boundary
that the Job depends on.
Do not turn all of these modes into one `unattended` flag and assume equal
protection.

### Keep each cost in its original unit

Each product can report cost in a different unit. E.g., a route may report
tokens instead of dollars. Keep the original value and its source. Any price
conversion should be a separate record with a dated rate. If a route provides no
useful cost event, say that the spend is unknown.

### Keep a Job clock outside each attempt

Each product may enforce a limit on one call or process. The outer controller
still needs a total for the whole Job. It should add completed attempts and the
current attempt, then apply the approved deadline and attempt limit without
resetting them after a restart.

### Separate result isolation from host control

A Git worktree keeps one candidate result away from another. It does not limit
network access or protect secrets. An operating system sandbox can limit those
actions. It does not decide which commit should enter the shared branch. A
controller needs both boundaries and should name which claim each one supports.

### Refuse completion when proof is missing

Several facts can be useful evidence.

- Process exit.
- A final message.
- A passing test.

None should silently stand in for a required receipt. The Job should stop as
unproven if a required provider stream is unknown. It should do the same if the
boundary probe is absent or the judge cannot return a sound decision. A person
can then review it.

### Use one account of Job state

Every command that changes a Job's lifecycle should use the same rules. This
includes commands that start work and commands that publish or undo it. A legacy
path that can publish without the current checks weakens the whole design. The
controller should refuse that path or bring it under the same contract.

### Measure the judge

A model judge adds a new source of error. Keep its full input and output. Also
record these facts.

- The route.
- The model.
- The version.

Build a reviewed set of good and bad Jobs. Measure false acceptance and false
rejection before using the judge's confidence as a product claim.

These rules add cost. They make sense when a Job is long or costly. They also
make sense when work is hard to undo or can affect other people. A short local
edit may need only the agent's own tools and a person's review. Use the added
controls only when the task's risk requires them.

## Conclusion

Coding agents have removed much of the effort of producing a candidate change.
Their tool loops and saved sessions now cover work that required close
supervision a short time ago. Sandboxes and other agents extend that work.

The remaining burden is more exact than "everything else." The operator must
approve a useful account of done. The system must preserve limits and history
when processes fail. A checker outside the worker's control must test the exact
result. A trusted step must decide whether to apply it.

Current agent products contain parts of that process, and they keep adding more.
The measured comparisons show that those parts can change outcomes without a
model change. They do not show that one large harness suits every task.

DeadReckon is an attempt to make the full Job process explicit across different
agents. Its code now has the main control points, and its focused tests cover
many hostile cases. Its own records also show what remains. The next proof must
come from repeated live Jobs and measured judge errors. It must also include
recovery after machine and process failures. Operators must show that the system
reduced their work without accepting worse results.

Until then, the right conclusion is modest. A capable coding agent can do the
work. Predictable delegation also needs a system that records what work was
approved. It must record what evidence passed. It must explain why the operator
may trust this exact result.

## Sources

### Agent products

- Simon Willison, [How coding agents
  work](https://simonwillison.net/guides/agentic-engineering-patterns/how-coding-agents-work/),
  16 March 2026.
- Anthropic, [Sandboxing in Claude
  Code](https://code.claude.com/docs/en/sandboxing), accessed 5 August 2026.
- Anthropic, [Keep Claude working toward a
  goal](https://code.claude.com/docs/en/goal), [Run agents in
  parallel](https://code.claude.com/docs/en/agents), and [Run parallel sessions
  with worktrees](https://code.claude.com/docs/en/worktrees), accessed 5 August
  2026.
- OpenAI, [Agent approvals and
  security](https://learn.chatgpt.com/docs/agent-approvals-security), accessed
  5 August 2026.
- OpenAI, [Long running
  work](https://learn.chatgpt.com/docs/long-running-work), [Noninteractive
  mode](https://learn.chatgpt.com/docs/non-interactive-mode), and
  [Worktrees](https://learn.chatgpt.com/docs/environments/git-worktrees),
  accessed 5 August 2026.
- Mario Zechner and contributors, [Pi coding agent
  README](https://github.com/earendil-works/pi), accessed 5 August 2026.

### Measured and published evidence

- Anthropic, [2026 Agentic Coding Trends
  Report](https://resources.anthropic.com/hubfs/2026%20Agentic%20Coding%20Trends%20Report.pdf),
  2026.
- Xingyu Li and others, [Your Agent May Not Need to Be So
  Scaffolded](https://arxiv.org/html/2512.10398v6), revised 3 February 2026.
- Minghao Yan and others, [When Tests Lie](https://arxiv.org/html/2606.26300v2),
  revised 29 June 2026.
- Naman Jain, [Reward hacking coding
  benchmarks](https://cursor.com/blog/reward-hacking-coding-benchmarks), 25 June
  2026.
- Birgitta Böckeler, [Harness
  engineering](https://martinfowler.com/articles/harness-engineering.html), 2
  April 2026.
- Birgitta Böckeler, [Sensors for coding
  agents](https://martinfowler.com/articles/sensors-for-coding-agents.html), 1
  July 2026.
- Addy Osmani, [Agent harness
  engineering](https://addyosmani.com/blog/agent-harness-engineering/), 19 April
  2026.
- Hugo Bowne Anderson, [Stop overengineering your agent
  harness](https://www.oreilly.com/radar/stop-overengineering-your-agent-harness/),
  3 June 2026.
- Simon Willison, [First, run the
  tests](https://simonwillison.net/guides/agentic-engineering-patterns/first-run-the-tests/)
  and [Anti
  patterns](https://simonwillison.net/guides/agentic-engineering-patterns/anti-patterns/),
  2026.

### DeadReckon and research evidence

- [Map of DeadReckon](MAP-OF-DEADRECKON.md), 2 August 2026.
- [DeadReckon provider descriptions](../crates/deadreckon-providers/descriptors/)
  and [test streams](../crates/deadreckon-providers/tests/fixtures/), read at
  commit `75a9a175`.
- [Credential free adversarial
  results](../examples/watchkeeper-dogfood/credential-free-results.json) and the
  [live trial guide](../examples/watchkeeper-dogfood/README.md), read at commit
  `75a9a175`.
- [Research brief and source
  audit](research/harness-essay/), 5 August 2026.
