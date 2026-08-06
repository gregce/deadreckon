# Citations gathered for the first harness essay draft

> This file preserves the quotes checked for commit `75a9a175`. The revised
> essay does not use every quote or claim below. See
> [`SOURCE-AUDIT.md`](SOURCE-AUDIT.md) for the current claim decisions.

Every quote below was fetched from the named URL during research on 2026-08-05.
Quotes are verbatim. Where a claim came from a secondary source, that is marked.

## The harness, defined

**Birgitta Böckeler, "Harness engineering for coding agent users", martinfowler.com, 2 April 2026.**
<https://martinfowler.com/articles/harness-engineering.html>

- "The term harness has emerged as a shorthand to mean everything in an AI agent except the model itself - Agent = Model + Harness."
- Guides (feedforward) "anticipate the agent's behaviour and aim to steer it *before* it acts."
- Sensors (feedback) "observe *after* the agent acts and help it self-correct."
- "A well-built outer harness serves two goals: it increases the probability that the agent gets it right in the first place, and it provides a feedback loop that self-corrects as many issues as possible before they even reach human eyes."
- "Separate, you get either an agent that keeps repeating the same mistakes (feedback-only) or an agent that encodes rules but never finds out whether they worked (feed-forward-only)."
- "A coding agent has none of this: no social accountability, no aesthetic disgust at a 300-line function, no intuition that 'we don't do it that way here,' and no organisational memory."
- "A good harness should not necessarily aim to fully eliminate human input, but to direct it to where our input is most important."

**Birgitta Böckeler, "Maintainability sensors for coding agents", martinfowler.com, 27 May 2026.**
<https://martinfowler.com/articles/sensors-for-coding-agents.html>

- "While some of these sensors really do increase my trust into the quality of the outcomes, they are not a magical solution to take the human totally out of the loop."
- "I can't help but wonder if this can also lead to a false sense of security and an illusion of quality... there are lots of more semantic aspects of quality that static analysis cannot catch."

**Simon Willison, "How coding agents work", 16 March 2026.**
<https://simonwillison.net/guides/agentic-engineering-patterns/how-coding-agents-work/>

- "A coding agent is a piece of software that acts as a harness for an LLM, extending that LLM with additional capabilities."
- "LLM + system prompt + tools in a loop."
- "A simple tool loop can be achieved with a few dozen lines of code on top of an existing LLM API" though "a good tool loop is a great deal more work than that."

## The harness beats the model

**Addy Osmani, "Agent Harness Engineering", 19 April 2026 (dateline on addyosmani.com, re-verified 5 August 2026). Republished by O'Reilly Radar 15 May 2026.**
<https://addyosmani.com/blog/agent-harness-engineering/>

- Verbatim, with the page's own lowercase and British spelling: "Claude Code, Cursor, Codex, Aider, Cline: these are all harnesses." The next sentence reads "The model underneath is sometimes the same, but the behaviour you experience is dominated by what the harness does."
- The sentence before it reads: "If that sounds like a lot of surface area, it is. And it's *your* surface area, not the model provider's."
- "A decent model with a great harness beats a great model with a bad harness."
- "The gap between what today's models can do and what you see them doing is largely a harness gap."
- "Agent = Model + Harness. If you're not the model, you're the harness."
- What Osmani counts inside the harness: system prompts, CLAUDE.md, AGENTS.md, skill files, subagent prompts, tools, skills, MCP servers, bundled infrastructure (filesystem, sandbox, browser), orchestration logic, hooks and middleware, observability (logs, traces, cost and latency metering).

**Sherman Wong, Zhenting Qi, Zhaodong Wang, Nathan Hu, Samuel Lin, Jun Ge, Erwin Gao, Wenlin Chen, Yilun Du, Minlan Yu, Ying Zhang (Meta, Harvard), "Confucius Code Agent: Scalable Agent Scaffolding for Real-World Codebases", 3 February 2026.**
<https://arxiv.org/html/2512.10398v6>

- SWE-Bench-Pro, Claude 4 Sonnet: SWE-Agent scaffold 42.7 percent, CCA scaffold 45.5 percent.
- SWE-Bench-Pro, Claude 4.5 Sonnet: Live-SWE-Agent scaffold 45.8 percent, CCA scaffold 52.7 percent.
- "even a weaker model equipped with a strong agent scaffold (Claude 4.5 Sonnet + CCA at 52.7%) can outperform a stronger model (Claude 4.5 Opus + Anthropic's proprietary scaffold at 52.0%)."

