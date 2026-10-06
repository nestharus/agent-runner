// ACP v2 agent receiver for one native Claude Code harness of a root
// supervisor (`"endpoint": "stdio"`). The owner speaks ND-JSON ACP v2 on
// this process's stdin/stdout; this process drives the unmodified Claude
// Code executable through the published Claude Agent SDK (`query()` with
// streaming input). Nothing else may write to stdout; diagnostics go to
// stderr.
//
// - Launch: `node acp-v2-receiver.mjs <launch.json>`, written by the owner
//   setup (crate `native_claude` module). It names the Claude Code
//   executable, model, effort, the Claude config directory (the work
//   user's own store; never read here), the agent-bash binary, the `bash`
//   policy and the built-in tools selected.
// - Configuration in code, not from the user's files: no user, project or
//   local settings (so no hooks, plugins, CLAUDE.md or settings MCP), only
//   this process's in-process MCP server (strict), permission mode
//   `dontAsk` (anything not pre-approved is denied, never asked), built-in
//   execution and delegation tools absent. The Claude Code process gets
//   this process's environment less every inherited `ANTHROPIC_*` and
//   `CLAUDE*` name (API keys, tokens, provider and endpoint redirects), plus
//   the launch's own settings; its login stays the store's own.
// - `bash` is one in-process MCP tool (`mcp__oulipoly__bash`). It runs a
//   command only through `agent-bash run --delivery sync` (or `async` with
//   `background: true`) and so through the root's own Bash ingress
//   (`OULIPOLY_ROOT_BASH_V1`): attributed, durably recorded, killed on
//   cancel. A background run's end reaches this session later as an owner
//   prompt (a new turn), never inside a busy turn and never polled. With an
//   allow list, a command not named exactly is refused here and nothing runs.
// - `explore` (`mcp__oulipoly__explore`), only when the launch names
//   explorer routes: asks the root's owner for one registered read-only
//   child through `explore-client.mjs` from inside this process (attributed
//   to this harness's work), blocks until its result, and returns the
//   child's answer and lifecycle. The owner enforces routes, budget and
//   depth; closing the connection (an aborted call) stops the child.
// - session/new starts Claude Code and answers once its control handshake
//   completed (bounded). One session per process; session/resume is refused.
// - session/prompt sends one user message with a fresh random uuid and a
//   minted, ascending `messageId`. It answers `{ messageId }` only once
//   Claude Code consumed that message: its replay echo
//   (`--replay-user-messages`), or a reply or result that names its uuid.
//   Consumption is not processing. No echo within the bound fails the
//   prompt (no ACK) and ends this process: a visible bounded end, not a
//   silent wait to the caller's deadline, and never a replay.
// - The turn: each main-thread assistant message with text becomes one
//   `agent_message` (its own ascending messageId) whose `_meta`
//   "oulipoly.ai/parentMessageId" is the input the turn answers, taken only
//   from Claude Code's explicit `user_message_uuid(s)` mapped back to this
//   receiver's messageId; never from position or uuid order. The result
//   ends the turn: one `state_update` idle tagged
//   "oulipoly.ai/lastUserMessageId" with the latest input it answered, and
//   a stop reason: `end_turn` (success without error), `max_tokens`,
//   `refusal`, or `_claude_<reason>`. A result answering no known input
//   while an acknowledged input is open ends that input visibly as
//   `_claude_unattributed` with an error notice; otherwise an untagged idle
//   is readiness only.
// - Visible outcomes as notices: the CLI's init (version, model,
//   capabilities, tools, MCP status; warning on a model other than the one
//   asked for), assistant errors (e.g. `model_not_found`,
//   `authentication_failed`), permission denials, result errors.
// - If Claude Code ends or the SDK fails, every acknowledged open input is
//   ended `_claude_exited`, unacknowledged prompts fail, and this process
//   exits 3. If the owner's connection closes, it closes Claude Code and
//   exits 0. Nothing here retries or replays.

import { spawn } from "node:child_process"
import { randomUUID } from "node:crypto"
import { readFileSync } from "node:fs"
import { Readable, Writable } from "node:stream"
import { agent, methods, ndJsonStream, PROTOCOL_VERSION } from "@agentclientprotocol/sdk/experimental/v2"
import { createSdkMcpServer, query, tool } from "@anthropic-ai/claude-agent-sdk"
import { z } from "zod"
import { exploreDescription, exploreRequest, pickRoute } from "./explore-client.mjs"

