// Client of a root's registered children, shared by the native Claude
// receiver's `explore` MCP tool and the native OpenCode `explore` tool. It
// runs inside the parent harness process, so the root's owner attributes
// the request to that harness's exact live work (a model's shell text
// cannot reach it; a Bash run's namespace is refused).
//
// One request line on the root's ingress (`OULIPOLY_ROOT_BASH_V1`):
// `{"v":1,"op":"child","route":R,"prompt":Q}`, then the connection stays
// open while the child lives: the owner stops the child if it closes. The
// owner answers with stage lines and ends with `result` (or `refused`).
// Nothing here retries, falls back or starts anything else.

import { createConnection } from "node:net"

const LINE_LIMIT = 16 * 1024 * 1024
const ANSWER_LIMIT = 256 * 1024

export const EXPLORE_TOOL_PURPOSE =
  "Ask a registered read-only explorer (a cheap child session inside this same root) one orientation question: " +
  "where things are and how they are wired together (files, modules, call paths, configuration, history). " +
  "Blocks until the child answers or ends, then returns its answer here. Use the answer to decide which areas to " +
  "inspect yourself; it is orientation, not validation of your work, and it may be wrong. Several calls may run in " +
  "parallel within the root's limits. Children cannot start children."

export function exploreDescription(config) {
  const routes = Array.isArray(config?.routes) ? config.routes : []
  return `${EXPLORE_TOOL_PURPOSE} Routes: ${routes.join(", ") || "none"} (default ${routes[0] ?? "none"}). ` +
    `Limits per root: ${config?.max_starts ?? "?"} child starts in total, ${config?.max_concurrent ?? "?"} at once; ` +
    "a refusal or failure is returned visibly and nothing is retried."
}

// The route a call names, or the only one configured.
export function pickRoute(config, route) {
  const routes = Array.isArray(config?.routes) ? config.routes : []
  if (route) return route
  return routes.length === 1 ? routes[0] : undefined
}

function stageText(stage) {
  switch (stage.event) {
    case "accepted":
      return `accepted(${stage.child}, starts ${stage.starts?.used}/${stage.starts?.max}, live ${stage.concurrent?.live}/${stage.concurrent?.max})`
    case "started":
      return `started(work=${stage.work})`
    case "turn-end":
      return `turn-end(${stage.stop_reason})`
    case "stopping":
      return `stopping(${stage.reason})`
    case "end":
      return `end(${stage.status}, namespace ${stage.namespace?.drained === true ? "drained" : "drain not reported"})`
    case "end-unknown":
      return `end-unknown(${stage.reason})`
    case "launch-failed":
    case "launch-unknown":
      return `${stage.event}(${stage.reason})`
    case "agent-message":
    case "notice":
      return undefined
    default:
      return stage.event
  }
}