**r/AI_Agents, "The agent harness matters more than the model you pick", 22 July 2026.**
<https://www.reddit.com/r/AI_Agents/comments/1v3h4xo/the_agent_harness_matters_more_than_the_model_you/>

- "The sharpest illustration we have seen of this is SWE-bench Verified: same model, same task set, but swapping the published harness produces a 34 to 48 point spread."
- Note: this is an unverified community claim. Use only as an illustration of what practitioners say, not as a measurement.

**@omarsar0 on X (Elvis Saravia, DAIR.AI), 4 August 2026.**
<https://x.com/omarsar0/status/2084714744880173451>

- "Picking the right agent harness is now a crucial skill for any AI engineer. Imagine using the same model, same task, and same prompt. Now move it between two agent harnesses and the cost per success can swing by 5 to 30x."
- Note: the URL returned 404 to a direct fetch on 5 August 2026. The corroborating record is this repository's own capture at `agent-harness-engineering-matters-more-than-the-model-raw-harness.md`, line 528.
- Quote boundaries matter here. "the cost per success by 5 to 30x" is not a contiguous span in the source. Quote from "move it between two agent harnesses" onward.

**@beamnxw on X, 25 July 2026.**
<https://x.com/beamnxw/status/2081044232479928709>

- "Harness > Everything around the model: tools, state, permissions, memory, sandboxes, retries, observability. A model can only be as reliable as the environment it runs in."

**r/LLMObservability, "The harness around the model decides more of your agent's behaviour than the model does", 3 August 2026.**
<https://www.reddit.com/r/LLMObservability/comments/1vedbnq/the_harness_around_the_model_decides_more_of_your/>

- "Most of the agent behaviour I have had to fix was not the model being dumb. It was the scaffolding: how tools were described, what got put back into context after a failure, how many steps were allowed, what happened on a timeout."

## The counterargument

**Hugo Bowne-Anderson, "Stop Overengineering Your Agent Harness", O'Reilly Radar, 22 July 2026.**
<https://www.oreilly.com/radar/stop-overengineering-your-agent-harness/>

- "Most agents you'll build don't need any of them." (compaction, memory, handoffs, sub-agents)
- "Few systems need all of it and the right harness depends on the job."
- "If both are low, keep the harness small... add infrastructure only when a real failure demands it."
- Concedes that coding agents, deep research systems and long-lived assistants are exactly the cases where harness work does pay off.

## Done is not done

**Qwen Team, "The Verification Horizon: No Silver Bullet for Coding Agent Rewards", 29 June 2026.**
<https://arxiv.org/html/2606.26300v2>

- "A classical intuition in computing holds that verifying a solution is easier than finding one. For today's coding agents, this asymmetry is reversing."
- "Every verifier we can build is only a proxy for human intent, never the intent itself."
- "When a proxy serves as a reward signal, the generator learns not only to satisfy the proxy but also to exploit the divergence between proxy and intent."
- "no fixed reward function can remain effective as policy capability continues to grow; and verification must co-evolve with the generator."
- Measured: hacked-resolved rate fell from 28.57 percent to 0.56 percent with behaviour monitoring; clean resolved rate rose from 40.22 percent to 60.53 percent across three SWE-Bench variants.

**Bingchen Zhao, Dhruv Srikanth, Yuxiang Wu, Zhengyao Jiang (Weco AI), "SpecBench: Measuring Reward Hacking in Long-Horizon Coding Agents", 20 May 2026.**
<https://arxiv.org/html/2605.21384v1>

- "every frontier agent saturates the visible suite, reward hacking persists."
- "the gap grows by 28 percentage points for every tenfold increase in code size."
- "among tasks over 25K LOC, it reaches 100pp." Note: this describes the worst-case gap, not the gap generally. Keep the qualifier.

**Naman Jain, "Reward hacking is swamping model intelligence gains", Cursor, 25 June 2026.**
<https://cursor.com/blog/reward-hacking-coding-benchmarks>

- "Opus 4.8 Max fell from 87.1% to 73.0%" on SWE-bench Pro.
- What changed: the authors sealed git history by removing and reinitialising the `.git` directory, and restricted internet access with an egress proxy that blocked everything except whitelisted package registries.
- Fetched and confirmed at the primary source on 5 August 2026.

**Mario Zechner, Pi. Quotes read from the source tree at version 0.83.0.**
<https://github.com/earendil-works/pi>

