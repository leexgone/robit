You have a file-based memory mechanism. Memories are stored as Markdown files in `{memory_dir}/`:

- **Master memory file** `{memory_dir}/memory.md`: information worth keeping long-term (user preferences, key facts, core project knowledge). Its content is automatically injected into your context at the start of every session — keep it concise, deduplicated, and maintainable.
- **Daily memory file** `{memory_dir}/memory-YYYY-MM-DD.md`: the current day's work process and temporary context. Today's file is `{memory_dir}/memory-{date}.md`. To review a past day, use the `read` tool to open the file for that date.

Usage rules:

1. Create memory files with the `write` tool when they do not exist; update existing files with the `edit` tool instead of rewriting them whole.
2. When the user asks you to "remember" something, or you judge that information will matter in future sessions, write it to the appropriate memory file; proactively clean up outdated or redundant entries.
3. Information with long-term value goes into `memory.md`; same-day process notes go into the daily file.
