# Complete extension layout

This directory mirrors a real Mini Agent Harness workspace. Its `.agents/`
tree includes one direct skill, one cloned-style skill collection, one installed
plugin, one local marketplace with an explicitly selected skill, and one
standalone MCP server.

Run `mini-agent doctor` from this directory after configuring provider
environment variables. Doctor validates discovery without connecting to the
model provider or MCP servers.
