// ACP v2 agent endpoint hosted INSIDE the OpenCode process as a server
// plugin, for a root supervisor harness with `"endpoint": "unix-socket"`.
// There is no bridge process: the listener lives in the OpenCode process
// that owns the native conversations.
//
// - Wire: upstream @agentclientprotocol/sdk/experimental/v2 AgentApp
//   (schema-v2.0.0-alpha.7), ND-JSON over the Unix socket the owner chose
//   and passed as OULIPOLY_ACP_V2_SOCKET. Without it nothing listens.
// - Scope: one OpenCode instance, its directory. session/new and
//   session/resume refuse any other cwd; resume also refuses a native
//   session recorded for another directory.
// - session/new creates a native session; session/resume attaches to an
//   existing native one without replay (the native store has the history).
// - session/prompt inserts through native POST /session/{id}/prompt_async
//   with a minted native messageID and the session's own configured model.
//   It answers { messageId } only after the native bus reported the user
//   message and its text part AND a native GET returns that message with
//   exactly the submitted text. Never from the HTTP status alone; a non-2xx
//   status is an error.
// - OULIPOLY_ACP_V2_NO_REPLY=1 makes the host insertion-only: native noReply,
//   so no model turn starts. Unset, the native loop runs whatever model the
//   session is configured with, and the turn reaches every connection
//   attached to the session:
//   - a native session error as an `error` `notice`; one that arrives while
//     a prompt still waits for its insertion fails that prompt (no ACK);
//   - at its end, read back from the native store rather than taken from the
//     event stream (native busy repeats, and the assistant message completes
//     after idle): each assistant text part since the last user message as
//     `agent_message` (native assistant message id, the part's text), then
//     one `state_update` idle whose stopReason maps the last assistant
//     message: error -> `_native_error`, finish "stop" -> end_turn,
//     "length" -> max_tokens, other -> `_native_<finish>`. The end is the
//     first native idle after busy once that message is completed (or none
//     exists); repeated native idles are not repeated;
//   - a native permission request as `session/request_permission` to the
//     attached connection. Only a selected allow_once option allows it once;
//     any other answer, a JSON-RPC error (e.g. method not found) or no
//     attached connection rejects it natively, reported as a `warning`
//     notice. A native question is always rejected the same way.
//   ACK keeps its meaning: insertion read back, never the turn's completion.
// - No dedup contract is advertised: nothing here survives a restart.
// - Diagnostics go to OULIPOLY_ACP_V2_LOG, never stderr (a TUI's screen).

import net from "node:net"
import { appendFileSync } from "node:fs"
import { randomBytes } from "node:crypto"
import { Readable, Writable } from "node:stream"
import { agent, methods, ndJsonStream, PROTOCOL_VERSION } from "@agentclientprotocol/sdk/experimental/v2"

type Notify = (update: unknown) => void
type Waiter = { session: string; message: boolean; text: boolean; done: () => void; fail: (error: Error) => void }
type Ask = (title: string, description: string) => Promise<string>
// A native turn between busy and its reported end.
type Turn = { error: boolean; idle: boolean; ended: boolean }

const g = globalThis as any
const shared: {
  waiters: Map<string, Waiter>
  // Native session id -> the update senders of connections attached to it.
  sessions: Map<string, Set<Notify>>
  // Native session id -> how its attached connections are asked for permission.
  asks: Map<string, Set<Ask>>
  // Native session id -> what the current native turn has shown so far.
  turns: Map<string, Turn>
} = (g.__oulipolyAcpV2 ??= { waiters: new Map(), sessions: new Map(), asks: new Map(), turns: new Map() })

function log(line: string) {
  const file = process.env.OULIPOLY_ACP_V2_LOG
  if (file) appendFileSync(file, `[acp-v2] ${new Date().toISOString()} ${line}\n`)
}

