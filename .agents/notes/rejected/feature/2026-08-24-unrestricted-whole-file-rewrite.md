# Unrestricted Whole-File Rewrite as Primary Editing Tool

Status: rejected — Full-file overwrites frequently drop unrelated code during long-context edits

## Context

Coding agents require tools to edit source code. An intuitive baseline is providing a `write_file` tool that replaces the entire file contents with new text.

## Rejected Proposal

Use full-file overwriting (`write_file`) as the primary code modification mechanism across the workspace.

## Rationale for Rejection

1. **Unrelated Code Loss**: Experimental validation ([docs/experiments/edit-surface.md](file:///D:/gh-ws/codex-ws/mini-codex/docs/experiments/edit-surface.md)) showed that while whole-file rewrites and precise search-and-replace both reach target changes in three steps, full-file rewrites frequently drop unrelated lines, comments, and imports in larger files.
2. **Standard Separation**: The workspace strictly exposes `edit_file` (unique string match replacement) for modifications, and reserves `write_file` exclusively for creating new files or explicit full rewrites.