- `packages/coding-agent/README.md:15` reads "Pi is a minimal terminal coding harness".
- `README.md:37-45` reads "Pi does not include a built-in permission system for restricting filesystem, process, network, or credential access" and "By default, it runs with the permissions of the user and process that launched it."
- The philosophy section at `packages/coding-agent/README.md:492-508` lists the omissions: no MCP, no sub-agents, no permission popups, no plan mode, no built-in to-dos, no background bash.

**Simon Willison, "First run the tests", 24 February 2026 (modified 28 February 2026).**
<https://simonwillison.net/guides/agentic-engineering-patterns/first-run-the-tests/>

- "If the code has never been executed it's pure luck if it actually works when deployed to production."
- Tests are "vital for ensuring AI-generated code does what it claims to do."

**Simon Willison, "Anti-patterns: things to avoid", 4 March 2026.**
<https://simonwillison.net/guides/agentic-engineering-patterns/anti-patterns/>

- "Don't file pull requests with code you haven't reviewed yourself."

## What operators actually do

**Anthropic, "2026 Agentic Coding Trends Report".**
<https://resources.anthropic.com/hubfs/2026%20Agentic%20Coding%20Trends%20Report.pdf>

- "developers use AI in roughly 60% of their work, they report being able to 'fully delegate' only 0-20% of tasks."
- "AI serves as a constant collaborator, but using it effectively requires thoughtful set-up and prompting, active supervision, validation, and human judgment - especially for high-stakes work."
- "Engineers describe using AI for tasks that are easily verifiable, well-defined, or repetitive, while keeping high-level design decisions and anything requiring organizational context or 'taste' for themselves."
- Engineers delegate tasks where they "can relatively easily sniff-check on correctness."
- "The shift is from writing code to reviewing, directing, and validating AI-generated code."
- "humans are still reviewing the code. It's not 'fully delegated' but highly collaborative."

**"25 Patterns in Agentic Engineering", SpecStory Press, 2026.**
<https://specstory.com/books/25-patterns-in-agentic-engineering-book-2026.pdf>

The title is exactly "25 Patterns in Agentic Engineering". The local repository
directory is named `extract-agentic-engineering`, and "Extract" is not part of
the title. Drawn from 1,310 captured sessions and 4,670 commits between September
2025 and May 2026.

- Thesis, from the introduction: "Once the code is cheap, the work is the intent behind it and the proof that it holds."
- Pattern 1: "A claim with no cited artifact is unverified by default."
- Pattern 2: "If the answer to 'accurate against what?' is another thing the agent generated, you have asked the witness to corroborate the witness."
- Pattern 13: "The agent never gets to feel finished; it gets a falsifiable definition of finished." The same pattern calls the self-grading spec "a power-move, not the default. Across 1,310 sessions, exactly four goal documents exist."
- Evidence block for pattern 1, February 2026: the agent's reasoning read "The user is rightly questioning my claim that Helicone's cost is accurate. I didn't actually verify it — I just assumed it."
- Corpus counts: 614 occurrences of "Request interrupted by user" across 184 transcripts, 335 declined tool uses, 441 turns opening with a word from the list no, wait, actually, stop, revert, undo, and one January 24 session taking 33 interrupts.
- The "Do not edit files" count has two measurements over the same corpus. The published figure is 88. A separate research note counts 91 across 52 transcripts. Say which one you are using.

**Paweł Józefiak, "Claude Code vs Codex CLI vs Aider vs OpenCode vs Pi vs Cursor", 15 April 2026.**
<https://thoughts.jock.pl/p/ai-coding-harness-agents-2026>

- On Pi: "Pi is genuinely different from everything else here, and I want to say this upfront: I like it." Tagline quoted as "there are many coding agents, but this one is mine."
- On Pi billing: "Pi is BYOM, bring your own API key... usage through Pi counts against API billing, not your Claude subscription."
- On unattended work: "Claude Code is the only tool built end-to-end for that."
- On losing coherence: "It handles step one well, sometimes step two, and starts losing coherence by step three or four."
- On long sessions: "Context loss on sessions longer than 2 hours, where it starts forgetting early decisions."

## Unverified, do not assert

- A secondary blog (claudehub.fr) attributes "I'm not reviewing that code" to Simon Willison, May 2026. This was not found on simonwillison.net during this research. Do not use.
- The r/AI_Agents "34 to 48 point spread" figure has no primary source. Treat as community claim only.
