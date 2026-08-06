# Source audit for the harness essay

This audit covers the essay and research assets added in commit `75a9a175`.
It records the checks made for the revision dated 5 August 2026.

The audit uses current documents from product makers for current product facts.
It uses local adapters and saved streams only for claims about DeadReckon's
integrations. It treats vendor studies as vendor studies and states their scope.
It treats DeadReckon's checked evidence as implementation evidence unless a live
trial supports a wider claim.

## Material corrections

| First draft claim | What the check found | Revision decision |
|---|---|---|
| Claude Code has no sandbox of its own. | [Current Claude Code documentation](https://code.claude.com/docs/en/sandboxing) describes operating system controls on macOS and Linux for file and network isolation. | Replaced the claim with the current documented behavior. |
| Pi has four built in tools. | The [current Pi README](https://github.com/earendil-works/pi) lists `read`, `bash`, `edit`, `write`, `grep`, `find`, and `ls`. | Changed the count to seven and kept the more important fact that Pi has no built in permission system. |
| Unattended work turns approvals off rather than delegating them. | [Current Codex documentation](https://learn.chatgpt.com/docs/agent-approvals-security) separates the sandbox from approval prompts and also describes automatic review for eligible requests. Claude Code also supports unattended work within configured permissions and sandbox rules. | Removed the product wide claim. The revision tells an outer controller to record the exact sandbox, approval, and reviewer policy. |
| Claude Code's run does not outlive its process. | A local process registry and service log described the processes running on one machine. They did not establish the product's full lifecycle behavior. | Removed the product claim. Kept the narrower need for a Job record that survives any one agent or controller process. |
| Böckeler's harness account stays inside one agent process. | [Böckeler's article](https://martinfowler.com/articles/harness-engineering.html) explicitly describes an outer harness built by users from project instructions and checks, with a person directing the work. | Corrected the reading. The revision separates four layers and places DeadReckon at the Job controller layer. |
| DeadReckon began from the gap between an agent saying done and an operator accepting it. | The original unmet needs work focused first on spend, context, coordination, undo, records, sandboxes, and billing limits. Independent meaning checks and signed completion receipts came later. | Rewrote the origin account as an evolution. The first controls showed the later completion problem. |
| The Qwen result supports DeadReckon's two part completion check. | The [Qwen study](https://arxiv.org/html/2606.26300v2) added a quality judge and a behavior monitor during training and evaluation across three benchmark variants. It did not test DeadReckon or a live Job controller. | Kept the numbers, stated the study setting, and removed the direct inference to DeadReckon. |
| A missing field in a DeadReckon provider adapter means the product lacks that feature. | An adapter records the contract DeadReckon uses. It does not list every product feature. Some descriptions also lacked a saved stream. | Limited all such findings to the adapter snapshot at commit `75a9a175`. |
| The dogfood evidence forms one result. | The credential free hostile case record has 13 passing proof groups and nine unproven live claims. The separate live matrix has 24 tasks, two attempts, no verified receipt, and 22 tasks not run. | Reports the two evidence sets separately. |
| Recent social posts strengthen the harness claim. | The posts in the first research pass were useful examples of current language but were not strong evidence for product behavior or outcomes. A new retrieval attempt returned no usable items because its configured sources were unavailable. | Removed social posts from the essay. The failed retrieval is not evidence that no discussion exists. |

## Claims kept with tighter limits

### The project record

The SpecStory book source supports these counts.

- 1,310 saved sessions.
- 4,670 commits on one shared branch.
- 614 interruptions across 184 transcripts.
- 335 refused tool calls.
- 441 correction openings.
- 88 uses of `Do not edit files` in April and May 2026.
- Four goal documents with their own exit test.

These counts describe one team and one product. The session count came from the
saved records, and the commit count came from Git. The interruption counts and
prompt counts came from fixed text searches, so they can miss events or count a
string used for another reason. The goal document count came from a hand review.
I state these limits next to the table in the essay.

### The effect of agent software

The [CCA study](https://arxiv.org/html/2512.10398v6) gives the strongest measured
support for the harness claim in the source set. The researchers held Claude
4.5 Sonnet and the task set fixed. They reported a mean pass rate of 45.8 percent
for Live SWE Agent and 52.7 percent for CCA on 731 SWE Bench Pro tasks. They
repeated the measurement three times with different random starting values.

This supports a 6.9 point effect from the surrounding agent software in that
setting. It does not rank the harness and model for all tasks. The revised essay
states both points together.

### A passing benchmark depends on its boundary

Cursor reported 731 Claude Opus 4.8 Max agent runs on SWE Bench Pro. The team
found that 63 percent of successful runs retrieved the original fix from Git
history. After the team sealed Git history and limited network access, the score
fell from 87.1 percent to 73.0 percent.

This is a study by a vendor on one benchmark. It supports the claim that the
environment changes what a pass means. It does not support the claim that the
model became less capable.

## Sources not used as main evidence

The revision does not use the largest SpecBench gap. The authors report an
important difference between visible tests and hidden tests. A full account of
the extreme figure would take more method detail than this essay can support.

The revision does not use counts from local installations. This includes command
counts and saved plan counts. It also includes plugin counts. Those values
describe one setup and become stale quickly.

The revision does not use Reddit or X posts as support for product behavior.
They can show that a phrase is common. The CCA comparison provides better
support for the actual effect.

## DeadReckon evidence boundary

Source inspection supports the following implementation claims.

- A strict Job cannot start with no checks or no required check.
- Fixed checks and the meaning judge make separate decisions.
- Signing material and proof output sit outside the worker workspace.
- A completion receipt binds the approved inputs, exact result, checks,
  judgment, and sandbox observation.
- The Job event record and lease rules survive one process.
- Wall time adds work across attempts.
- The trusted publication step applies the result named by the verified receipt.
- A later read checks that the receipt still exists and remains valid.

The checked hostile case record supports 13 passing proof groups against the
named source revision. Nine live claims remain unproven. The live matrix does
not contain a verified run. No reviewed data set yet measures false acceptance
or false rejection by the meaning judge.

The revision therefore describes DeadReckon as an implemented and tested design.
It does not claim proven operator benefit.

## Writing comparison

I compared the final revision with Simon Willison's technical guides. His
claims are short, and each main claim has one concrete example. He also states a
limit close to the evidence. The revision follows those patterns. It keeps a
longer form than Willison's guide entries because it must explain local
implementation evidence and open proof gaps.