// Unit tests import this module's pure functions without a launch.
const MAIN = process.env.OULIPOLY_CLAUDE_RECEIVER_NO_MAIN !== "1"
const config = MAIN ? JSON.parse(readFileSync(process.argv[2], "utf8")) : {}
const START_MS = config.start_timeout_s * 1000
const ACK_MS = config.ack_timeout_s * 1000
const BASH_TOOL = "mcp__oulipoly__bash"
const EXPLORE_TOOL = "mcp__oulipoly__explore"
// The receiver's own tools this launch offers.
const OWN_TOOLS = MAIN && config.explore?.routes?.length ? [BASH_TOOL, EXPLORE_TOOL] : [BASH_TOOL]
// Never offered, whatever `tools` says: execution, delegation, background
// and network tools of the built-in set.
const DENIED_TOOLS = [
  "Agent", "Task", "Bash", "BashOutput", "KillShell", "Monitor", "TaskOutput", "TaskStop",
  "WebFetch", "WebSearch", "Skill", "NotebookEdit", "PowerShell", "Glob", "Grep",
]
const OUTPUT_LIMIT = 64 * 1024 * 1024
// Claude can replace an oversized MCP result with a spill-file error,
// including its wait receipt. Keep a useful encoded payload prefix here;
// this is a presentation policy, not a claim about the SDK's token limit.
const BASH_PAYLOAD_TEXT_BYTES = 4096
const ROOT_V1_SURFACE = "agent-bash-root-v1"
const ROOT_V1_OUTCOMES = ["refused", "not-started", "ended", "ended-output-unproven", "unknown"]

function log(line) {
  process.stderr.write(`[claude-acp-v2] ${new Date().toISOString()} ${line}\n`)
}

// Ascending ids: fixed-width hex of max(ms * 4096, previous + 1), so a later
// id always compares greater as a string, even within one millisecond.
let lastId = 0n
export function mintId(prefix = "msg_") {
  const next = BigInt(Date.now()) * 4096n
  lastId = next > lastId ? next : lastId + 1n
  return prefix + lastId.toString(16).padStart(16, "0")
}

// The environment Claude Code gets: this process's, less inherited Claude
// and Anthropic configuration and credentials, plus the launch's own.
export function claudeEnv(env, launch) {
  const out = {}
  for (const [name, value] of Object.entries(env)) {
    if (/^(ANTHROPIC_|CLAUDE)/.test(name)) continue
    if (/^(OULIPOLY_|AGENT_BASH_|NODE_|OTEL_)/.test(name)) continue
    if (["AWS_BEARER_TOKEN_BEDROCK", "ENABLE_TOOL_SEARCH", "DEBUG_CLAUDE_AGENT_SDK", "DEBUG"].includes(name)) continue
    out[name] = value
  }
  return { ...out, ...launch }
}

