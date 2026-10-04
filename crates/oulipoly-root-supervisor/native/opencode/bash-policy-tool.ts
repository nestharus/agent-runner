// Native permission gate for the agent-bash OpenCode `bash` tool, loaded by
// a root's native OpenCode host as `<config dir>/tool/bash.ts`.
//
// - OpenCode 1.18.30 asks native permission for its built-in `bash` and for
//   MCP tools, never for a custom tool: an unwrapped agent-bash `bash` would
//   run whatever the configured `bash` permission says (only a `"*": "deny"`
//   hides it).
// - This file registers as `bash` (a config directory's `tool/<name>.ts`
//   default export) and so replaces the built-in. Before anything runs it
//   asks the native `bash` permission for the exact command (a handle poll:
//   `handle <handle>`), evaluated by OpenCode against the configured
//   ruleset:
//   - allow: runs, no request;
//   - deny: native denial, nothing runs, no request;
//   - ask: a native permission request, which the ACP v2 endpoint forwards
//     to the owner; only a selected allow_once runs it, once.
// - Then it runs the unmodified agent-bash tool, copied beside this
//   directory as `../agent-bash/bash.ts`; its description and arguments are
//   that tool's own. Inside a root, that tool runs through the root's own
//   Bash ingress (`OULIPOLY_ROOT_BASH_V1`) only.
// - `always` names only the exact pattern asked for.

import { tool } from "@opencode-ai/plugin"
import agentBash from "../agent-bash/bash.ts"

export default tool({
  description: agentBash.description,
  args: agentBash.args,
  async execute(args, context) {
    const pattern = args.command ?? `handle ${args.handle ?? ""}`
    await context.ask({
      permission: "bash",
      patterns: [pattern],
      always: [pattern],
      metadata: { command: args.command, handle: args.handle, delivery: args.delivery, workdir: args.workdir },
    })
    return agentBash.execute(args, context)
  },
})