// Same shape as OpenCode's Identifier.ascending("message"): msg_ + 12 hex + 14 base62.
let lastMs = 0
let counter = 0
function mintMessageID(): string {
  const now = Date.now()
  if (now !== lastMs) {
    lastMs = now
    counter = 0
  }
  counter++
  const value = BigInt(now) * 0x1000n + BigInt(counter)
  const time = Buffer.alloc(6)
  for (let i = 0; i < 6; i++) time[i] = Number((value >> BigInt(40 - 8 * i)) & 0xffn)
  const chars = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz"
  let tail = ""
  for (const b of randomBytes(14)) tail += chars[b % 62]
  return "msg_" + time.toString("hex") + tail
}

function settle(id: string, field: "message" | "text") {
  const waiter = shared.waiters.get(id)
  if (!waiter) return
  waiter[field] = true
  if (waiter.message && waiter.text) waiter.done()
}

function notify(session: string, update: unknown) {
  for (const send of shared.sessions.get(session) ?? []) send(update)
}

function stopReason(info: any, error: boolean): string | undefined {
  if (error || info?.error) return "_native_error"
  if (info?.finish === "stop") return "end_turn"
  if (info?.finish === "length") return "max_tokens"
  return info?.finish ? `_native_${info.finish}` : undefined
}

// Reports the turn's end from a native readback, or waits for its last
// assistant message to complete.
async function endTurn(client: any, session: string, current: Turn) {
  const read = await client.session.messages({ path: { id: session }, throwOnError: true }).catch((error: any) => {
    log(`turn readback failed ${session} ${error?.message ?? error}`)
    return undefined
  })
  if (current.ended) return
  const messages: any[] = read?.data ?? []
  const lastUser = messages.map((m) => m.info.role).lastIndexOf("user")
  const replies = messages.slice(lastUser + 1).filter((m) => m.info.role === "assistant")
  const last = replies.at(-1)?.info
  // Its completion event calls again.
  if (read && last && !last.time?.completed && !last.error) return
  current.ended = true
  shared.turns.delete(session)
  for (const reply of replies)
    for (const part of reply.parts)
      if (part.type === "text" && part.text)
        notify(session, {
          sessionUpdate: "agent_message",
          messageId: reply.info.id,
          content: [{ type: "text", text: part.text }],
        })
  const stop = read ? stopReason(last, current.error) : current.error ? "_native_error" : "_native_unread"
  log(`turn end ${session} replies=${replies.length} stop=${stop}`)
  notify(session, { sessionUpdate: "state_update", state: "idle", ...(stop ? { stopReason: stop } : {}) })
}

// The first attached connection answers; none attached is a refusal.
async function ask(session: string, title: string, description: string): Promise<string> {
  const asker = [...(shared.asks.get(session) ?? [])][0]
  if (!asker) return "refused: no attached connection"
  try {
    return await asker(title, description)
  } catch (error: any) {
    return `refused: ${error?.code ?? ""} ${error?.message ?? String(error)}`.trim()
  }
}

async function permission(client: any, p: any) {
  const title = `${p.permission} ${(p.patterns ?? []).join(" ")}`.trim()
  const answer = await ask(p.sessionID, title, `native permission ${p.id}`)
  const response = answer === "allow_once" ? "once" : "reject"
  log(`permission ${p.id} ${title} owner=${answer} native=${response}`)
  await client
    .postSessionIdPermissionsPermissionId({
      path: { id: p.sessionID, permissionID: p.id },
      body: { response },
      throwOnError: true,
    })
    .catch((error: any) => log(`permission ${p.id} reply failed ${error?.message ?? error}`))
  if (response === "reject")
    notify(p.sessionID, {
      sessionUpdate: "notice",
      severity: "warning",
      title: `permission rejected: ${title}`,
      description: answer,
    })
}

async function question(client: any, p: any) {
  log(`question ${p.id} rejected: no question route`)
  await client._client
    .post({ url: "/question/{requestID}/reject", path: { requestID: p.id }, throwOnError: true })
    .catch((error: any) => log(`question ${p.id} reject failed ${error?.message ?? error}`))
  notify(p.sessionID, {
    sessionUpdate: "notice",
    severity: "warning",
    title: "question rejected",
    description: "refused: no question route",
  })
}

