// A stand-in for the Claude Code executable, used only by this crate's
// native Claude tests (and the packaging fixture). It is not Claude Code:
// it speaks the subset of the stream-json control protocol the Agent SDK
// drives (`initialize`, user messages, `mcp_message` to the SDK's
// in-process MCP server) and scripts its replies from the prompt text.
// Tests install it, with a `#!<node>` line, where the SDK's platform
// package keeps the executable.
//
// FAKE_CLAUDE_RECORD names a file it appends JSON lines to: its argv, the
// environment it got, the MCP tool list and tool results.
// FAKE_CLAUDE_SCENARIO selects a behavior: normal (default), no-echo,
// crash, error-result, unattributed, model-mismatch, denial, and for
// session continuation: receiver-stops-on-second (at a new session's second
// input, before any echo, this stand-in SIGKILLs its parent, the receiver,
// and exits: a receiver stop with that input owed; a resumed process
// behaves normally), forget-session (the same, but its session is never
// recorded), resume-forks (the same, and a resumed process silently runs
// under a different session id).
//
// Sessions: `--session-id=<uuid>` names a new session, `--resume=<id>`
// continues a recorded one, as the Agent SDK passes them. FAKE_CLAUDE_SESSIONS
// names this stand-in's own session record (JSON lines; not a Claude store).
// A resume of an unrecorded id writes Claude Code 2.1.289's message ("No
// conversation found with session ID: <id>") to stderr and exits 1 before
// reading any input. A resumed process first emits that session's earlier
// history at its first input: the earlier input's replay echo, an assistant
// reply and a result, all attributed to that earlier input. The
// prior-blocks-{named,unnamed}-{before,after} controls instead emit two
// blocks (only the first stamped) and a named/unnamed prior result, before
// or after the current reply establishes its parent. unknown-result echoes
// current input then emits only a result naming a fresh unknown UUID;
// sessionless-attribution omits positive session identity on echo/replies.
// prior-blocks-named-{before-ack,after-long} pause 2.6 s (past the
// test receipt interval), then finish a positively attributed current turn.
//
// Prompt text: `EXPLORE <question>` invokes the published SDK explore
// callback and returns its complete text; `RUN <command>` calls the MCP
// `bash` tool with that command
// and answers `DONE <first output line>`; `BACKGROUND <command>` calls it
// with `background: true` and answers `STARTED <first line>`; an owner
// completion input (`[Background Bash completion]`) reads every retained
// byte it names, accepts that identity and answers `COMPLETED ...`;
// anything else is answered
// `ECHO <text>` in two assistant messages (the first carries the
// attribution, as Claude Code's first reply does).

import { appendFileSync, readFileSync } from "node:fs"
import { createInterface } from "node:readline"
import { randomUUID } from "node:crypto"

const scenario = process.env.FAKE_CLAUDE_SCENARIO || "normal"
const recordFile = process.env.FAKE_CLAUDE_RECORD
const sessionsFile = process.env.FAKE_CLAUDE_SESSIONS
const args = process.argv.slice(2)
const flag = (name) => {
  const at = args.indexOf(name)
  return at >= 0 ? args[at + 1] : undefined
}
const valueOf = (name) => args.find((arg) => arg.startsWith(`${name}=`))?.slice(name.length + 1)
const resuming = valueOf("--resume")
const multiBlockHistory = scenario.startsWith("prior-blocks-")
const stopsReceiverOnSecond = (["receiver-stops-on-second", "forget-session", "resume-forks"].includes(scenario) || multiBlockHistory) && !resuming

function record(value) {
  if (recordFile) appendFileSync(recordFile, JSON.stringify(value) + "\n")
}

function send(value) {
  process.stdout.write(JSON.stringify(value) + "\n")
}

function recorded() {
  try {
    return readFileSync(sessionsFile, "utf8").split("\n").filter(Boolean).map((line) => JSON.parse(line))
  } catch {
    return []
  }
}

function remember(value) {
  if (sessionsFile && scenario !== "forget-session") appendFileSync(sessionsFile, JSON.stringify(value) + "\n")
}

record({
  argv: args,
  env: Object.fromEntries(Object.entries(process.env).filter(([name]) =>
    /^(ANTHROPIC_|CLAUDE|ENABLE_TOOL_SEARCH|DISABLE_|OULIPOLY_|AGENT_BASH_|HOME$|NODE_OPTIONS)/.test(name))),
  cwd: process.cwd(),
})

const history = resuming ? recorded().filter((entry) => entry.session === resuming) : []
if (resuming && !history.length) {
  process.stderr.write(`No conversation found with session ID: ${resuming}\n`)
  process.exit(1)
}
const session = resuming ? (scenario === "resume-forks" ? randomUUID() : resuming) : (valueOf("--session-id") ?? randomUUID())
const earlier = history.filter((entry) => entry.user).at(-1)?.user
if (!resuming) remember({ session })