// Renders only what the root's stage lines established (the agent-bash
// root v1 result object); the wait status is read from it, never from
// agent-bash's own exit status.
export function renderRootV1(exitCode, stdout, stderr) {
  const unresolved = (reason) =>
    `Root v1 result unresolved (${reason}); the command may have run; do not replay.\n` +
    `exit: ${exitCode}\nstdout: ${stdout.slice(0, 4096)}\nstderr: ${stderr.slice(0, 4096)}`
  if (exitCode !== 0) return { text: unresolved("agent-bash did not complete its result"), error: true }
  let value
  try {
    value = JSON.parse(stdout)
  } catch {
    return { text: unresolved("result is not one JSON object"), error: true }
  }
  const output = value?.output
  if (value?.result_surface !== ROOT_V1_SURFACE || value.version !== 1 || value.delivery_mode !== "sync" ||
      !ROOT_V1_OUTCOMES.includes(value.outcome) || typeof value.effects_possible !== "boolean" ||
      !Array.isArray(value.stages) || !Array.isArray(value.faults) || !output ||
      typeof output.base64 !== "string" || !Number.isSafeInteger(output.bytes) || output.bytes < 0) {
    return { text: unresolved("result surface invalid"), error: true }
  }
  const bytes = Buffer.from(output.base64, "base64")
  // The requester counts the drained stream but carries only a prefix.
  // Empty refusal/no-start results have no presentation fields.
  const hasPresentation = ["presented_bytes", "omitted_bytes", "remainder"].some((key) => key in output)
  const presented = hasPresentation ? output.presented_bytes : output.bytes
  const omitted = hasPresentation ? output.omitted_bytes : 0
  if (!Number.isSafeInteger(presented) || presented < 0 ||
      !Number.isSafeInteger(omitted) || omitted < 0 || presented > output.bytes ||
      omitted !== output.bytes - presented || bytes.length !== presented ||
      bytes.toString("base64") !== output.base64 ||
      (hasPresentation && (omitted ? !["discarded", "retained-by-owner", "partial-owner-retention"].includes(output.remainder) : output.remainder !== "none")))
    return { text: unresolved("output length or encoding mismatch"), error: true }
  const ended = value.outcome === "ended" || value.outcome === "ended-output-unproven"
  if (ended !== (value.wait !== null && typeof value.wait === "object") ||
      value.effects_possible !== !["refused", "not-started"].includes(value.outcome))
    return { text: unresolved("result outcome inconsistent"), error: true }
  const delivery = value.outcome === "ended" ? (omitted ? "partial" : "complete")
    : ["refused", "not-started"].includes(value.outcome) ? "none" : "unproven"
  if (output.delivery !== delivery ||
      (delivery === "none" && output.bytes !== 0))
    return { text: unresolved("output delivery inconsistent"), error: true }
  const stage = (s) => {
    const name = String(s?.event)
    if (name === "accepted") return `accepted(work=${s.work}, durable=${s.durable})`
    if (name === "output" && s.chunks !== undefined) return `output(chunks=${s.chunks}, bytes=${s.bytes})`
    if (name === "output-closed") return `output-closed(bytes=${s.bytes})`
    if (name === "end") return `end(${s.status}, observer=${s.observer}, output=${s.output?.state})`
    return s?.reason === undefined ? name : `${name}(${s.reason})`
  }
  const stages = `stages: ${value.stages.map(stage).join(" -> ") || "none"}`
  const faults = value.faults.length ? `\nfaults: ${value.faults.join("; ")}` : ""
  const text = bytes.toString("utf8")
  const utf8 = !bytes.includes(0) && Buffer.from(text, "utf8").equals(bytes)
  let shown = Math.min(bytes.length, utf8 ? BASH_PAYLOAD_TEXT_BYTES : BASH_PAYLOAD_TEXT_BYTES / 2)
  // The whole prefix is valid UTF-8 here. Back off if the cut would split
  // a character; binary output uses hex and budgets its 2x expansion.
  if (utf8 && shown < bytes.length) {
    while (shown > 0 && (bytes[shown] & 0xc0) === 0x80) shown--
  }
  const consumerOmitted = presented - shown
  const totalOmitted = output.bytes - shown
  const retention = renderRetention(output.retained, output.reference)
  const presentation = consumerOmitted
    ? `\nProducer output: ${presented} carried bytes of ${output.bytes} received; ${omitted} omitted; ` +
      (retention ? "owner retention described below." : omitted ? "remainder discarded, not retained." : "remainder none.") +
      `\nClaude presentation: ${shown} shown bytes of ${presented} producer-carried; ` +
      `${consumerOmitted} additionally omitted; ${totalOmitted} total omitted of ${output.bytes} received; ` +
      (retention ? "additional remainder not presented; see owner retention below." : "additional remainder not presented, not retained by this receiver.")
    : `\nOutput: ${presented} shown bytes of ${output.bytes} received; ${omitted} omitted; ` +
      (retention ? "owner retention described below." : omitted ? "remainder discarded, not retained." : "remainder none.")
  const prefix = bytes.subarray(0, shown)
  const body = `\n--- output (stderr joined; ${shown} shown bytes, ${utf8 ? "utf8" : "hex"}) ---\n` +
    (utf8 ? prefix.toString("utf8") : prefix.toString("hex"))
  switch (value.outcome) {
    case "refused":
      return { text: `Root v1 refused by ${value.refusal?.by} (${value.refusal?.reason})` +
        `${value.refusal?.detail ? `: ${value.refusal.detail}` : ""}. Nothing was run.\n${stages}`, error: true }
    case "not-started":
      return { text: `Root v1 accepted the command, then reported a positive no-start; nothing was run.\n${stages}${faults}`, error: true }
    case "unknown":
      return { text: `Root v1 outcome unknown (${value.meaning}): the command may have run. Do not replay.\n` +
        `${stages}${faults}${presentation}${retention}${bytes.length ? `${body}\n(output above is partial and unproven)` : ""}`, error: true }
    default: {
      const wait = value.wait.exit?.code !== undefined
        ? `exited with code ${value.wait.exit.code}`
        : `signaled with signal ${value.wait.exit?.signal}`
      const status = value.outcome === "ended"
        ? `output ${totalOmitted ? "partial" : "complete"} (full stream counted, closed, matched by the end)`
        : "output delivery unproven: the output below may be incomplete; do not replay"
      return { text: `Root v1 work ended: ${wait} (${value.wait.status}, observer ${value.wait.observer}); ` +
        `${status}.\n${stages}${faults}${presentation}${retention}${body}`, error: false }
    }
  }
}

