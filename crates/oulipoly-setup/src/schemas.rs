/// JSON schema passed to `claude --json-schema` to constrain agent output.
pub const AGENT_TURN_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "actions": {
      "type": "array",
      "items": {
        "type": "object",
        "properties": {
          "type": { "type": "string", "enum": [
            "status", "run_command", "write_config", "test_integration",
            "ask_user", "sync_skill", "sync_mcp", "update_memory", "complete"
          ]},
          "message": { "type": "string" },
          "command": { "type": "string" },
          "args": { "type": "array", "items": { "type": "string" } },
          "description": { "type": "string" },
          "path": { "type": "string" },
          "content": { "type": "string" },
          "model_name": { "type": "string" },
          "action": { "type": "object" },
          "source_cli": { "type": "string" },
          "target_cli": { "type": "string" },
          "skill_name": { "type": "string" },
          "mcp_name": { "type": "string" },
          "config": { "type": "string" },
          "node_type": { "type": "string" },
          "label": { "type": "string" },
          "data": { "type": "string" },
          "edges": { "type": "array", "items": {
            "type": "object",
            "properties": {
              "target_label": { "type": "string" },
              "edge_type": { "type": "string" }
            },
            "required": ["target_label", "edge_type"]
          }},
          "summary": { "type": "string" },
          "items": { "type": "array", "items": { "type": "string" } }
        },
        "required": ["type"]
      }
    },
    "done": { "type": "boolean" }
  },
  "required": ["actions", "done"]
}"#;
