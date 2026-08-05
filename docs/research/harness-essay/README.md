# Research assets for the harness essay

These files back
[`docs/WHAT-CODING-AGENTS-LEAVE-TO-THE-OPERATOR.md`](../../WHAT-CODING-AGENTS-LEAVE-TO-THE-OPERATOR.md).

| File | What it holds |
|---|---|
| `CITATIONS.md` | Every external quote used in the essay, verbatim, with author, publication, date and URL. Includes a section listing claims that were found but could not be verified, which the essay does not use. |
| `RESEARCH-BRIEF.md` | The consolidated brief from the research pass over the DeadReckon source, the book, the course, Pi, and the on-disk evidence of what mainstream coding agents provide. |
| `agent-harness-engineering-*-raw-harness.md` | Raw output from the social and web corpus search over the 30 days to 5 August 2026, kept so the community quotes can be traced back. |

## How the research was done

1. Eight readers ran in parallel over the DeadReckon Rust crates, the
   `/Users/gdc/extract-agentic-engineering` book and course, `/Users/gdc/pi`,
   and the on-disk state of installed coding agents. Each returned claims paired
   with a file path, and a list of what it could not verify.
2. The web and social corpus search covered Reddit, X, YouTube, Hacker News,
   GitHub, TikTok, Instagram and general web search for the 30 days to
   5 August 2026.
3. Primary sources were fetched directly rather than quoted from search
   snippets, so every quote in `CITATIONS.md` comes from the article itself.
4. The draft was then judged by four independent readers, covering repository
   fact checking, external citation checking, compliance with the writing rules,
   and a comparison against Simon Willison's technical writing.

## Rules the essay follows

The prose style is fixed. Plain everyday words, complete sentences, no dashes of
any kind, colons only to introduce a list, no analogies or imagery, no invented
hyphenated adjectives, and no series of three items inside a sentence where a
bullet list would do.