// A background run's result: `running` is durable acceptance and start
// only; no wait, exit or output is known until its completion arrives as a
// later message in this conversation.
export function renderRootV1Async(exitCode, stdout, stderr) {
  const unresolved = (reason) => ({ text:
    `Root v1 background result unresolved (${reason}); the command may be running; do not replay.\n` +
    `exit: ${exitCode}\nstdout: ${stdout.slice(0, 2048)}\nstderr: ${stderr.slice(0, 2048)}`, error: true })
  if (exitCode !== 0) return unresolved("agent-bash did not complete its result")
  let value
  try { value = JSON.parse(stdout) } catch { return unresolved("result is not one JSON object") }
  if (value?.result_surface !== ROOT_V1_SURFACE || value.version !== 1 || value.delivery_mode !== "async" ||
      !Array.isArray(value.stages)) return unresolved("result surface invalid")
  const stages = `stages: ${value.stages.map(s => String(s?.event) + (s?.reason ? `(${s.reason})` : "")).join(" -> ") || "none"}`
  const reference = typeof value.output?.reference === "string" ? value.output.reference : undefined
  switch (value.outcome) {
    case "running":
      if (value.wait !== null || value.effects_possible !== true || !reference) return unresolved("async result inconsistent")
      return { text: `Root v1 background work accepted and started (reference=${reference}); it is still running. ` +
        "Its end (wait status, output facts and retained output identity) will arrive later in this conversation as a " +
        "separate message; do not poll for it. Nothing about its exit or output is known yet.\n" + stages, error: false }
    case "refused":
      return { text: `Root v1 refused background work (${value.refusal?.reason}). Nothing was run; not converted to synchronous.\n${stages}`, error: true }
    case "not-started":
      return { text: `Root v1 accepted the background command, then reported a positive no-start; nothing was run.\n${stages}`, error: true }
    case "unknown":
      return { text: `Root v1 background outcome unknown (${value.meaning}): the command may be running or may have run. ` +
        `Do not replay; a completion may or may not arrive.${reference ? ` reference=${reference}` : ""}\n${stages}`, error: true }
    default:
      return unresolved("async result outcome invalid")
  }
}

function renderRetention(record, reference) {
  if (!record) return reference ? `\nOwner output reference=${reference}; seal not observed. ` +
    `Use {output_identity: "${reference}"} to request retained bytes from the current owner; availability/loss may remain unknown.` : ""
  if (!["complete", "partial"].includes(record.state) || typeof record.identity !== "string") {
    return `\nOwner retention: ${record.state ?? "unknown"}; no recoverable identity. ${JSON.stringify(record.losses ?? record.reason ?? "")}`
  }
  return `\nOwner retention: ${record.state}, ${record.bytes} bytes; received=${record.received ?? "unknown"}; ` +
    `losses=${JSON.stringify(record.losses)}. identity=${record.identity}` +
    `\nRead with {output_identity: "${record.identity}", output_offset: 0, output_length: 1024}; continue at next_offset. ` +
    `Explicit local acceptance: {output_identity: "${record.identity}", accept_output: true}. ` +
    "Acceptance names retained bytes only; no input ACK, processing, remote settlement or drain. Retention ends with root/store retirement."
}

export function renderRootOutput(exitCode, stdout) {
  const unknown = { text: "Native output reply unresolved; local acceptance unconfirmed. No command replay.", error: true }
  if (exitCode !== 0) return unknown
  let value
  try { value = JSON.parse(stdout) } catch { return unknown }
  if (value.result_surface !== "agent-bash-root-v1-output" || value.version !== 1) return unknown
  if (["refused", "unknown"].includes(value.outcome)) {
    return { text: `Root v1 output ${value.outcome}: ${value.reason}. Local acceptance unconfirmed; no command replay.`, error: true }
  }
  const reply = value.reply
  if (value.outcome === "accepted") {
    return { text: `Root v1 exact local acceptance: ${reply.retained.identity}; durable=${reply.durable}; repeat=${reply.repeat}. ` +
      `receipt=${JSON.stringify(reply.receipt)}. Not an input ACK, processing, remote settlement or drain.`, error: false }
  }
  if (value.outcome !== "read") return unknown
  const bytes = Buffer.from(reply.b64, "base64")
  const text = bytes.toString("utf8")
  const utf8 = !bytes.includes(0) && Buffer.from(text, "utf8").equals(bytes)
  return { text: `Root v1 retained range: identity=${reply.retained.identity}; state=${reply.retained.state}; ` +
    `offset=${reply.offset}; length=${bytes.length}; next_offset=${reply.next_offset}; eof=${reply.eof}; ` +
    `received=${reply.retained.received ?? "unknown"}; losses=${JSON.stringify(reply.retained.losses)}. ` +
    "No local acceptance was recorded by this read." +
    `\n--- retained bytes (${utf8 ? "utf8" : "hex"}; ${bytes.length} bytes) ---\n${utf8 ? text : bytes.toString("hex")}`, error: false }
}

