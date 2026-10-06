// The receiver's session continuation guards, without Claude Code, an
// account or a model. Run like the other native tests: in a native source
// overlay with published node_modules and OULIPOLY_CLAUDE_RECEIVER_NO_MAIN=1.
import { test } from "node:test"
import assert from "node:assert/strict"
import { randomUUID } from "node:crypto"
import { resumableSessionId, sessionOf } from "../claude/acp-v2-receiver.mjs"

test("only a session id of the receiver's own UUID form is continued", () => {
  const id = randomUUID()
  assert.equal(resumableSessionId(id), id)
  for (const invalid of [
    undefined, null, 7, "", " " + id, id + "\n", id.toUpperCase(),
    `claude-${id}`, // an id minted by an earlier receiver version
    "/home/user/.claude/projects/x/session.jsonl", // a transcript path Claude Code would load
    "../" + id, "latest", id.replaceAll("-", ""),
  ]) {
    assert.throws(() => resumableSessionId(invalid), /not a Claude Code session this receiver opened; nothing started/,
      JSON.stringify(invalid))
  }
})

test("a frame of another session is a mismatch; none named is not a match", () => {
  const id = randomUUID()
  assert.equal(sessionOf({ session_id: id }, id), "same")
  assert.equal(sessionOf({ session_id: randomUUID() }, id), "mismatch")
  assert.equal(sessionOf({ session_id: "" }, id), "absent")
  assert.equal(sessionOf({}, id), "absent")
  assert.equal(sessionOf({ session_id: id }, undefined), "mismatch")
})
