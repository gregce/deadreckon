use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProofBlock {
    pub proof_path: PathBuf,
    pub story_path: PathBuf,
    pub lineage: String,
    pub next_command: String,
}

impl ProofBlock {
    pub fn render_text(&self) -> String {
        format!(
            "gate: SIGNED by dr-gate — the agent could not have written this\nproof:  {}\nstory:  {}\nlineage: {}\n→ {}\n",
            self.proof_path.display(),
            self.story_path.display(),
            self.lineage,
            self.next_command
        )
    }

    pub fn render_lines(&self) -> Vec<String> {
        self.render_text()
            .trim_end_matches('\n')
            .lines()
            .map(str::to_string)
            .collect()
    }
}