// One command through agent-bash and so through the root's Bash ingress.
function runRootBash(command, cwd, outputRequest, background = false) {
  return new Promise((done) => {
    let stdout = ""
    let stderr = ""
    let size = 0
    let child
    try {
      const argv = outputRequest ? (outputRequest.accept_output ? ["native-accept", outputRequest.output_identity]
        : ["native-output", outputRequest.output_identity, "--offset", String(outputRequest.output_offset ?? 0),
           "--length", String(outputRequest.output_length ?? 1024)])
        : ["run", "--delivery", background ? "async" : "sync", "--", "bash", "-lc", command]
      child = spawn(config.agent_bash_bin, argv, {
        cwd, env: process.env, stdio: ["ignore", "pipe", "pipe"],
      })
    } catch (error) {
      done({ text: outputRequest ? `Native output client not started (${error?.code ?? error}); no request sent.`
        : `agent-bash not started (${error?.code ?? error}); nothing was run.`, error: true })
      return
    }
    const take = (sink) => (chunk) => {
      size += chunk.length
      if (size > OUTPUT_LIMIT) {
        child.kill("SIGKILL")
        return
      }
      if (sink === "out") stdout += chunk
      else stderr += chunk
    }
    child.stdout.setEncoding("utf8").on("data", take("out"))
    child.stderr.setEncoding("utf8").on("data", take("err"))
    child.on("error", (error) =>
      done({ text: outputRequest ? `Native output client not started (${error?.code ?? error}); no request sent.`
        : `agent-bash not started (${error?.code ?? error}); nothing was run.`, error: true }))
    child.on("close", (code) => {
      if (size > OUTPUT_LIMIT) {
        done({ text: outputRequest ? "Native output result unresolved (result larger than this tool's bound); local acceptance unconfirmed."
          : "Root v1 result unresolved (result larger than this tool's bound); the command may have run; do not replay.", error: true })
        return
      }
      done(outputRequest ? renderRootOutput(code, stdout.trim())
        : background ? renderRootV1Async(code, stdout.trim(), stderr) : renderRootV1(code, stdout.trim(), stderr))
    })
  })
}

export function bashDecision(command, policy) {
  if (policy.authority === "trusted-task") return { run: true }
  if (Array.isArray(policy.allow) && policy.allow.includes(command)) return { run: true }
  return { run: false, text: `Denied by this root's bash policy: ${JSON.stringify(command)} is not a command it names. Nothing was run.` }
}

// --- ACP side -------------------------------------------------------------

let acp // the owner's connection, from session/new
let session // { id, cwd }
let claude // the Query
let fatalReason
const inputs = new Map() // uuid -> input
const byId = new Map() // messageId -> input
let turn // the current turn: { parent, answered: Set, buffered: [] }

class Pushed {
  constructor() {
    this.queue = []
    this.waiters = []
    this.ended = false
  }
  push(item) {
    const waiter = this.waiters.shift()
    if (waiter) waiter({ value: item, done: false })
    else this.queue.push(item)
  }
  end() {
    this.ended = true
    for (const waiter of this.waiters.splice(0)) waiter({ value: undefined, done: true })
  }
  [Symbol.asyncIterator]() {
    return {
      next: () => {
        if (this.queue.length) return Promise.resolve({ value: this.queue.shift(), done: false })
        if (this.ended) return Promise.resolve({ value: undefined, done: true })
        return new Promise((resolve) => this.waiters.push(resolve))
      },
    }
  }
}
const stdinOfClaude = new Pushed()

function notify(update) {
  if (!acp || !session) return Promise.resolve()
  return acp.notify(methods.client.session.update, { sessionId: session.id, update }).catch((error) =>
    log(`notify failed ${error?.message ?? error}`))
}

function notice(severity, title, description) {
  log(`notice ${severity} ${title}${description ? `: ${description}` : ""}`)
  return notify({ sessionUpdate: "notice", severity, title, ...(description ? { description } : {}) })
}

