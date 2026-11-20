# Control responses captured from the real CLI

Captured from Claude Code 2.1.329 on Linux on 2026-11-20, by sending the
control request over stdin after `initialize`, with no prompt (no API call).
Paths are replaced with `/probe/` and `/home/user`.

- `get_context_usage.json`: the answer to `get_context_usage`.
- `mcp_status.json`: the answer to `mcp_status` with one stdio server listing
  two valid tools.
- `mcp_status_rejected_tools.json`: the same server, where one of the two tools
  lists `"inputSchema": {}`. The CLI drops every tool of the server, reports it
  `connected`, and gives no error.
