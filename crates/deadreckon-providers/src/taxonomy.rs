pub fn normalize_tool_category(raw_name: &str) -> &'static str {
    match raw_name {
        "Read" => "Read",
        "Edit" => "Edit",
        "Write" | "NotebookEdit" => "Write",
        "Bash" => "Bash",
        "Grep" => "Grep",
        "Glob" => "Glob",
        "Task" | "Agent" => "Task",
        "Skill" => "Tool",
        "shell_command" | "exec_command" | "write_stdin" | "shell" => "Bash",
        "apply_patch" => "Edit",
        "spawn_agent" => "Task",
        "read_file" | "list_directory" => "Read",
        "write_file" => "Write",
        "edit_file" | "replace" => "Edit",
        "run_command" | "execute_command" | "run_shell_command" => "Bash",
        "search_files" | "grep" | "grep_search" => "Grep",
        "read" => "Read",
        "edit" => "Edit",
        "write" => "Write",
        "bash" => "Bash",
        "glob" => "Glob",
        "task" => "Task",
        "view" => "Read",
        "report_intent" => "Tool",
        "Shell" => "Bash",
        "StrReplace" => "Edit",
        "LS" => "Read",
        "create_file" => "Write",
        "look_at" => "Read",
        "undo_edit" => "Edit",
        "finder" => "Grep",
        "read_web_page" => "Read",
        "skill" => "Tool",
        "find" => "Read",
        "str_replace" => "Edit",
        "exec" | "process" => "Bash",
        "browser" | "web_search" | "web_fetch" | "image" | "canvas" | "tts" | "message"
        | "nodes" => "Tool",
        "sessions_list" | "sessions_history" | "sessions_send" | "sessions_spawn" | "subagents"
        | "agents_list" | "session_status" => "Task",
        "fs_search" => "Grep",
        "patch" | "multi_patch" | "undo" | "remove" => "Edit",
        "fetch" => "Read",
        "todo_write" | "todo_read" => "Tool",
        "parallel" => "Task",
        "terminal" => "Bash",
        "browser_navigate" | "browser_snapshot" | "browser_click" | "browser_type"
        | "browser_scroll" | "browser_press" | "browser_back" | "browser_close"
        | "browser_vision" | "browser_console" | "browser_get_images" => "Tool",
        "vision_analyze" => "Read",
        "delegate_task" => "Task",
        "execute_code" => "Bash",
        "todo" | "memory" | "session_search" | "skill_view" | "skills_list" | "skill_manage"
        | "clarify" | "text_to_speech" | "cronjob" => "Tool",
        "ReadFile" => "Read",
        "WriteFile" => "Write",
        "EditFile" => "Edit",
        "RunTerminalCommand" => "Bash",
        "LaunchSubagent" => "Task",
        "WebFetch" | "WebSearch" => "Tool",
        "TodoWrite" | "AskUserQuestion" | "ProposePlanToUser" => "Tool",
        "subagent__ZencoderSubagent" => "Task",
        "zencoder-rag-mcp__web_search" => "Read",
        "code_interpreter" => "Bash",
        "read_files" => "Read",
        "apply_file_diff" => "Edit",
        "search_codebase" => "Grep",
        "call_mcp_tool"
        | "read_mcp_resource"
        | "suggest_plan"
        | "suggest_create_plan"
        | "use_computer" => "Tool",
        "write_to_long_running_shell_command" => "Bash",
        "read_shell_command_output" => "Read",
        raw if raw.contains("subagent") => "Task",
        _ => "Other",
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_tool_category;

    #[test]
    fn taxonomy_matches_agentsview_core_cases() {
        for (raw, category) in [
            ("Read", "Read"),
            ("exec_command", "Bash"),
            ("apply_patch", "Edit"),
            ("spawn_agent", "Task"),
            ("read_file", "Read"),
            ("write_file", "Write"),
            ("edit_file", "Edit"),
            ("run_shell_command", "Bash"),
            ("grep_search", "Grep"),
            ("read", "Read"),
            ("bash", "Bash"),
            ("view", "Read"),
            ("RunTerminalCommand", "Bash"),
            ("WebSearch", "Tool"),
            ("code_interpreter", "Bash"),
        ] {
            assert_eq!(normalize_tool_category(raw), category, "{raw}");
        }
    }

    #[test]
    fn taxonomy_subagent_names_map_to_task() {
        assert_eq!(
            normalize_tool_category("mcp__zen_subagents__spawn_subagent"),
            "Task"
        );
    }
}