function latest(ids) {
  return [...ids].sort().at(-1)
}

// Claude Code named these uuids as consumed or answered.
function consumed(uuids) {
  const known = []
  for (const uuid of uuids) {
    const input = inputs.get(uuid)
    if (!input) continue
    known.push(input)
    if (!input.acked) {
      input.acked = true
      clearTimeout(input.timer)
      input.resolve()
    }
  }
  return known
}

function attribution(message) {
  if (Array.isArray(message.user_message_uuids)) return message.user_message_uuids
  return typeof message.user_message_uuid === "string" ? [message.user_message_uuid] : []
}

function startTurn() {
  if (!turn) {
    turn = { parent: undefined, answered: new Set(), buffered: [] }
    void notify({ sessionUpdate: "state_update", state: "running" })
  }
  return turn
}

function attribute(message) {
  const known = consumed(attribution(message))
  if (!known.length) return
  const current = startTurn()
  for (const input of known) current.answered.add(input.messageId)
  current.parent = latest(known.map((input) => input.messageId))
  for (const text of current.buffered.splice(0)) emitText(text, current.parent)
}

function emitText(text, parent) {
  void notify({
    sessionUpdate: "agent_message",
    messageId: mintId("amsg_"),
    content: [{ type: "text", text }],
    ...(parent ? { _meta: { "oulipoly.ai/parentMessageId": parent } } : {}),
  })
}

export function stopReasonOf(result) {
  if (result.subtype === "success" && !result.is_error) {
    if (result.stop_reason === "end_turn" || result.stop_reason == null) return "end_turn"
    if (result.stop_reason === "max_tokens") return "max_tokens"
    if (result.stop_reason === "refusal") return "refusal"
    return `_claude_${result.stop_reason}`
  }
  if (result.subtype === "success") return "_claude_error"
  return `_claude_${result.subtype}`
}

function openAcked() {
  return [...byId.values()].filter((input) => input.acked && !input.ended)
}

function endInputs(ids, stopReason) {
  const last = latest(ids)
  for (const input of byId.values()) if (input.messageId <= last) input.ended = true
  return notify({
    sessionUpdate: "state_update",
    state: "idle",
    stopReason,
    _meta: { "oulipoly.ai/lastUserMessageId": last },
  })
}

function onResult(result) {
  attribute(result)
  const current = startTurn()
  for (const text of current.buffered.splice(0)) emitText(text, current.parent)
  turn = undefined
  const stop = stopReasonOf(result)
  if (stop !== "end_turn") {
    const detail = [result.api_error_status ? `api status ${result.api_error_status}` : "",
      ...(Array.isArray(result.errors) ? result.errors : []),
      result.subtype === "success" && result.is_error ? String(result.result ?? "") : ""]
      .filter(Boolean).join("; ")
    void notice("error", `claude turn ended: ${stop}`, detail.slice(0, 4000) || undefined)
  }
  const denials = Array.isArray(result.permission_denials) ? result.permission_denials : []
  if (denials.length)
    void notice("warning", "claude permission denials", denials.map((d) => d.tool_name).join(", "))
  if (current.answered.size) {
    void endInputs(current.answered, stop)
    return
  }
  const open = openAcked()
  if (open.length) {
    void notice("error", "claude result answers no acknowledged input",
      "the turn's result carries no user_message_uuid of an input this receiver sent; ending the open input as unattributed")
    void endInputs(open.map((input) => input.messageId), "_claude_unattributed")
    return
  }
  void notify({ sessionUpdate: "state_update", state: "idle", stopReason: stop })
}

function onInit(init) {
  void notice("info", "claude init", JSON.stringify({
    claude_code_version: init.claude_code_version,
    session_id: init.session_id,
    model: init.model,
    permissionMode: init.permissionMode,
    apiKeySource: init.apiKeySource,
    tools: init.tools,
    mcp_servers: init.mcp_servers,
    capabilities: init.capabilities ?? "absent",
  }))
  if (init.model !== config.model)
    void notice("warning", "claude model differs", `asked ${config.model}, init reports ${init.model}`)
  const bash = (init.mcp_servers ?? []).find((server) => server.name === "oulipoly")
  if (bash && bash.status !== "connected")
    void notice("error", "claude bash tool server not connected", String(bash.status))
  const extra = (init.tools ?? []).filter((name) => !OWN_TOOLS.includes(name) && !config.tools.includes(name))
  if (extra.length) void notice("warning", "claude offers tools outside the launch policy", extra.join(", "))
}