// Renders what the owner's lines established; `lost` says why the stream
// ended early, if it did.
export function renderChild(stages, lost) {
  const last = stages.at(-1)
  const accepted = stages.find((stage) => stage.event === "accepted")
  const lifecycle = stages.map(stageText).filter(Boolean).join(" -> ") || "none"
  const notices = stages
    .filter((stage) => stage.event === "notice" && stage.severity !== "info")
    .map((stage) => `${stage.severity}: ${stage.title}${stage.description ? ` (${String(stage.description).slice(0, 500)})` : ""}`)
  const noticeText = notices.length ? `\nNotices: ${notices.join("; ")}` : ""
  if (last?.event === "refused")
    return { text: `Explorer request refused by this root (${last.reason}). No child was started. Not retried.`, isError: true }
  if (last?.event !== "result") {
    if (!accepted)
      return { text: `Explorer request outcome unknown (${lost ?? "no reply"}): the root's reply was lost before admission was confirmed, so a child may or may not have been admitted. Do not retry automatically.`, isError: true }
    return { text: `Explorer ${accepted.child} was admitted, but its result was not received (${lost ?? "stream ended"}): it may have run. The root stops a child whose requester goes away. Do not retry automatically.\nLifecycle: ${lifecycle}${noticeText}`, isError: true }
  }
  const answer = typeof last.answer === "string"
    ? (last.answer.length > ANSWER_LIMIT ? `${last.answer.slice(0, ANSWER_LIMIT)}\n[answer truncated by this tool]` : last.answer)
    : undefined
  const head = `Explorer ${last.child} (route ${last.route}): ${last.outcome}` +
    (last.stopped ? `; stopped by the root (${last.stopped})` : "") + "."
  const life = last.lifecycle
  const status = life
    ? `\nLifecycle status: end ${life.end}; its Bash runs still open at this result: ${life.bash_runs_open}` +
      (life.bash_run_end_unknown ? " (one ended unknown)" : "") + `; root budget: ${life.budget}. ` +
      "The outcome above is about the answer's content only; it does not prove the child or its Bash ended or drained."
    : "\nLifecycle status: not reported by the root (end and drain unknown to this tool)."
  const body = answer !== undefined
    ? `\nAnswer (the child's last reply before its turn ended; verify what matters yourself):\n${answer}`
    : "\nNo answer was received from the child."
  return {
    text: `${head}${body}${status}\nLifecycle: ${lifecycle}${noticeText}`,
    // Error/success concerns the answer only (see the status line).
    isError: last.outcome !== "answered",
  }
}

// Asks one child and resolves with the rendered result. `signal` (an
// AbortSignal) closes the connection, which makes the owner stop the child.
export function exploreRequest(route, question, { socketPath = process.env.OULIPOLY_ROOT_BASH_V1, signal } = {}) {
  return new Promise((done) => {
    if (!socketPath) {
      done({ text: "No root ingress (OULIPOLY_ROOT_BASH_V1 is not set): nothing was asked and no child started.", isError: true })
      return
    }
    if (!route) {
      done({ text: "No explorer route named and more than one is configured: nothing was asked.", isError: true })
      return
    }
    const stages = []
    let buffer = ""
    let finished = false
    let requested = false
    let lost
    const socket = createConnection(socketPath)
    const finish = () => {
      if (finished) return
      finished = true
      socket.destroy()
      done(renderChild(stages, lost))
    }
    const abort = () => {
      lost = "aborted by this agent; the connection was closed, so the root stops the child"
      finish()
    }
    if (signal?.aborted) {
      socket.destroy()
      done({ text: "Explorer request aborted before it was sent: nothing was asked.", isError: true })
      return
    }
    signal?.addEventListener?.("abort", abort, { once: true })
    socket.setEncoding("utf8")
    socket.on("connect", () => {
      // From here the request may reach the owner: a later loss is unknown.
      requested = true
      socket.write(JSON.stringify({ v: 1, op: "child", route, prompt: question }) + "\n")
    })
    socket.on("data", (chunk) => {
      buffer += chunk
      if (buffer.length > LINE_LIMIT) {
        lost = "reply line larger than this tool's bound"
        finish()
        return
      }
      let at
      while ((at = buffer.indexOf("\n")) >= 0) {
        const line = buffer.slice(0, at)
        buffer = buffer.slice(at + 1)
        if (!line.trim()) continue
        let stage
        try {
          stage = JSON.parse(line)
        } catch {
          lost = "malformed reply line"
          finish()
          return
        }
        stages.push(stage)
        if (stage.event === "result" || stage.event === "refused") {
          finish()
          return
        }
      }
    })
    socket.on("error", (error) => {
      lost = stages.length ? `connection error ${error?.code ?? error}` : `owner unreachable (${error?.code ?? error})`
      if (!requested) {
        finished = true
        done({ text: `Explorer request not delivered: ${lost}. Nothing was admitted by this call that this tool knows of; do not retry automatically.`, isError: true })
        return
      }
      finish()
    })
    socket.on("close", () => {
      lost = lost ?? "connection closed by the root"
      finish()
    })
  })
}
