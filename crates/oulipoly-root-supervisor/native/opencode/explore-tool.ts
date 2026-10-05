// The parent's `explore` tool for a root's native OpenCode host, loaded as
// `<config dir>/tool/explore.ts` only when the root's intent allows
// registered children. It asks the root's owner for one registered child
// (`../explore-client.mjs`) from inside this OpenCode process, so the
// request is attributed to this host's exact live work. The routes and
// limits it describes are the launch's (`../explore.json`); the owner
// enforces them. Native permission: the root's config names `explore`
// as allowed; nothing else is widened.

import { tool } from "@opencode-ai/plugin"
import { readFileSync } from "node:fs"
import { exploreDescription, exploreRequest, pickRoute } from "../explore-client.mjs"

const config = JSON.parse(readFileSync(new URL("../explore.json", import.meta.url), "utf8"))

export default tool({
  description: exploreDescription(config),
  args: {
    question: tool.schema.string().describe("the orientation question for the explorer"),
    route: tool.schema.string().describe("the explorer route (default: the only one)").optional(),
  },
  async execute(args, context) {
    const result = await exploreRequest(pickRoute(config, args.route), args.question, { signal: context.abort })
    return result.text
  },
})