function onMessage(message) {
  if (message.type === "user" && message.isReplay === true && typeof message.uuid === "string") {
    consumed([message.uuid])
    return
  }
  if (message.type === "system" && message.subtype === "init") return onInit(message)
  if (message.type === "result") return onResult(message)
  if (message.parent_tool_use_id) return // subagent frames carry no attribution
  if (message.type === "stream_event" || message.type === "thinking_tokens") {
    attribute(message)
    return
  }
  if (message.type === "assistant") {
    startTurn()
    attribute(message)
    if (message.error) void notice("error", `claude assistant error: ${message.error}`)
    const text = (message.message?.content ?? [])
      .filter((block) => block?.type === "text" && block.text)
      .map((block) => block.text)
      .join("")
    if (!text) return
    if (turn.parent) emitText(text, turn.parent)
    else turn.buffered.push(text)
  }
}

async function fatal(reason) {
  if (fatalReason) return
  fatalReason = reason
  await notice("error", "claude receiver ending", reason)
  for (const input of byId.values()) {
    if (!input.acked) {
      clearTimeout(input.timer)
      input.reject(new Error(`no consumption of this input: ${reason}`))
    }
  }
  const open = openAcked()
  if (open.length) await endInputs(open.map((input) => input.messageId), "_claude_exited")
  try {
    claude?.close()
  } catch {}
  process.exitCode = 3
  setTimeout(() => process.exit(3), 200)
}

async function pump() {
  try {
    for await (const message of claude) onMessage(message)
    await fatal("Claude Code ended its stream")
  } catch (error) {
    await fatal(`Claude Code or SDK failed: ${error?.message ?? error}`)
  }
}

function bashServer(cwd) {
  const description = config.bash.authority === "trusted-task"
    ? "Run a shell command (bash -lc) through this root's own attributed Bash ingress. By default synchronous: returns the root's record, wait status and combined output. With background: true it returns once started, and its end arrives later in this conversation as a separate message."
    : `Run one of these exact shell commands (bash -lc) through this root's own attributed Bash ingress (synchronously, or with background: true as background work whose end arrives later as a separate message); any other command is refused: ${JSON.stringify(config.bash.allow)}`
  return createSdkMcpServer({
    name: "oulipoly",
    version: "1",
    alwaysLoad: true,
    tools: [
      tool("bash", description + " Read retained output with output_identity/output_offset/output_length; accept_output records explicit exact local acceptance.", {
        output_identity: z.string().optional().describe("native retained identity (rv1o:...) or pre-seal work reference (rv1w:...) returned by Bash"),
        output_offset: z.number().int().nonnegative().optional().describe("native byte offset (default 0)"),
        output_length: z.number().int().min(1).max(1024).optional().describe("native range byte count, 1..1024 (default 1024)"),
        accept_output: z.boolean().optional().describe("explicit local acceptance of precisely output_identity; no ACK/processing/drain"),
        command: z.string().optional().describe("the shell command to run"),
        workdir: z.string().optional().describe("absolute working directory (default: the session's)"),
        background: z.boolean().optional().describe("run as background work: returns once started; its end arrives later in this conversation as a separate message (do not poll)"),
      }, async (args) => {
        const retained = ["output_identity", "output_offset", "output_length", "accept_output"].some(key => args[key] !== undefined)
        if (retained) {
          if (typeof args.output_identity !== "string" || args.command !== undefined || args.workdir !== undefined ||
              args.background !== undefined ||
              (args.accept_output && (args.output_offset !== undefined || args.output_length !== undefined))) {
            return { content: [{ type: "text", text: "Native output request conflicts with command/workdir or acceptance range. Nothing sent." }], isError: true }
          }
          const result = await runRootBash(undefined, cwd, args)
          return { content: [{ type: "text", text: result.text }], isError: result.error }
        }
        if (typeof args.command !== "string") return { content: [{ type: "text", text: "command or output_identity required. Nothing sent." }], isError: true }
        const decision = bashDecision(args.command, config.bash)
        if (!decision.run) return { content: [{ type: "text", text: decision.text }], isError: true }
        const workdir = args.workdir ?? cwd
        if (!workdir.startsWith("/"))
          return { content: [{ type: "text", text: "workdir must be absolute. Nothing was run." }], isError: true }
        const result = await runRootBash(args.command, workdir, undefined, args.background === true)
        return { content: [{ type: "text", text: result.text }], isError: result.error }
      }),
      ...(config.explore?.routes?.length ? [exploreTool()] : []),
    ],
  })
}

