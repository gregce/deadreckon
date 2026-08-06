# Research assets for the harness essay

These files back
[`docs/WHAT-CODING-AGENTS-LEAVE-TO-THE-OPERATOR.md`](../../WHAT-CODING-AGENTS-LEAVE-TO-THE-OPERATOR.md).

| File | What it holds |
|---|---|
| `SOURCE-AUDIT.md` | The current claim ledger. It records material corrections to the first draft, the limits placed on each source, and the evidence boundary for DeadReckon. |
| `CITATIONS.md` | The quote record assembled for the first draft. It is kept for traceability, but `SOURCE-AUDIT.md` records which claims remain in the revised essay. |
| `RESEARCH-BRIEF.md` | The consolidated brief from the first research pass over the DeadReckon source, the book, the course, Pi, and the installed agent state. Read it with `SOURCE-AUDIT.md`, since installed product facts became stale and some inferences were too broad. |
| `agent-harness-engineering-*-raw-harness.md` | Raw output from the social and web corpus search over the 30 days to 5 August 2026, kept so the community quotes can be traced back. |

## How the research was done

1. Eight readers ran in parallel over the DeadReckon Rust crates, the
   `/Users/gdc/extract-agentic-engineering` book and course, `/Users/gdc/pi`,
   and the on-disk state of installed coding agents. Each returned claims paired
   with a file path, and a list of what it could not verify.
2. The first web and social search covered Reddit, X, YouTube, Hacker News,
   GitHub, TikTok, Instagram and general web search for the 30 days to
   5 August 2026.
3. Primary sources were fetched directly rather than quoted from search
   snippets, so every quote in `CITATIONS.md` comes from the article itself.
4. The draft was then judged by four independent readers, covering repository
   fact checking, external citation checking, compliance with the writing rules,
   and a comparison against Simon Willison's technical writing.
5. The revision checked current first party product documents, corrected broad
   claims based on local state, and separated implementation tests from live
   operator evidence. `SOURCE-AUDIT.md` records those decisions.

## Rules the essay follows

The prose style is fixed. Plain everyday words, complete sentences, no dashes of
any kind, colons only to introduce a list, no analogies or imagery, no invented
hyphenated adjectives, and no series of three items inside a sentence where a
bullet list would do.
