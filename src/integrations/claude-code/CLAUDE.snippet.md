## Delegating with Hufu

This machine has Hufu installed. To hand work to another agent, use the `hufu_*` MCP tools (or the
`hufu` command line): `hufu_agents` to see who is available, `hufu_submit` to start a background
job, `hufu_wait` or `hufu_status` to follow it, `hufu_result` to read the outcome. Ask for the
lowest permission tier that works, give every write task an acceptance check, and read both
`state` and `verdict` before trusting a result.