export function exploreTool(explore = config.explore) {
  return tool("explore", exploreDescription(explore), {
    question: z.string().describe("the orientation question for the explorer"),
    route: z.string().optional().describe("the explorer route (default: the only one)"),
  }, async (args, extra) => {
    const result = await exploreRequest(pickRoute(explore, args.route), args.question, { signal: extra?.signal })
    return { content: [{ type: "text", text: result.text }], isError: result.isError }
  })
}

function start(cwd) {
  return query({
    prompt: stdinOfClaude,
    options: {
      cwd,
      model: config.model,
      effort: config.effort,
      pathToClaudeCodeExecutable: config.claude_executable,
      env: claudeEnv(process.env, config.claude_env),
      settingSources: [],
      strictMcpConfig: true,
      mcpServers: { oulipoly: bashServer(cwd) },
      plugins: [],
      tools: config.tools,
      allowedTools: [...config.tools, ...OWN_TOOLS],
      disallowedTools: DENIED_TOOLS.filter((name) => !config.tools.includes(name)),
      // No permission callback: in `dontAsk` anything not pre-approved is
      // denied without asking; the result's permission_denials report it.
      permissionMode: "dontAsk",
      extraArgs: { "replay-user-messages": null },
      stderr: (data) => process.stderr.write(data.length > 8192 ? data.slice(0, 8192) + "\n[truncated]\n" : data),
    },
  })
}

function withTimeout(promise, ms, what) {
  let timer
  return Promise.race([
    promise.finally(() => clearTimeout(timer)),
    new Promise((_, reject) => {
      timer = setTimeout(() => reject(new Error(`${what} not within ${ms / 1000} s`)), ms)
    }),
  ])
}

function app() {
  return agent({ name: "oulipoly-claude-acp-v2" })
    .onRequest(methods.agent.initialize, () => ({
      protocolVersion: PROTOCOL_VERSION,
      info: { name: "claude-code+oulipoly-acp-v2", version: "0" },
      capabilities: { session: {} },
    }))
    .onRequest(methods.agent.session.resume, () => {
      throw new Error("session/resume is not served: each harness starts a new Claude Code session")
    })
    .onRequest(methods.agent.session.new, async ({ params, client }) => {
      if (session || claude) throw new Error("one session per receiver")
      if (typeof params.cwd !== "string" || !params.cwd.startsWith("/")) throw new Error("cwd must be absolute")
      claude = start(params.cwd)
      void pump()
      try {
        await withTimeout(claude.initializationResult(), START_MS, "Claude Code control handshake")
      } catch (error) {
        void fatal(`start failed: ${error?.message ?? error}`)
        throw error
      }
      acp = client
      session = { id: `claude-${randomUUID()}`, cwd: params.cwd }
      log(`session/new ${session.id} cwd=${params.cwd} model=${config.model} effort=${config.effort}`)
      return { sessionId: session.id }
    })
    .onRequest(methods.agent.session.prompt, async ({ params }) => {
      if (!session || params.sessionId !== session.id) throw new Error(`session ${params.sessionId} is not open here`)
      if (fatalReason) throw new Error(`receiver ending: ${fatalReason}`)
      const texts = params.prompt.map((block) => {
        if (block.type !== "text") throw new Error(`only text blocks are handled, got ${block.type}`)
        return block.text
      })
      const input = { uuid: randomUUID(), messageId: mintId(), acked: false, ended: false }
      const consumedNow = new Promise((resolve, reject) => {
        input.resolve = resolve
        input.reject = reject
      })
      input.timer = setTimeout(() => {
        if (!input.acked) void fatal(`no consumption echo for ${input.messageId} within ${ACK_MS / 1000} s`)
      }, ACK_MS)
      inputs.set(input.uuid, input)
      byId.set(input.messageId, input)
      stdinOfClaude.push({
        type: "user",
        uuid: input.uuid,
        session_id: "",
        message: { role: "user", content: texts.map((text) => ({ type: "text", text })) },
        parent_tool_use_id: null,
        client_composed: true,
      })
      log(`prompt sent messageId=${input.messageId} uuid=${input.uuid}`)
      await consumedNow
      log(`ACK messageId=${input.messageId}`)
      await notify({ sessionUpdate: "user_message", messageId: input.messageId, content: texts.map((text) => ({ type: "text", text })) })
      return { messageId: input.messageId }
    })
}

if (MAIN) {
  const connection = app().connect(ndJsonStream(Writable.toWeb(process.stdout), Readable.toWeb(process.stdin)))
  connection.closed.then(() => {
    log("owner connection closed")
    stdinOfClaude.end()
    try {
      claude?.close()
    } catch {}
    process.exit(fatalReason ? 3 : 0)
  })
}