let requests = 0
const pending = new Map()
function mcp(message) {
  const request_id = `fake-${++requests}`
  send({ type: "control_request", request_id, request: { subtype: "mcp_message", server_name: "oulipoly", message } })
  return new Promise((resolve) => pending.set(request_id, resolve))
}

async function callBash(args) {
  const called = await mcp({ jsonrpc: "2.0", id: ++requests + 10, method: "tools/call", params: { name: "bash", arguments: args } })
  record({ retained_call: args, retained_result: called })
  const reply = called?.mcp_response?.result
  if (!reply) throw new Error("missing retained result")
  return reply
}

// Every retained byte of `identity`, page by page through the Bash tool.
async function retrieve(identity) {
  let offset = 0
  let pages = 0
  const chunks = []
  while (true) {
    const reply = await callBash({ output_identity: identity, output_offset: offset, output_length: 1024 })
    if (reply.isError) throw new Error(JSON.stringify(reply))
    const text = reply.content[0].text
    const mode = text.match(/--- retained bytes \((utf8|hex);/)[1]
    chunks.push(Buffer.from(text.split(" ---\n")[1], mode === "utf8" ? "utf8" : "hex"))
    const next = Number(text.match(/next_offset=(\d+)/)[1])
    if (next !== offset + chunks.at(-1).length) throw new Error("range progress mismatch")
    offset = next
    pages++
    if (text.includes("eof=true")) break
    if (pages > 1024) throw new Error("finite page bound")
  }
  return { acquired: Buffer.concat(chunks), pages }
}

// Documented SDK shape: several content blocks can share message.id,
// and only the first top-level block carries user_message_uuid(s).
function priorBlocks() {
  const id = randomUUID()
  const named = { user_message_uuid: earlier, user_message_uuids: [earlier] }
  for (const [text, attribution] of [["PRIOR first block", named], ["PRIOR unstamped block", {}]]) {
    send({ type: "assistant", uuid: randomUUID(), session_id: session, parent_tool_use_id: null,
      ...attribution, message: { id, role: "assistant", model: flag("--model"),
        content: [{ type: "text", text }], stop_reason: null } })
  }
  send({ type: "result", subtype: "success", uuid: randomUUID(), session_id: session,
    duration_ms: 1, duration_api_ms: 1, is_error: false, num_turns: 1, result: "PRIOR",
    stop_reason: "max_tokens", total_cost_usd: 0, usage: {}, modelUsage: {}, permission_denials: [],
    ...(scenario.includes("unnamed") ? {} : named) })
  record({ emitted_history_of: earlier })
}

let initSent = false
let mcpReady = false
async function turn(user) {
  const text = user.message.content.map((block) => block.text).join("")
  const uuids = [user.uuid]
  const attribution = scenario === "unattributed" ? {} : { user_message_uuid: user.uuid, user_message_uuids: uuids }
  if (!initSent) {
    initSent = true
    const tools = (flag("--tools") || "").split(",").filter(Boolean)
    send({
      type: "system", subtype: "init", session_id: session, uuid: randomUUID(),
      claude_code_version: "fake", cwd: process.cwd(), apiKeySource: "none",
      model: scenario === "model-mismatch" ? "claude-other" : flag("--model"),
      permissionMode: flag("--permission-mode"), tools: [...tools, "mcp__oulipoly__bash"],
      mcp_servers: [{ name: "oulipoly", status: "connected" }], slash_commands: [], output_style: "default",
      skills: [], plugins: [], capabilities: ["fake_capability_v1"],
    })
  }
  const assistant = (content, extra = {}) => send({
    type: "assistant", uuid: randomUUID(), session_id: session, parent_tool_use_id: null,
    message: { id: randomUUID(), role: "assistant", model: flag("--model"), content, stop_reason: null }, ...extra,
    ...(scenario === "sessionless-attribution" ? { session_id: "" } : {}),
  })
  const result = (fields) => send({
    type: "result", subtype: "success", uuid: randomUUID(), session_id: session, duration_ms: 1,
    duration_api_ms: 1, is_error: false, num_turns: 1, result: "", stop_reason: "end_turn",
    total_cost_usd: 0, usage: {}, modelUsage: {}, permission_denials: [], ...attribution, ...fields,
    ...(scenario === "sessionless-attribution" ? { session_id: "" } : {}),
  })
  if (scenario === "unknown-result") {
    const unknown = randomUUID()
    result({ user_message_uuid: unknown, user_message_uuids: [unknown], stop_reason: "max_tokens" })
    return // remain live, without any assistant attribution or later terminal
  }
  if (scenario === "crash") {
    assistant([{ type: "text", text: "about to fail" }], attribution)
    process.exit(1)
  }
  if (scenario === "error-result") {
    assistant([{ type: "text", text: "model not available" }], { ...attribution, error: "model_not_found" })
    result({ is_error: true, result: "model not available", api_error_status: 404 })
    return
  }
  const ready = async () => {
    if (!mcpReady) {
      mcpReady = true
      await mcp({ jsonrpc: "2.0", id: 1, method: "initialize", params: { protocolVersion: "2025-06-18", capabilities: {}, clientInfo: { name: "fake-claude", version: "0" } } })
      await mcp({ jsonrpc: "2.0", method: "notifications/initialized" })
      const listed = await mcp({ jsonrpc: "2.0", id: 2, method: "tools/list", params: {} })
      record({ tools_list: listed })
    }
  }
  // An owner completion of background work: reads every retained byte it
  // names through the Bash tool and explicitly accepts that exact identity.
  if (text.startsWith("[Background Bash completion]")) {
    await ready()
    const facts = JSON.parse(text.trim().split("\n").at(-1))
    const identity = facts.retained?.identity
    if (!identity) throw new Error("completion names no retained identity")
    const { acquired, pages } = await retrieve(identity)
    record({ completion_facts: facts, acquired_b64: acquired.toString("base64"), identity, pages })
    const accepted = await callBash({ output_identity: identity, accept_output: true })
    if (accepted.isError || !accepted.content[0].text.includes("repeat=false")) throw new Error("completion acceptance failed")
    const answer = `COMPLETED work=${facts.work} status=${facts.end.status} bytes=${acquired.length} pages=${pages} accepted=${identity}`
    assistant([{ type: "text", text: answer }], attribution)
    result({ result: answer })
    return
  }
  if (text.startsWith("BACKGROUND ")) {
    await ready()
    const arguments_ = { command: text.slice(11), background: true }
    assistant([{ type: "tool_use", id: "toolu_1", name: "mcp__oulipoly__bash", input: arguments_ }], attribution)
    const called = await mcp({ jsonrpc: "2.0", id: 3, method: "tools/call", params: { name: "bash", arguments: arguments_ } })
    record({ tool_result: called })
    const body = called?.mcp_response?.result?.content?.[0]?.text ?? ""
    send({ type: "user", uuid: randomUUID(), session_id: session, parent_tool_use_id: null,
      message: { role: "user", content: [{ type: "tool_result", tool_use_id: "toolu_1", content: body }] } })
    assistant([{ type: "text", text: `STARTED ${body.split("\n")[0]}` }])
    result({ result: `STARTED ${body.split("\n")[0]}` })
    return
  }
  if (text.startsWith("RUN ") || text.startsWith("EXPLORE ") || text.startsWith("RETAIN ")) {
    const exploring = text.startsWith("EXPLORE ")
    const name = exploring ? "explore" : "bash"
    const arguments_ = exploring ? { question: text.slice(8) } : { command: text.slice(text.startsWith("RETAIN ") ? 7 : 4) }
    await ready()
    assistant([{ type: "tool_use", id: "toolu_1", name: `mcp__oulipoly__${name}`, input: arguments_ }], attribution)
    const called = await mcp({ jsonrpc: "2.0", id: 3, method: "tools/call", params: { name, arguments: arguments_ } })
    record({ tool_result: called })
    let body = called?.mcp_response?.result?.content?.[0]?.text ?? ""
    if (text.startsWith("RETAIN ")) {
      const identity = body.match(/identity=(rv1o:[^\s]+)/)?.[1]
      if (!identity) throw new Error("no retained identity in actual Bash result")
      const call = async (args) => {
        const called = await mcp({ jsonrpc: "2.0", id: ++requests + 10, method: "tools/call", params: { name: "bash", arguments: args } })
        record({ retained_call: args, retained_result: called })
        const reply = called?.mcp_response?.result
        if (!reply) throw new Error("missing retained result")
        return reply
      }
      let offset = 0
      let pages = 0
      const chunks = []
      while (true) {
        const reference = "rv1w:" + identity.split(":").slice(1, 3).join(":")
        const reply = await call({ output_identity: offset === 0 ? reference : identity, output_offset: offset, output_length: 1024 })
        if (reply.isError) throw new Error(JSON.stringify(reply))
        const text = reply.content[0].text
        const mode = text.match(/--- retained bytes \((utf8|hex);/)[1]
        const chunk = text.split(" ---\n")[1]
        chunks.push(Buffer.from(chunk, mode === "utf8" ? "utf8" : "hex"))
        const next = Number(text.match(/next_offset=(\d+)/)[1])
        if (next !== offset + chunks.at(-1).length) throw new Error("range progress mismatch")
        offset = next
        pages++
        if (text.includes("eof=true")) break
        if (pages > 1024) throw new Error("finite page bound")
      }
      const acquired = Buffer.concat(chunks)
      record({ acquired_b64: acquired.toString("base64"), identity, pages })
      const bad = await call({ output_identity: identity.replace(/:[0-9a-f]{64}$/, ":" + "0".repeat(64)), accept_output: true })
      if (!bad.isError) throw new Error("wrong identity accepted")
      const accepted = await call({ output_identity: identity, accept_output: true })
      const repeat = await call({ output_identity: identity, accept_output: true })
      if (accepted.isError || repeat.isError || !accepted.content[0].text.includes("repeat=false") || !repeat.content[0].text.includes("repeat=true")) {
        throw new Error("exact acceptance or repeat failed")
      }
      body = `RETAINED identity=${identity} bytes=${acquired.length} pages=${pages}; exact local acceptance and repeat returned`
    }
    send({ type: "user", uuid: randomUUID(), session_id: session, parent_tool_use_id: null,
      message: { role: "user", content: [{ type: "tool_result", tool_use_id: "toolu_1", content: body }] } })
    assistant([{ type: "text", text: `DONE ${exploring ? body : body.split("\n")[0]}` }])
    result({ result: `DONE ${exploring ? body : body.split("\n")[0]}` })
    return
  }
  if (resuming && multiBlockHistory && scenario.endsWith("before")) priorBlocks()
  assistant([{ type: "text", text: "thinking about it" }], attribution)
  if (resuming && multiBlockHistory && scenario.includes("-after")) priorBlocks()
  if (resuming && ["prior-blocks-named-before-ack", "prior-blocks-named-after-long"].includes(scenario)) {
    await new Promise((resolve) => setTimeout(resolve, 2600))
    assistant([{ type: "text", text: "current progress after quiet work" }], attribution)
  }
  assistant([{ type: "text", text: `ECHO ${text}` }])
  const denials = scenario === "denial" ? [{ tool_name: "Write", tool_use_id: "toolu_9", tool_input: {} }] : []
  result({ result: `ECHO ${text}`, permission_denials: denials })
}

let chain = Promise.resolve()
let users = 0
const lines = createInterface({ input: process.stdin })
lines.on("line", (line) => {
  let message
  try {
    message = JSON.parse(line)
  } catch {
    return
  }
  if (message.type === "control_request") {
    if (message.request?.subtype === "initialize")
      record({ initialize: { sdkMcpServers: message.request.sdkMcpServers, hooks: message.request.hooks ?? null } })
    send({ type: "control_response", response: { subtype: "success", request_id: message.request_id,
      response: message.request?.subtype === "initialize" ? { commands: [], models: [], account: {} } : {} } })
    return
  }
  if (message.type === "control_response") {
    const resolve = pending.get(message.response?.request_id)
    pending.delete(message.response?.request_id)
    resolve?.(message.response?.response)
    return
  }
  if (message.type === "user") {
    users++
    record({ user: { uuid: message.uuid, client_composed: message.client_composed ?? null, session_id: message.session_id ?? null } })
    if (stopsReceiverOnSecond && users === 2) {
      record({ stopped_receiver_at_input: users, receiver_pid: process.ppid })
      process.kill(process.ppid, "SIGKILL")
      process.exit(1)
    }
    if (!resuming) remember({ session, user: message.uuid })
    if (scenario === "no-echo") return
    if (resuming && users === 1 && earlier && !multiBlockHistory) {
      // The resumed session's earlier history, before the new input's echo.
      const earlierAttribution = { user_message_uuid: earlier, user_message_uuids: [earlier] }
      send({ type: "user", uuid: earlier, session_id: session, parent_tool_use_id: null, isReplay: true,
        message: { role: "user", content: [{ type: "text", text: "earlier input" }] } })
      send({ type: "assistant", uuid: randomUUID(), session_id: session, parent_tool_use_id: null, ...earlierAttribution,
        message: { id: randomUUID(), role: "assistant", model: flag("--model"), content: [{ type: "text", text: "PRIOR answer" }], stop_reason: null } })
      send({ type: "result", subtype: "success", uuid: randomUUID(), session_id: session, duration_ms: 1, duration_api_ms: 1,
        is_error: false, num_turns: 1, result: "PRIOR answer", stop_reason: "end_turn", total_cost_usd: 0, usage: {},
        modelUsage: {}, permission_denials: [], ...earlierAttribution })
      record({ emitted_history_of: earlier })
    }
    if (resuming && multiBlockHistory && scenario.endsWith("before-ack")) priorBlocks()
    if (args.includes("--replay-user-messages")) send({ ...message, session_id: scenario === "sessionless-attribution" ? "" : session, isReplay: true })
    chain = chain.then(() => turn(message))
  }
})
lines.on("close", () => chain.then(() => process.exit(0)))