function listen(socketPath: string, client: any, directory: string) {
  // Connections, each with the sessions it attached to.
  const app = (attached: Map<string, Attachment>) =>
    agent({ name: "oulipoly-opencode-acp-v2" })
      .onRequest(methods.agent.initialize, () => ({
        protocolVersion: PROTOCOL_VERSION,
        info: { name: "opencode+oulipoly-acp-v2", version: "0" },
        // `{}`: the baseline session surface, session/resume included.
        capabilities: { session: {} },
      }))
      .onRequest(methods.agent.session.resume, async ({ params, client: acp }) => {
        if (params.cwd !== directory) throw new Error(`cwd ${params.cwd} is not this host's ${directory}`)
        const existing = await client.session.get({ path: { id: params.sessionId }, throwOnError: true })
        const id = existing.data.id as string
        if (existing.data.directory !== directory)
          throw new Error(`session ${id} belongs to ${existing.data.directory}, not ${directory}`)
        attach(attached, id, acp)
        log(`session/resume native=${id} title=${JSON.stringify(existing.data.title)}`)
        return {}
      })
      .onRequest(methods.agent.session.new, async ({ params, client: acp }) => {
        if (params.cwd !== directory) throw new Error(`cwd ${params.cwd} is not this host's ${directory}`)
        const created = await client.session.create({ body: {}, throwOnError: true })
        const id = created.data.id as string
        attach(attached, id, acp)
        log(`session/new native=${id}`)
        return { sessionId: id }
      })
      .onRequest(methods.agent.session.prompt, async ({ params }) => {
        if (!attached.has(params.sessionId)) throw new Error(`session ${params.sessionId} is not open here`)
        const texts = params.prompt.map((block: any) => {
          if (block.type !== "text") throw new Error(`only text blocks are handled, got ${block.type}`)
          return block.text as string
        })
        const noReply = process.env.OULIPOLY_ACP_V2_NO_REPLY === "1"
        const messageID = mintMessageID()
        const inserted = new Promise<void>((done, fail) =>
          shared.waiters.set(messageID, { session: params.sessionId, message: false, text: false, done, fail }),
        )
        try {
          const res = await client.session.promptAsync({
            path: { id: params.sessionId },
            body: { messageID, parts: texts.map((text) => ({ type: "text", text })), noReply },
          })
          const status = res.response?.status ?? 0
          log(`prompt_async http=${status} messageID=${messageID} noReply=${noReply}`)
          if (status < 200 || status > 299) throw new Error(`native prompt_async answered ${status}`)
          await inserted
        } finally {
          shared.waiters.delete(messageID)
        }
        const readback = await client.session.message({
          path: { id: params.sessionId, messageID },
          throwOnError: true,
        })
        const info = readback.data.info
        const body = readback.data.parts.filter((part: any) => part.type === "text").map((part: any) => part.text)
        if (info.id !== messageID || info.role !== "user" || JSON.stringify(body) !== JSON.stringify(texts))
          throw new Error("native readback mismatch")
        log(`ACK messageId=${messageID}`)
        notify(params.sessionId, {
          sessionUpdate: "user_message",
          messageId: messageID,
          content: texts.map((text) => ({ type: "text", text })),
        })
        return { messageId: messageID }
      })

  const server = net.createServer((socket) => {
    const attached = new Map<string, Attachment>()
    socket.on("close", () => {
      for (const [id, { send, asker }] of attached) {
        shared.sessions.get(id)?.delete(send)
        shared.asks.get(id)?.delete(asker)
      }
    })
    socket.on("error", (error) => log(`connection error ${error.message}`))
    const output = Writable.toWeb(socket) as WritableStream<Uint8Array>
    const input = Readable.toWeb(socket) as ReadableStream<Uint8Array>
    app(attached).connect(ndJsonStream(output, input))
  })
  server.on("error", (error) => log(`listener error ${error.message}`))
  server.listen(socketPath, () => log(`listening ${socketPath} pid=${process.pid} directory=${directory}`))
}

