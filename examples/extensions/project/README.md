# Complete extension layout

This directory mirrors a real Mini Agent Harness workspace. Its `.agents/`
tree includes one workspace skill, one installed plugin, and one standalone MCP
server.

Run `mini-agent` from this directory after configuring provider environment
variables. Use `/status` to inspect the loaded runtime and extension summary;
this does not connect to MCP servers until a turn uses them.