type Attachment = { send: Notify; asker: Ask }

function attach(attached: Map<string, Attachment>, id: string, acp: any) {
  const send: Notify = (update) => void acp.notify(methods.client.session.update, { sessionId: id, update } as any)
  const asker: Ask = async (title, description) => {
    const answer: any = await acp.request(methods.client.session.requestPermission, {
      sessionId: id,
      title,
      description,
      options: [
        { optionId: "allow_once", name: "Allow once", kind: "allow_once" },
        { optionId: "reject_once", name: "Reject", kind: "reject_once" },
      ],
    })
    const outcome = answer?.outcome
    return outcome?.outcome === "selected" ? String(outcome.optionId) : `refused: ${outcome?.outcome ?? "no outcome"}`
  }
  const previous = attached.get(id)
  if (previous) {
    shared.sessions.get(id)?.delete(previous.send)
    shared.asks.get(id)?.delete(previous.asker)
  }
  attached.set(id, { send, asker })
  let senders = shared.sessions.get(id)
  if (!senders) shared.sessions.set(id, (senders = new Set()))
  senders.add(send)
  let askers = shared.asks.get(id)
  if (!askers) shared.asks.set(id, (askers = new Set()))
  askers.add(asker)
}

export const AcpV2Endpoint = async ({ client, directory }: any) => {
  const socketPath = process.env.OULIPOLY_ACP_V2_SOCKET
  if (socketPath && !g.__oulipolyAcpV2Listening) {
    g.__oulipolyAcpV2Listening = true
    listen(socketPath, client, directory)
  }
  return {
    event: async ({ event }: any) => {
      const p = event.properties ?? {}
      if (process.env.OULIPOLY_ACP_V2_TRACE === "1" && event.type !== "message.part.delta")
        log(`event ${event.type} ${p.sessionID ?? p.info?.sessionID ?? p.part?.sessionID ?? ""} ${p.info?.role ?? p.part?.type ?? p.status?.type ?? ""}`)
      if (event.type === "message.updated" && p.info?.role === "user") settle(p.info.id, "message")
      if (event.type === "message.updated" && p.info?.role === "assistant" && p.info.time?.completed) {
        const current = shared.turns.get(p.info.sessionID)
        if (current?.idle) void endTurn(client, p.info.sessionID, current)
      }
      if (event.type === "message.part.updated" && p.part?.type === "text") settle(p.part.messageID, "text")
      if (event.type === "session.error" && p.sessionID) {
        const description = String(p.error?.data?.message ?? p.error?.name ?? "unknown")
        log(`session.error ${p.sessionID} ${p.error?.name ?? ""}`)
        const current = shared.turns.get(p.sessionID)
        if (current) current.error = true
        for (const [id, waiter] of shared.waiters)
          if (waiter.session === p.sessionID) {
            shared.waiters.delete(id)
            waiter.fail(new Error(`native session error before insertion: ${description}`))
          }
        notify(p.sessionID, { sessionUpdate: "notice", severity: "error", title: "native session error", description })
      }
      if (event.type === "permission.asked") void permission(client, p)
      if (event.type === "question.asked") void question(client, p)
      if (event.type === "session.status" && p.status?.type === "busy" && !shared.turns.has(p.sessionID)) {
        shared.turns.set(p.sessionID, { error: false, idle: false, ended: false })
        notify(p.sessionID, { sessionUpdate: "state_update", state: "running" })
      }
      if (event.type === "session.status" && p.status?.type === "idle") {
        const current = shared.turns.get(p.sessionID)
        if (current) {
          current.idle = true
          void endTurn(client, p.sessionID, current)
        }
      }
    },
  }
}
